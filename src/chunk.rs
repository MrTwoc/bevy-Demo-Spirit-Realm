//! Voxel chunk: block storage + face-culled mesh generation.
//!
//! TODO(P0): 当前使用 32³ 正方体区块，后期需迁移到 16×32×16 SubChunk。

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use std::hash::Hash;

use crate::chunk_dirty::ChunkMeshHandle;
use crate::resource_pack::ResourcePackManager;

/// Size of one dimension of a chunk (32³ blocks per chunk).
pub const CHUNK_SIZE: usize = 32;

/// A single block type identifier.
/// 0 = air (not rendered), 1 = grass, 2 = stone, 3 = dirt, 4 = sand.
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

impl Face {
    /// 将 Face 转换为资源包映射表中的面名称
    pub fn to_face_name(&self) -> &'static str {
        match self {
            Face::Top => "top",
            Face::Bottom => "bottom",
            _ => "side", // Right, Left, Front, Back 都用 "side"
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

/// 6 个方向的邻居区块数据，用于跨区块面剔除。
///
/// 索引顺序与 FACES 一致：[+X, -X, +Y, -Y, +Z, -Z]。
/// 如果某个方向没有邻居（未加载），对应位置为 `None`，
/// 面剔除时会将缺失的邻居视为空气（即保留该面）。
pub struct ChunkNeighbors {
    /// 6 个方向的邻居区块数据引用
    pub neighbors: [Option<BlockId>; 6],
    /// 6 个方向的邻居完整数据（用于跨边界查询）
    pub neighbor_data: [Option<Vec<BlockId>>; 6],
}

impl ChunkNeighbors {
    /// 创建空的邻居数据（所有方向都没有邻居）
    pub fn empty() -> Self {
        Self {
            neighbors: [None; 6],
            neighbor_data: std::array::from_fn(|_| None),
        }
    }

    /// 获取指定方向邻居在 (x, y, z) 位置的方块 ID。
    /// 如果邻居不存在，返回 0（空气）。
    pub fn get_neighbor_block(&self, face_index: usize, x: usize, y: usize, z: usize) -> BlockId {
        if let Some(ref data) = self.neighbor_data[face_index] {
            if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
                let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
                data[idx]
            } else {
                0
            }
        } else {
            0 // 没有邻居数据，视为空气
        }
    }
}

/// Chunk data: three-state storage for a 32x32x32 voxel chunk.
#[derive(Component, Clone)]
pub enum ChunkData {
    Empty,
    Uniform(BlockId),
    Mixed(Vec<BlockId>),
}

impl ChunkData {
    pub fn new() -> Self {
        Self::Empty
    }

    pub fn filled(block_id: BlockId) -> Self {
        Self::Uniform(block_id)
    }

    fn flatten(x: usize, y: usize, z: usize) -> usize {
        z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x
    }

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

    /// 将 ChunkData 转换为 Vec<BlockId>（用于传递给邻居查询）
    pub fn to_vec(&self) -> Vec<BlockId> {
        match self {
            ChunkData::Empty => vec![0; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE],
            ChunkData::Uniform(id) => vec![*id; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE],
            ChunkData::Mixed(data) => data.clone(),
        }
    }

    /// 判断指定面是否可见（需要渲染）。
    ///
    /// 当邻居在区块边界内时，直接查询本区块数据。
    /// 当邻居在区块边界外时，通过 `neighbors` 查询邻居区块数据。
    pub fn is_face_visible(
        &self,
        x: usize,
        y: usize,
        z: usize,
        face: &[i32; 3],
        face_index: usize,
        neighbors: &ChunkNeighbors,
    ) -> bool {
        let nx = x as i32 + face[0];
        let ny = y as i32 + face[1];
        let nz = z as i32 + face[2];

        let current_id = self.get(x, y, z);

        // 邻居在区块边界内，直接查询本区块
        if nx >= 0
            && ny >= 0
            && nz >= 0
            && nx < CHUNK_SIZE as i32
            && ny < CHUNK_SIZE as i32
            && nz < CHUNK_SIZE as i32
        {
            return self.get(nx as usize, ny as usize, nz as usize) != current_id;
        }

        // 邻居在区块边界外，查询邻居区块数据
        let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

        let neighbor_id =
            neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z);
        neighbor_id != current_id
    }
}

impl Default for ChunkData {
    fn default() -> Self {
        Self::Empty
    }
}

