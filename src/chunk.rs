//! Voxel chunk: block storage + face-culled mesh generation.
//!
//! TODO(P0): 当前使用 32³ 正方体区块，后期需迁移到 16×32×16 SubChunk。
//! 原因：Y 轴范围 ±20480（40960 格），是 Minecraft 的 160 倍。
//! 16×32×16 设计：
//!   - Y 轴保持 32 格：覆盖完整地层（石→土→草→空气），40960/32=1280 SubChunk/列
//!   - XZ 缩小到 16 格：更细粒度加载/卸载，单 SubChunk 内存从 32KB 降至 8KB
//!   - SuperChunk 合批：8×8×4 SubChunk = 256×128×64 米，合并为 1 个 Draw Call
//! 迁移时需将 CHUNK_SIZE 改为分轴常量：CHUNK_SIZE_XZ=16, CHUNK_SIZE_Y=32
//! 参见 docs/架构总纲.md §3.1 垂直分层

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use std::hash::Hash;

/// Size of one dimension of a chunk (32³ blocks per chunk).
/// TODO(P0): 后期改为 CHUNK_SIZE_XZ=16, CHUNK_SIZE_Y=32（16×32×16 SubChunk）
pub const CHUNK_SIZE: usize = 32;

/// A single block type identifier.
/// 0 = air (not rendered), 1 = grass, 2 = stone, 3 = dirt.
pub type BlockId = u8;

/// Face direction on a block.
#[derive(Clone, Copy)]
pub enum Face {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

/// Block type → color mapping.
/// 1 = grass (green), 2 = stone (gray), 3 = dirt (brown), 4 = sand (yellow)
fn block_color(block_id: BlockId) -> [f32; 4] {
    match block_id {
        1 => [0.2, 0.8, 0.2, 1.0], // green (grass)
        2 => [0.5, 0.5, 0.5, 1.0], // gray (stone)
        3 => [0.6, 0.4, 0.2, 1.0], // brown (dirt)
        4 => [0.9, 0.8, 0.2, 1.0], // yellow (sand)
        _ => [0.0, 0.0, 0.0, 1.0], // black (unknown)
    }
}

/// All 6 faces of a block in order: +X, -X, +Y, -Y, +Z, -Z
const FACES: [(Face, [i32; 3]); 6] = [
    (Face::Right, [1, 0, 0]),
    (Face::Left, [-1, 0, 0]),
    (Face::Top, [0, 1, 0]),
    (Face::Bottom, [0, -1, 0]),
    (Face::Front, [0, 0, 1]),
    (Face::Back, [0, 0, -1]),
];

/// Chunk data: three-state storage for a 32x32x32 voxel chunk.
///
/// - `Empty`: all air, zero memory allocation
/// - `Uniform`: all blocks are the same type, stores only the BlockId (2 bytes)
/// - `Mixed`: heterogeneous data, stores the full 32³ array (64 KB)
#[derive(Component, Clone)]
pub enum ChunkData {
    Empty,
    Uniform(BlockId),
    Mixed(Vec<BlockId>),
}

impl ChunkData {
    /// Creates a new chunk filled entirely with air.
    pub fn new() -> Self {
        Self::Empty
    }

    /// Creates a chunk pre-filled with a specific block type.
    pub fn filled(block_id: BlockId) -> Self {
        Self::Uniform(block_id)
    }

    /// Linear index from 3D coordinates.
    fn flatten(x: usize, y: usize, z: usize) -> usize {
        z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x
    }

    /// Returns the block ID at (x, y, z). Returns 0 (air) if out of bounds.
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(id) => *id,
            ChunkData::Mixed(data) => {
                if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
                    data[Self::flatten(x, y, z)]
                } else {
                    0
                }
            }
        }
    }

    /// Sets the block at (x, y, z). No-op if out of bounds.
    /// Automatically upgrades from Empty/Uniform to Mixed when needed.
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return;
        }

        match self {
            ChunkData::Empty => {
                *self = ChunkData::Uniform(id);
            }
            ChunkData::Uniform(current_id) => {
                if *current_id != id {
                    // Upgrade to Mixed: fill with current id first, then write new id
                    let mut data = vec![*current_id; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
                    data[Self::flatten(x, y, z)] = id;
                    *self = ChunkData::Mixed(data);
                }
            }
            ChunkData::Mixed(data) => {
                data[Self::flatten(x, y, z)] = id;
            }
        }
    }

    /// Returns whether the face of block at (x, y, z) in direction `face`
    /// should be rendered.
    ///
    /// A face is exposed when the neighbour has a *different* block type
    /// (or is air / out of bounds), because the neighbour's own mesh
    /// cannot fill that gap.
    pub fn is_face_visible(&self, x: usize, y: usize, z: usize, face: &[i32; 3]) -> bool {
        let nx = x as i32 + face[0];
        let ny = y as i32 + face[1];
        let nz = z as i32 + face[2];

        // Out of chunk bounds → always exposed
        if nx < 0
            || ny < 0
            || nz < 0
            || nx >= CHUNK_SIZE as i32
            || ny >= CHUNK_SIZE as i32
            || nz >= CHUNK_SIZE as i32
        {
            return true;
        }

        // Neighbor has a different block type (or is air) → this face is exposed
        self.get(nx as usize, ny as usize, nz as usize) != self.get(x, y, z)
    }
}

