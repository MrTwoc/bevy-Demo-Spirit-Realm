//! Voxel chunk: 32x32x32 block storage + face-culled mesh generation.

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use std::hash::Hash;

use crate::atlas::{self, dirt, grass, stone};

/// Size of one dimension of a chunk (32³ blocks per chunk).
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

/// Block type + face direction → which atlas slot to use.
#[derive(Clone, Copy)]
enum BlockTexture {
    GrassTop,
    GrassSide,
    GrassBottom, // dirt texture
    DirtTop,
    DirtSide,
    Stone,
}

impl BlockTexture {
    fn from_block_and_face(block_id: BlockId, face: Face) -> Self {
        match block_id {
            1 => match face {
                // grass
                Face::Top => BlockTexture::GrassTop,
                Face::Bottom => BlockTexture::GrassBottom,
                _ => BlockTexture::GrassSide,
            },
            2 => BlockTexture::Stone, // stone (all faces same)
            3 => match face {
                // dirt
                Face::Top | Face::Bottom => BlockTexture::DirtTop,
                _ => BlockTexture::DirtSide,
            },
            _ => BlockTexture::Stone,
        }
    }

    fn atlas_slot(&self) -> atlas::AtlasSlot {
        match self {
            BlockTexture::GrassTop => grass::TOP,
            BlockTexture::GrassSide => grass::RIGHT,
            BlockTexture::GrassBottom => dirt::TOP,
            BlockTexture::DirtTop => dirt::TOP,
            BlockTexture::DirtSide => dirt::RIGHT,
            BlockTexture::Stone => stone::TOP,
        }
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
/// Returns (positions, uvs, normals, indices).
///
/// UVs are computed from a texture atlas based on block type and face direction.
pub fn generate_chunk_mesh(
    chunk: &Chunk,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
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
                    let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face, block_id);
                    positions.extend(face_verts);
                    uvs.extend(face_uvs);
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

    (positions, uvs, normals, indices)
}

/// Returns the 4 vertices, UVs, and normal for a single face.
fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    block_id: BlockId,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        Face::Top => {
            // +Y face (grass top)
            let h = 0.5;
            (
                [
                    [x as f32 - 0.5, y as f32 + h, z as f32 - 0.5],
                    [x as f32 + 0.5, y as f32 + h, z as f32 - 0.5],
                    [x as f32 + 0.5, y as f32 + h, z as f32 + 0.5],
                    [x as f32 - 0.5, y as f32 + h, z as f32 + 0.5],
                ],
                [0.0, 1.0, 0.0],
            )
        }
        Face::Bottom => {
            // -Y face (dirt side)
            let h = -0.5;
            (
                [
                    [x as f32 - 0.5, y as f32 + h, z as f32 + 0.5],
                    [x as f32 + 0.5, y as f32 + h, z as f32 + 0.5],
                    [x as f32 + 0.5, y as f32 + h, z as f32 - 0.5],
                    [x as f32 - 0.5, y as f32 + h, z as f32 - 0.5],
                ],
                [0.0, -1.0, 0.0],
            )
        }
        Face::Right => {
            // +X face
            let h = 0.5;
            (
                [
                    [x as f32 + h, y as f32 - 0.5, z as f32 - 0.5],
                    [x as f32 + h, y as f32 - 0.5, z as f32 + 0.5],
                    [x as f32 + h, y as f32 + 0.5, z as f32 + 0.5],
                    [x as f32 + h, y as f32 + 0.5, z as f32 - 0.5],
                ],
                [1.0, 0.0, 0.0],
            )
        }
        Face::Left => {
            // -X face
            let h = -0.5;
            (
                [
                    [x as f32 + h, y as f32 - 0.5, z as f32 + 0.5],
                    [x as f32 + h, y as f32 - 0.5, z as f32 - 0.5],
                    [x as f32 + h, y as f32 + 0.5, z as f32 - 0.5],
                    [x as f32 + h, y as f32 + 0.5, z as f32 + 0.5],
                ],
                [-1.0, 0.0, 0.0],
            )
        }
        Face::Front => {
            // +Z face
            let h = 0.5;
            (
                [
                    [x as f32 + 0.5, y as f32 - 0.5, z as f32 + h],
                    [x as f32 - 0.5, y as f32 - 0.5, z as f32 + h],
                    [x as f32 - 0.5, y as f32 + 0.5, z as f32 + h],
                    [x as f32 + 0.5, y as f32 + 0.5, z as f32 + h],
                ],
                [0.0, 0.0, 1.0],
            )
        }
        Face::Back => {
            // -Z face
            let h = -0.5;
            (
                [
                    [x as f32 - 0.5, y as f32 - 0.5, z as f32 + h],
                    [x as f32 + 0.5, y as f32 - 0.5, z as f32 + h],
                    [x as f32 + 0.5, y as f32 + 0.5, z as f32 + h],
                    [x as f32 - 0.5, y as f32 + 0.5, z as f32 + h],
                ],
                [0.0, 0.0, -1.0],
            )
        }
    };

    // Compute atlas UV for this block type + face direction
    let tex = BlockTexture::from_block_and_face(block_id, face);
    let (u0, u1, v0, v1) = {
        let slot = tex.atlas_slot();
        let (u0, u1, v0, v1) = slot.uv();
        (u0, u1, v0, v1)
    };

    // UV vertex order must match position vertex order per face.
    // Position vertices are ordered: v0=bottom-left, v1=bottom-right, v2=top-right, v3=top-left (viewed from outside).
    let face_uvs: [[f32; 2]; 4] = match face {
        Face::Top | Face::Bottom | Face::Right | Face::Front => [
            [u0, v0], // bottom-left
            [u1, v0], // bottom-right
            [u1, v1], // top-right
            [u0, v1], // top-left
        ],
        Face::Left | Face::Back => [
            [u1, v0], // bottom-right (mirrored)
            [u0, v0], // bottom-left
            [u0, v1], // top-left
            [u1, v1], // top-right
        ],
    };

    (verts, face_uvs, normal)
}