pub type Chunk = ChunkData;

/// Generates a face-culled mesh for the chunk.
/// Returns (positions, uvs, normals, indices).
/// UV 坐标从 ResourcePackManager 的动态 Atlas 中查找。
///
/// `neighbors` 提供 6 个方向的邻居区块数据，用于跨区块面剔除。
/// 当邻居不存在时，边界面上的方块面会被保留（视为空气）。
pub fn generate_chunk_mesh(
    chunk: &Chunk,
    resource_pack: &ResourcePackManager,
    neighbors: &ChunkNeighbors,
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
                    continue;
                }

                for (face_index, (face, offset)) in FACES.iter().cloned().enumerate() {
                    if !chunk.is_face_visible(x, y, z, &offset, face_index, neighbors) {
                        continue;
                    }

                    let base_index = positions.len() as u32;
                    let face_name = face.to_face_name();

                    // 从资源包查找 UV 坐标
                    let uv = resource_pack
                        .get_block_uv(block_id, face_name)
                        .unwrap_or((0.0, 1.0, 0.0, 1.0));

                    let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face, uv);
                    positions.extend(face_verts);
                    uvs.extend(face_uvs);
                    normals.extend([face_normal; 4]);
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
    uv: (f32, f32, f32, f32),
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        Face::Top => (
            [
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        Face::Bottom => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32, y as f32, z as f32],
            ],
            [0.0, -1.0, 0.0],
        ),
        Face::Right => (
            [
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
            ],
            [1.0, 0.0, 0.0],
        ),
        Face::Left => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        Face::Front => (
            [
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        Face::Back => (
            [
                [x as f32, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let (u_min, u_max, v_min, v_max) = uv;
    let face_uvs = [
        [u_min, v_max],
        [u_max, v_max],
        [u_max, v_min],
        [u_min, v_min],
    ];

    (verts, face_uvs, normal)
}

// --------------------------------------------------------------------------
// Terrain helpers
// --------------------------------------------------------------------------

/// Fills a chunk with terrain layers.
pub fn fill_terrain(chunk: &mut Chunk) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            chunk.set(x, 0, z, 2); // stone
            chunk.set(x, 1, z, 3); // dirt

            let hash = (x as u32).wrapping_mul(73856093) ^ (z as u32).wrapping_mul(19349663);
            let top_block = match hash % 3 {
                0 => 1, // grass
                1 => 4, // sand
                _ => 2, // stone
            };
            chunk.set(x, 2, z, top_block);
        }
    }
}

// ---------------------------------------------------------------------------
// Bevy spawn systems
// ---------------------------------------------------------------------------

/// Spawns a single chunk entity with texture-mapped mesh.
///
/// 返回 Entity ID。mesh 和 material handle 通过 `ChunkMeshHandle` 组件存储在实体上，
/// 用于后续重建时移除旧资源，避免 GPU 内存泄漏。
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
    resource_pack: &ResourcePackManager,
    atlas_texture: &Handle<Image>,
    neighbors: &ChunkNeighbors,
) -> Entity {
    let (positions, uvs, normals, indices) = generate_chunk_mesh(&chunk, resource_pack, neighbors);

    let mesh_handle = meshes.add(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(indices)),
    );

    let mat_handle = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(atlas_texture.clone()),
        ..default()
    });

    commands
        .spawn((
            chunk,
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(mat_handle.clone()),
            ChunkMeshHandle {
                mesh: mesh_handle,
                material: mat_handle,
            },
            Transform::from_translation(position),
            Visibility::default(),
        ))
        .id()
}

// ---------------------------------------------------------------------------
// Chunk coordinate system
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn from_world(world: Vec3) -> Self {
        Self {
            x: world.x.floor() as i32,
            y: world.y.floor() as i32,
            z: world.z.floor() as i32,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkCoord {
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            cx: (world_pos.x / CHUNK_SIZE as f32).floor() as i32,
            cy: (world_pos.y / CHUNK_SIZE as f32).floor() as i32,
            cz: (world_pos.z / CHUNK_SIZE as f32).floor() as i32,
        }
    }

    pub fn to_world_origin(self) -> Vec3 {
        Vec3::new(
            self.cx as f32 * CHUNK_SIZE as f32,
            self.cy as f32 * CHUNK_SIZE as f32,
            self.cz as f32 * CHUNK_SIZE as f32,
        )
    }
}

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
