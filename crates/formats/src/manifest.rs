use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: &str = "1.0";

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn push_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

fn push_opt_str(buf: &mut Vec<u8>, s: &Option<String>) {
    match s {
        Some(v) => {
            buf.push(1);
            push_str(buf, v);
        }
        None => buf.push(0),
    }
}

fn push_opt_i32_array_4(buf: &mut Vec<u8>, v: &Option<[i32; 4]>) {
    match v {
        Some(a) => {
            buf.push(1);
            for x in a {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        None => buf.push(0),
    }
}

fn push_opt_i64_array_2(buf: &mut Vec<u8>, v: &Option<[i64; 2]>) {
    match v {
        Some(a) => {
            buf.push(1);
            for x in a {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        None => buf.push(0),
    }
}

fn push_opt_u32(buf: &mut Vec<u8>, v: &Option<u32>) {
    match v {
        Some(x) => {
            buf.push(1);
            buf.extend_from_slice(&x.to_le_bytes());
        }
        None => buf.push(0),
    }
}

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

    /// Compute a deterministic content hash for the manifest.
    ///
    /// This hash is stable across platforms and does not include `content_hash` itself.
    /// Chunk entries are hashed in a canonical order.
    pub fn compute_content_hash_hex(&self) -> String {
        let mut chunks = self.chunks.clone();
        chunks.sort_by(|a, b| {
            a.kind
                .cmp(&b.kind)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.id.cmp(&b.id))
        });

        let mut bytes: Vec<u8> = Vec::with_capacity(1024);
        push_str(&mut bytes, &self.version);
        push_opt_str(&mut bytes, &self.name);

        for c in &chunks {
            push_str(&mut bytes, &c.id);
            push_str(&mut bytes, &c.kind);
            push_str(&mut bytes, &c.path);

            push_opt_str(&mut bytes, &c.content_hash);
            push_opt_str(&mut bytes, &c.source_blob_hash);
            push_opt_i32_array_4(&mut bytes, &c.lon_lat_bounds_q);
            push_opt_i64_array_2(&mut bytes, &c.time_bounds_us);
            push_opt_u32(&mut bytes, &c.feature_count);
        }

        let h = blake3::hash(&bytes);
        to_hex(h.as_bytes())
    }

    /// Set the manifest `content_hash` and `package_id` based on the computed content hash.
    ///
    /// Convention: `package_id == content_hash` when `content_hash` is present.
    pub fn compute_and_set_identity(&mut self) {
        let hex = self.compute_content_hash_hex();
        self.content_hash = Some(hex.clone());
        self.package_id = hex;
    }
}
