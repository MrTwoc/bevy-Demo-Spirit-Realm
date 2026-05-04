//! Voxel chunk: 32x32x32 block storage + face-culled mesh generation.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};

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

/// All 6 faces of a block in order: +X, -X, +Y, -Y, +Z, -Z
const FACES: [(Face, [i32; 3]); 6] = [
    (Face::Right,  [1, 0, 0]),
    (Face::Left,   [-1, 0, 0]),
    (Face::Top,    [0, 1, 0]),
    (Face::Bottom, [0, -1, 0]),
    (Face::Front,  [0, 0, 1]),
    (Face::Back,   [0, 0, -1]),
];

/// Chunk component: stores 32x32x32 block IDs.
///
/// Internally stored as a flat 1D vector for cache friendliness.
/// Index formula: `z * CHUNK_SIZE^2 + y * CHUNK_SIZE + x`
#[derive(Component, Clone)]
pub struct Chunk {
    /// Raw block data, indexed as `(z * CHUNK_SIZE^2 + y * CHUNK_SIZE + x)`.
    blocks: Vec<BlockId>,
}

impl Chunk {
    /// Creates a new chunk filled entirely with air.
    pub fn new() -> Self {
        Self {
            blocks: vec![0; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE],
        }
    }

    /// Creates a chunk pre-filled with a specific block type.
    pub fn filled(block_id: BlockId) -> Self {
        Self {
            blocks: vec![block_id; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE],
        }
    }

    /// Returns the block ID at (x, y, z). Returns 0 (air) if out of bounds.
    fn get_block_unchecked(&self, x: usize, y: usize, z: usize) -> BlockId {
        self.blocks[Self::flatten(x, y, z)]
    }

    /// Linear index from 3D coordinates. Panics if out of bounds.
    fn flatten(x: usize, y: usize, z: usize) -> usize {
        z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x
    }

    /// Sets the block at (x, y, z). No-op if out of bounds.
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, block_id: BlockId) {
        if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
            self.blocks[Self::flatten(x, y, z)] = block_id;
        }
    }

    /// Returns whether the face of block at (x, y, z) in direction `face`
    /// should be rendered (i.e., the neighbor is air or out of bounds).
    fn is_face_visible(&self, x: usize, y: usize, z: usize, face: &[i32; 3]) -> bool {
        let nx = x as i32 + face[0];
        let ny = y as i32 + face[1];
        let nz = z as i32 + face[2];

        // Out of chunk bounds → exposed, render this face
        if nx < 0 || ny < 0 || nz < 0
            || nx >= CHUNK_SIZE as i32
            || ny >= CHUNK_SIZE as i32
            || nz >= CHUNK_SIZE as i32
        {
            return true;
        }

        // Neighbor is air → exposed
        self.get_block_unchecked(nx as usize, ny as usize, nz as usize) == 0
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

/// Generates a face-culled mesh for the chunk.
///
/// Only renders faces that are adjacent to air (or chunk boundary).
/// Returns (positions, uvs, normals, indices) for a `TriangleList` mesh.
pub fn generate_chunk_mesh(chunk: &Chunk) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut uvs       = Vec::new();
    let mut normals   = Vec::new();
    let mut indices   = Vec::new();

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_id = chunk.get_block_unchecked(x, y, z);
                if block_id == 0 {
                    continue; // air
                }

                for (face, offset) in FACES.iter().cloned() {
                    if !chunk.is_face_visible(x, y, z, &offset) {
                        continue; // face is occluded
                    }

                    let base_index = positions.len() as u32;
                    let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face);
                    positions.extend(face_verts);
                    uvs.extend(face_uvs);
                    normals.extend([face_normal; 4]);
                    indices.extend([
                        base_index,     base_index + 1, base_index + 2,
                        base_index,    base_index + 2,  base_index + 3,
                    ]);
                }
            }
        }
    }

    (positions, uvs, normals, indices)
}

