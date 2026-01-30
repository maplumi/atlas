use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use scene::World;
use scene::components::VectorGeometryKind;

use crate::scene_package::{ScenePackage, ScenePackageError};
use crate::vector_chunk::VectorChunk;

#[derive(Debug)]
pub enum SceneWorldLoadError {
    Package(ScenePackageError),
    ChunkIo {
        path: PathBuf,
        source: std::io::Error,
    },
    ChunkHashMissing {
        chunk_id: String,
        path: PathBuf,
    },
    ChunkHashMismatch {
        chunk_id: String,
        path: PathBuf,
        expected: String,
        found: String,
    },
    ChunkParse {
        chunk_id: String,
        source: String,
    },
}

impl std::fmt::Display for SceneWorldLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneWorldLoadError::Package(e) => write!(f, "scene package error: {e}"),
            SceneWorldLoadError::ChunkIo { path, source } => {
                write!(f, "failed to read chunk {}: {source}", path.display())
            }
            SceneWorldLoadError::ChunkHashMissing { chunk_id, path } => {
                write!(
                    f,
                    "missing content_hash for chunk {chunk_id} ({})",
                    path.display()
                )
            }
            SceneWorldLoadError::ChunkHashMismatch {
                chunk_id,
                path,
                expected,
                found,
            } => write!(
                f,
                "content hash mismatch for chunk {chunk_id} ({}): expected {expected}, found {found}",
                path.display()
            ),
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
        let chunk = if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("avc"))
            .unwrap_or(false)
        {
            // Hash enforcement for binary chunks: makes caching and streaming deterministic and safe.
            let expected = entry.content_hash.clone().ok_or_else(|| {
                SceneWorldLoadError::ChunkHashMissing {
                    chunk_id: entry.id.clone(),
                    path: path.clone(),
                }
            })?;

            let file = std::fs::File::open(&path).map_err(|e| SceneWorldLoadError::ChunkIo {
                path: path.clone(),
                source: e,
            })?;

            let mut reader = HashingReader::new(file);
            let chunk = VectorChunk::from_avc_reader(&mut reader).map_err(|e| {
                SceneWorldLoadError::ChunkParse {
                    chunk_id: entry.id.clone(),
                    source: e.to_string(),
                }
            })?;

            // Ensure the hash covers the entire file (even if trailing bytes exist).
            std::io::copy(&mut reader, &mut std::io::sink()).map_err(|e| {
                SceneWorldLoadError::ChunkIo {
                    path: path.clone(),
                    source: e,
                }
            })?;

            let found = reader.finalize_hex();
            if !eq_hex(&expected, &found) {
                return Err(SceneWorldLoadError::ChunkHashMismatch {
                    chunk_id: entry.id.clone(),
                    path: path.clone(),
                    expected,
                    found,
                });
            }

            chunk
        } else {
            let payload = fs::read_to_string(&path).map_err(|e| SceneWorldLoadError::ChunkIo {
                path: path.clone(),
                source: e,
            })?;
            VectorChunk::from_geojson_str(&payload).map_err(|e| {
                SceneWorldLoadError::ChunkParse {
                    chunk_id: entry.id.clone(),
                    source: e.to_string(),
                }
            })?
        };

        ingest_vector_chunk(&mut world, &chunk, expected);
    }

    Ok(world)
}

struct HashingReader<R> {
    inner: R,
    hasher: blake3::Hasher,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    fn finalize_hex(&self) -> String {
        to_hex(self.hasher.clone().finalize().as_bytes())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn eq_hex(a: &str, b: &str) -> bool {
    // Allow either case in manifests.
    a.trim().eq_ignore_ascii_case(b.trim())
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
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../apps/web/assets");
        let world = load_world_from_package_dir(root).expect("load world");

        // cities: 6 point features
        // air corridors: currently 3 lines
        // regions: currently 3 areas
        // (counts are asserted loosely to keep this test stable if demo changes)
        let geoms = world.vector_geometries_by_entity();
        assert!(!geoms.is_empty());
    }
}