impl Default for ChunkData {
    fn default() -> Self {
        Self::Empty
    }
}

/// Backward-compatible type alias: `Chunk` is now `ChunkData::Mixed`
/// but the simpler name is kept for ergonomics.
pub type Chunk = ChunkData;

/// Generates a face-culled mesh for the chunk.
///
/// Only renders faces adjacent to a different block type (or air / chunk boundary).
/// Returns (positions, colors, normals, indices).
///
/// Colors are per-vertex RGBA based on block type (solid color, no textures).
pub fn generate_chunk_mesh(
    chunk: &Chunk,
) -> (Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut colors = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_id = chunk.get(x, y, z);
                if block_id == 0 {
                    continue; // air
                }

                for (face, offset) in FACES.iter().cloned() {
                    if !chunk.is_face_visible(x, y, z, &offset) {
                        continue; // face is occluded
                    }

                    let base_index = positions.len() as u32;
                    let (face_verts, face_color, face_normal) = face_quad(x, y, z, face, block_id);
                    positions.extend(face_verts);
                    colors.extend([face_color; 4]);
                    normals.extend([face_normal; 4]);
                    // Reverse winding order: (0,2,1) and (0,3,2) to get
                    // counter-clockwise triangles when viewed from the
                    // face normal direction (Bevy uses right-hand coordinate
                    // system with back-face culling = CCW front faces).
                    indices.extend([
                        base_index,
                        base_index + 2,
                        base_index + 1,
                        base_index,
                        base_index + 3,
                        base_index + 2,
                    ]);
                }
            }
        }
    }

    (positions, colors, normals, indices)
}

/// Returns the 4 vertices, color, and normal for a single face.
fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    block_id: BlockId,
) -> ([[f32; 3]; 4], [f32; 4], [f32; 3]) {
    // 方块 (x, y, z) 覆盖世界空间 [x, y, z] → [x+1, y+1, z+1]，
    // 与 DDA 射线检测的 floor() 坐标系保持一致。
    let (verts, normal) = match face {
        Face::Top => {
            // +Y face
            (
                [
                    [x as f32, y as f32 + 1.0, z as f32],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                    [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                ],
                [0.0, 1.0, 0.0],
            )
        }
        Face::Bottom => {
            // -Y face
            (
                [
                    [x as f32, y as f32, z as f32 + 1.0],
                    [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                    [x as f32 + 1.0, y as f32, z as f32],
                    [x as f32, y as f32, z as f32],
                ],
                [0.0, -1.0, 0.0],
            )
        }
        Face::Right => {
            // +X face
            (
                [
                    [x as f32 + 1.0, y as f32, z as f32],
                    [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                ],
                [1.0, 0.0, 0.0],
            )
        }
        Face::Left => {
            // -X face
            (
                [
                    [x as f32, y as f32, z as f32 + 1.0],
                    [x as f32, y as f32, z as f32],
                    [x as f32, y as f32 + 1.0, z as f32],
                    [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                ],
                [-1.0, 0.0, 0.0],
            )
        }
        Face::Front => {
            // +Z face
            (
                [
                    [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                    [x as f32, y as f32, z as f32 + 1.0],
                    [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                ],
                [0.0, 0.0, 1.0],
            )
        }
        Face::Back => {
            // -Z face
            (
                [
                    [x as f32, y as f32, z as f32],
                    [x as f32 + 1.0, y as f32, z as f32],
                    [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                    [x as f32, y as f32 + 1.0, z as f32],
                ],
                [0.0, 0.0, -1.0],
            )
        }
    };

    let color = block_color(block_id);

    (verts, color, normal)
}

// --------------------------------------------------------------------------
// Terrain helpers
// --------------------------------------------------------------------------

/// Fills a chunk with only the bottom 3 layers.
/// y=0 → stone  (BlockId=2, gray, bottom)
/// y=1 → dirt   (BlockId=3, brown, middle)
/// y=2 → random top layer: grass (green), sand (yellow), or stone (gray)
/// y>=3 → air   (BlockId=0)
pub fn fill_terrain(chunk: &mut Chunk) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            chunk.set(x, 0, z, 2); // stone  — bottom (gray)
            chunk.set(x, 1, z, 3); // dirt   — middle (brown)

            // Simple deterministic pseudo-random based on coordinates
            let hash = (x as u32).wrapping_mul(73856093) ^ (z as u32).wrapping_mul(19349663);
            let top_block = match hash % 3 {
                0 => 1, // grass  (green)
                1 => 4, // sand   (yellow)
                _ => 2, // stone  (gray)
            };
            chunk.set(x, 2, z, top_block);
            // y >= 3: air (implicit, chunk is zero-initialized)
        }
    }
}

// ---------------------------------------------------------------------------
// Bevy spawn systems
// ---------------------------------------------------------------------------

/// Spawns a single chunk entity with ONE combined mesh and ONE draw call.
/// Uses vertex colors for solid-color blocks (no textures).
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
) -> Entity {
    let (positions, colors, normals, indices) = generate_chunk_mesh(&chunk);

    let mesh = meshes.add(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(indices)),
    );

    let mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        ..default()
    });

    commands
        .spawn((
            chunk,
            Mesh3d(mesh),
            MeshMaterial3d(mat),
            Transform::from_translation(position),
            Visibility::default(),
        ))
        .id()
}