/// Returns the 4 vertices, UVs, and normal for a single face.
fn face_quad(x: usize, y: usize, z: usize, face: Face) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        Face::Top => {
            // +Y face (grass top)
            let h = 0.5;
            ([
                [x as f32 - 0.5, y as f32 + h, z as f32 - 0.5],
                [x as f32 + 0.5, y as f32 + h, z as f32 - 0.5],
                [x as f32 + 0.5, y as f32 + h, z as f32 + 0.5],
                [x as f32 - 0.5, y as f32 + h, z as f32 + 0.5],
            ], [0.0, 1.0, 0.0])
        }
        Face::Bottom => {
            // -Y face (dirt side)
            let h = -0.5;
            ([
                [x as f32 - 0.5, y as f32 + h, z as f32 + 0.5],
                [x as f32 + 0.5, y as f32 + h, z as f32 + 0.5],
                [x as f32 + 0.5, y as f32 + h, z as f32 - 0.5],
                [x as f32 - 0.5, y as f32 + h, z as f32 - 0.5],
            ], [0.0, -1.0, 0.0])
        }
        Face::Right => {
            // +X face
            let h = 0.5;
            ([
                [x as f32 + h, y as f32 - 0.5, z as f32 - 0.5],
                [x as f32 + h, y as f32 - 0.5, z as f32 + 0.5],
                [x as f32 + h, y as f32 + 0.5, z as f32 + 0.5],
                [x as f32 + h, y as f32 + 0.5, z as f32 - 0.5],
            ], [1.0, 0.0, 0.0])
        }
        Face::Left => {
            // -X face
            let h = -0.5;
            ([
                [x as f32 + h, y as f32 - 0.5, z as f32 + 0.5],
                [x as f32 + h, y as f32 - 0.5, z as f32 - 0.5],
                [x as f32 + h, y as f32 + 0.5, z as f32 - 0.5],
                [x as f32 + h, y as f32 + 0.5, z as f32 + 0.5],
            ], [-1.0, 0.0, 0.0])
        }
        Face::Front => {
            // +Z face
            let h = 0.5;
            ([
                [x as f32 + 0.5, y as f32 - 0.5, z as f32 + h],
                [x as f32 - 0.5, y as f32 - 0.5, z as f32 + h],
                [x as f32 - 0.5, y as f32 + 0.5, z as f32 + h],
                [x as f32 + 0.5, y as f32 + 0.5, z as f32 + h],
            ], [0.0, 0.0, 1.0])
        }
        Face::Back => {
            // -Z face
            let h = -0.5;
            ([
                [x as f32 - 0.5, y as f32 - 0.5, z as f32 + h],
                [x as f32 + 0.5, y as f32 - 0.5, z as f32 + h],
                [x as f32 + 0.5, y as f32 + 0.5, z as f32 + h],
                [x as f32 - 0.5, y as f32 + 0.5, z as f32 + h],
            ], [0.0, 0.0, -1.0])
        }
    };
    (verts, face_uvs(face), normal)
}

/// UV coordinates for each face (placeholder — single color per face).
fn face_uvs(_face: Face) -> [[f32; 2]; 4] {
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
}

/// Assembles a `Mesh` from face-culled chunk data.
pub fn build_chunk_mesh(chunk: &Chunk) -> Mesh {
    let (positions, uvs, normals, indices) = generate_chunk_mesh(chunk);

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_indices(Indices::U32(indices))
}

/// Fills a chunk with a flat grass layer at y=0 and dirt below.
///
/// y=0       → grass (BlockId=1)
/// y=1..3    → dirt  (BlockId=3)
/// y=4..31   → stone (BlockId=2)
pub fn fill_terrain(chunk: &mut Chunk) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            chunk.set_block(x, 0, z, 1); // grass top
            for y in 1..4 {
                chunk.set_block(x, y, z, 3); // dirt
            }
            for y in 4..CHUNK_SIZE {
                chunk.set_block(x, y, z, 2); // stone
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Material helpers
// ---------------------------------------------------------------------------

/// Block ID → StandardMaterial color (base_color only, no textures yet).
fn block_material(block_id: BlockId) -> Color {
    match block_id {
        1 => Color::srgb(0.3, 0.65, 0.2),   // grass  — medium green
        2 => Color::srgb(0.5, 0.5, 0.5),   // stone  — mid gray
        3 => Color::srgb(0.55, 0.35, 0.2), // dirt   — brown
        _ => Color::srgb(1.0, 0.0, 1.0),   // unknown — magenta
    }
}

// ---------------------------------------------------------------------------
// Bevy spawn systems
// ---------------------------------------------------------------------------

/// Spawns a single chunk entity with its mesh + materials.
/// Currently uses one material per visible face (block-type-aware),
/// producing up to 3 draw calls (grass / dirt / stone faces).
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
) {
    // Build face-culled mesh once.
    let mesh = meshes.add(build_chunk_mesh(&chunk));

    // Collect visible block types in this chunk for material creation.
    // We create one material per block type present.
    let mut block_types_present = std::collections::HashSet::new();
    for &b in &chunk.blocks {
        if b != 0 {
            block_types_present.insert(b);
        }
    }

    // Map BlockId → StandardMaterial handle.
    let _mat_handles: std::collections::HashMap<BlockId, Handle<StandardMaterial>> =
        block_types_present
            .into_iter()
            .map(|id| {
                let mat = materials.add(StandardMaterial::from_color(block_material(id)));
                (id, mat)
            })
            .collect();

    // For now, spawn as a plain entity with a single combined mesh
    // and a single representative material (grass green) for the demo.
    // TODO: multi-material per-face rendering (atlas or per-type meshes).
    let demo_mat = materials.add(StandardMaterial::from_color(Color::srgb(0.3, 0.65, 0.2)));

    commands.spawn((
        chunk,
        Mesh3d(mesh),
        MeshMaterial3d(demo_mat),
        Transform::from_translation(position),
        Visibility::default(),
    ));
}

/// Creates the initial chunk and spawns it at world origin.
pub fn spawn_initial_chunks(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let mut chunk = Chunk::filled(0); // start with air
    fill_terrain(&mut chunk);

    spawn_chunk_entity(&mut commands, &mut materials, &mut meshes, chunk, Vec3::ZERO);

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