// --------------------------------------------------------------------------
// Terrain helpers
// --------------------------------------------------------------------------

/// Fills a chunk with only the bottom 3 layers.
/// y=0 → stone  (BlockId=2, bottom / deepest)
/// y=1 → dirt   (BlockId=3, middle)
/// y=2 → grass  (BlockId=1, top / surface)
/// y>=3 → air   (BlockId=0)
pub fn fill_terrain(chunk: &mut Chunk) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            chunk.set(x, 0, z, 2); // stone  — bottom
            chunk.set(x, 1, z, 3); // dirt   — middle
            chunk.set(x, 2, z, 1); // grass  — top surface
            // y >= 3: air (implicit, chunk is zero-initialized)
        }
    }
}

// ---------------------------------------------------------------------------
// Bevy spawn systems
// ---------------------------------------------------------------------------

/// Spawns a single chunk entity with ONE combined mesh and ONE draw call.
/// Spawns a single chunk entity with ONE combined mesh and ONE draw call.
/// Uses a texture atlas for per-face UV mapping.
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
    texture_handle: Handle<Image>,
) {
    let (positions, uvs, normals, indices) = generate_chunk_mesh(&chunk);

    let mesh = meshes.add(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(indices)),
    );

    let mat = materials.add(StandardMaterial {
        base_color_texture: Some(texture_handle),
        ..default()
    });

    commands.spawn((
        chunk,
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(position),
        Visibility::default(),
    ));
}

/// Creates the initial chunk and spawns it at world origin.
pub fn spawn_initial_chunks(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    asset_server: Res<AssetServer>,
) {
    let mut chunk = Chunk::filled(0); // start with air
    fill_terrain(&mut chunk);

    let texture_handle = asset_server.load("textures/array_texture.png");

    spawn_chunk_entity(
        &mut commands,
        &mut materials,
        &mut meshes,
        chunk,
        Vec3::ZERO,
        texture_handle,
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
    crate::hud::setup_hud(commands, camera_entity);
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
