use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneManifest {
    pub version: String,
    pub package_id: String,
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub chunks: Vec<ChunkEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkEntry {
    pub id: String,
    pub kind: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_blob_hash: Option<String>,

    // Optional baked metadata for fast chunk pruning / indexing.
    // Quantization matches AVc (1e-6 degrees): [min_lon_q, min_lat_q, max_lon_q, max_lat_q]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon_lat_bounds_q: Option<[i32; 4]>,
    // Microseconds: [min_start_us, max_end_us]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_bounds_us: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_count: Option<u32>,
}

impl SceneManifest {
    pub fn new(package_id: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION.to_string(),
            package_id: package_id.into(),
            name: None,
            content_hash: None,
            chunks: Vec::new(),
        }
    }
}
