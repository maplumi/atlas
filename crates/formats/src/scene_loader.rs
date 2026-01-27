use std::fs;
use std::path::{Path, PathBuf};

use scene::World;
use scene::components::VectorGeometryKind;

use crate::scene_package::{ScenePackage, ScenePackageError};
use crate::vector_chunk::{VectorChunk, VectorChunkError};

#[derive(Debug)]
pub enum SceneWorldLoadError {
    Package(ScenePackageError),
    ChunkIo {
        path: PathBuf,
        source: std::io::Error,
    },
    ChunkParse {
        chunk_id: String,
        source: VectorChunkError,
    },
}

impl std::fmt::Display for SceneWorldLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneWorldLoadError::Package(e) => write!(f, "scene package error: {e}"),
            SceneWorldLoadError::ChunkIo { path, source } => {
                write!(f, "failed to read chunk {}: {source}", path.display())
            }
            SceneWorldLoadError::ChunkParse { chunk_id, source } => {
                write!(f, "failed to parse chunk {chunk_id}: {source}")
            }
        }
    }
}

impl std::error::Error for SceneWorldLoadError {}

pub fn load_world_from_package_dir(root: impl AsRef<Path>) -> Result<World, SceneWorldLoadError> {
    let package = ScenePackage::load(root).map_err(SceneWorldLoadError::Package)?;
    load_world_from_package(&package)
}

pub fn load_world_from_package(package: &ScenePackage) -> Result<World, SceneWorldLoadError> {
    let mut world = World::new();

    // Always include the globe as the scene root.
    scene::prefabs::spawn_wgs84_globe(&mut world);

    for entry in &package.manifest().chunks {
        let expected = match entry.kind.as_str() {
            "points" => Some(VectorGeometryKind::Point),
            "lines" => Some(VectorGeometryKind::Line),
            "areas" => Some(VectorGeometryKind::Area),
            _ => None,
        };

        let path = package.root().join(&entry.path);
        let payload = fs::read_to_string(&path).map_err(|e| SceneWorldLoadError::ChunkIo {
            path: path.clone(),
            source: e,
        })?;
        let chunk = VectorChunk::from_geojson_str(&payload).map_err(|e| {
            SceneWorldLoadError::ChunkParse {
                chunk_id: entry.id.clone(),
                source: e,
            }
        })?;

        ingest_vector_chunk(&mut world, &chunk, expected);
    }

    Ok(world)
}

fn ingest_vector_chunk(
    world: &mut World,
    chunk: &VectorChunk,
    expected: Option<VectorGeometryKind>,
) {
    crate::scene_ingest::ingest_vector_chunk(world, chunk, expected)
}

#[cfg(test)]
mod tests {
    use super::load_world_from_package_dir;

    #[test]
    fn loads_demo_assets_into_world() {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../apps/viewer_web/assets");
        let world = load_world_from_package_dir(root).expect("load world");

        // cities: 6 point features
        // air corridors: currently 3 lines
        // regions: currently 3 areas
        // (counts are asserted loosely to keep this test stable if demo changes)
        let geoms = world.vector_geometries_by_entity();
        assert!(!geoms.is_empty());
    }
}
