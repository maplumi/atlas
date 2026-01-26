use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneManifest {
    pub version: String,
    pub package_id: String,
    pub name: Option<String>,
    pub chunks: Vec<ChunkEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkEntry {
    pub id: String,
    pub kind: String,
    pub path: String,
}

impl SceneManifest {
    pub fn new(package_id: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION.to_string(),
            package_id: package_id.into(),
            name: None,
            chunks: Vec::new(),
        }
    }
}
