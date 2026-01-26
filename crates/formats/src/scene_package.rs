use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{MANIFEST_VERSION, SceneManifest};

pub const MANIFEST_FILE_NAME: &str = "scene.manifest.json";

#[derive(Debug, Clone)]
pub struct ScenePackage {
    root: PathBuf,
    manifest: SceneManifest,
}

#[derive(Debug)]
pub enum ScenePackageError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    UnsupportedVersion { found: String },
}

impl fmt::Display for ScenePackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenePackageError::Io(err) => write!(f, "I/O error: {err}"),
            ScenePackageError::Parse(err) => write!(f, "Manifest parse error: {err}"),
            ScenePackageError::UnsupportedVersion { found } => {
                write!(f, "Unsupported manifest version: {found}")
            }
        }
    }
}

impl std::error::Error for ScenePackageError {}

impl ScenePackage {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, ScenePackageError> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join(MANIFEST_FILE_NAME);
        let payload = fs::read_to_string(&manifest_path).map_err(ScenePackageError::Io)?;
        let manifest: SceneManifest =
            serde_json::from_str(&payload).map_err(ScenePackageError::Parse)?;

        if manifest.version != MANIFEST_VERSION {
            return Err(ScenePackageError::UnsupportedVersion {
                found: manifest.version,
            });
        }

        Ok(Self { root, manifest })
    }

    pub fn manifest(&self) -> &SceneManifest {
        &self.manifest
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::{MANIFEST_FILE_NAME, ScenePackage, ScenePackageError};
    use crate::manifest::{ChunkEntry, MANIFEST_VERSION, SceneManifest};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let id = format!("atlas_scene_package_{label}_{}", std::process::id());
        dir.push(id);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn load_scene_package_manifest() {
        let root = temp_dir("load");
        let mut manifest = SceneManifest::new("demo-package");
        manifest.name = Some("Demo".to_string());
        manifest.chunks.push(ChunkEntry {
            id: "chunk-1".to_string(),
            kind: "terrain".to_string(),
            path: "chunks/terrain-1.bin".to_string(),
        });

        let payload = serde_json::to_string_pretty(&manifest).expect("serialize manifest");
        fs::write(root.join(MANIFEST_FILE_NAME), payload).expect("write manifest");

        let package = ScenePackage::load(&root).expect("load package");
        assert_eq!(package.root(), root.as_path());
        assert_eq!(package.manifest(), &manifest);
    }

    #[test]
    fn rejects_unsupported_manifest_version() {
        let root = temp_dir("version");
        let mut manifest = SceneManifest::new("demo-package");
        manifest.version = "2.0".to_string();

        let payload = serde_json::to_string_pretty(&manifest).expect("serialize manifest");
        fs::write(root.join(MANIFEST_FILE_NAME), payload).expect("write manifest");

        let err = ScenePackage::load(&root).expect_err("expect version error");
        match err {
            ScenePackageError::UnsupportedVersion { found } => {
                assert_eq!(found, "2.0");
                assert_ne!(found, MANIFEST_VERSION);
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