/// Creates the initial chunk and spawns it at world origin.
pub fn spawn_initial_chunks(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let mut chunk = Chunk::filled(0); // start with air
    fill_terrain(&mut chunk);

    spawn_chunk_entity(
        &mut commands,
        &mut materials,
        &mut meshes,
        chunk,
        Vec3::ZERO,
    );

    // Camera starts above the chunk.
    use crate::camera::CameraController;
    let camera_transform = Transform::from_xyz(16.0, 20.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
        ))
        .id();

    // Create HUD tied to this camera entity
    crate::hud::setup_hud(&mut commands, camera_entity);
}

// ---------------------------------------------------------------------------
// Chunk coordinate system
// ---------------------------------------------------------------------------

/// World-space position of a block (block-centered coordinates).
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    /// Convert world-space coordinates to block position.
    pub fn from_world(world: Vec3) -> Self {
        Self {
            x: world.x.floor() as i32,
            y: world.y.floor() as i32,
            z: world.z.floor() as i32,
        }
    }
}

/// Chunk coordinate (each chunk covers a 32³ volume).
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkCoord {
    /// Convert world-space coordinates to the chunk that contains them.
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            cx: (world_pos.x / CHUNK_SIZE as f32).floor() as i32,
            cy: (world_pos.y / CHUNK_SIZE as f32).floor() as i32,
            cz: (world_pos.z / CHUNK_SIZE as f32).floor() as i32,
        }
    }

    /// Convert chunk coordinate to world-space origin of that chunk.
    pub fn to_world_origin(self) -> Vec3 {
        Vec3::new(
            self.cx as f32 * CHUNK_SIZE as f32,
            self.cy as f32 * CHUNK_SIZE as f32,
            self.cz as f32 * CHUNK_SIZE as f32,
        )
    }
}

/// Converts a world-space block position to (chunk_coord, local_block_index).
/// Returns None if the position is in an unloaded chunk.
pub fn world_to_chunk(local_pos: BlockPos) -> Option<(ChunkCoord, usize)> {
    let cx = local_pos.x.div_euclid(CHUNK_SIZE as i32);
    let cy = local_pos.y.div_euclid(CHUNK_SIZE as i32);
    let cz = local_pos.z.div_euclid(CHUNK_SIZE as i32);

    let lx = local_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = local_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = local_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

    Some((
        ChunkCoord { cx, cy, cz },
        lz * CHUNK_SIZE * CHUNK_SIZE + ly * CHUNK_SIZE + lx,
    ))
}

/// Mark a block as dirty and collect all chunks that need mesh rebuild.
///
/// When a block changes, the chunk it belongs to always needs rebuild.
/// Additionally, if the block is on a chunk boundary, each neighboring
/// chunk that shares that boundary may have exposed faces that changed
/// and must also rebuild.
pub fn mark_block_dirty(
    coord: ChunkCoord,
    local_pos: (usize, usize, usize),
    dirty_chunks: &mut Vec<ChunkCoord>,
) {
    dirty_chunks.push(coord);

    let (x, y, z) = local_pos;
    if x == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx - 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if x == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx + 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if y == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy - 1,
            cz: coord.cz,
        });
    }
    if y == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy + 1,
            cz: coord.cz,
        });
    }
    if z == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz - 1,
        });
    }
    if z == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz + 1,
        });
    }
}
