use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    /// Data is stored in Atlas' binary vector chunk format.
    pub avc_base64: String,
    pub count_points: usize,
    pub count_lines: usize,
    pub count_polys: usize,
    pub created_at_ms: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub entries: BTreeMap<String, CatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    NotFound,
    StorageUnavailable,
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::NotFound => write!(f, "catalog entry not found"),
            CatalogError::StorageUnavailable => write!(f, "browser storage unavailable"),
            CatalogError::Corrupt(msg) => write!(f, "catalog storage corrupt: {msg}"),
            CatalogError::Io(msg) => write!(f, "catalog storage error: {msg}"),
        }
    }
}

impl std::error::Error for CatalogError {}

pub trait CatalogStore {
    fn list(&self) -> Result<Vec<CatalogEntry>, CatalogError>;
    fn get(&self, id: &str) -> Result<Option<CatalogEntry>, CatalogError>;
    fn upsert(&mut self, entry: CatalogEntry) -> Result<(), CatalogError>;
    fn delete(&mut self, id: &str) -> Result<bool, CatalogError>;
}

pub fn id_for_avc_bytes(avc_bytes: &[u8]) -> String {
    blake3::hash(avc_bytes).to_hex().to_string()
}

pub fn avc_bytes_to_base64(avc_bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(avc_bytes)
}

pub fn avc_base64_to_bytes(avc_base64: &str) -> Result<Vec<u8>, CatalogError> {
    base64::engine::general_purpose::STANDARD
        .decode(avc_base64)
        .map_err(|e| CatalogError::Corrupt(e.to_string()))
}

#[derive(Debug, Default)]
pub struct InMemoryCatalogStore {
    snapshot: CatalogSnapshot,
}

impl InMemoryCatalogStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_avc_bytes(
        &mut self,
        mut entry: CatalogEntry,
        avc_bytes: &[u8],
    ) -> Result<(), CatalogError> {
        entry.avc_base64 = avc_bytes_to_base64(avc_bytes);
        self.upsert(entry)
    }

    pub fn get_avc_bytes(&self, id: &str) -> Result<Option<Vec<u8>>, CatalogError> {
        let Some(entry) = self.get(id)? else {
            return Ok(None);
        };
        avc_base64_to_bytes(&entry.avc_base64).map(Some)
    }
}

impl CatalogStore for InMemoryCatalogStore {
    fn list(&self) -> Result<Vec<CatalogEntry>, CatalogError> {
        Ok(self.snapshot.entries.values().cloned().collect())
    }

    fn get(&self, id: &str) -> Result<Option<CatalogEntry>, CatalogError> {
        Ok(self.snapshot.entries.get(id).cloned())
    }

    fn upsert(&mut self, entry: CatalogEntry) -> Result<(), CatalogError> {
        self.snapshot.entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<bool, CatalogError> {
        Ok(self.snapshot.entries.remove(id).is_some())
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_storage {
    use super::{CatalogEntry, CatalogError, CatalogSnapshot, CatalogStore};
    use base64::Engine as _;

    // Keep per-chunk strings relatively small to reduce peak wasm allocations.
    // IMPORTANT: must be a multiple of 4 to preserve base64 quartet boundaries.
    const AVC_CHUNK_CHARS: usize = 64_000;

    #[derive(Debug)]
    pub struct LocalStorageCatalogStore {
        key_prefix: String,
    }

    impl LocalStorageCatalogStore {
        pub fn new(key: impl Into<String>) -> Result<Self, CatalogError> {
            let key_prefix = key.into();
            let store = Self { key_prefix };

            // Best-effort migration from legacy format (single snapshot stored at `key_prefix`).
            // This avoids huge JSON re-serialization on each upsert, which can OOM in wasm.
            store.migrate_legacy_snapshot()?;

            Ok(store)
        }

        fn index_key(&self) -> String {
            format!("{}.index", self.key_prefix)
        }

        fn entry_key(&self, id: &str) -> String {
            format!("{}.entry.{}", self.key_prefix, id)
        }

        fn avc_count_key(&self, id: &str) -> String {
            format!("{}.avc.{}.count", self.key_prefix, id)
        }

        fn avc_chunk_key(&self, id: &str, idx: usize) -> String {
            format!("{}.avc.{}.{}", self.key_prefix, id, idx)
        }

        fn legacy_snapshot_key(&self) -> &str {
            &self.key_prefix
        }

        fn load_index(&self) -> Result<Vec<String>, CatalogError> {
            let storage = window_local_storage()?;
            let raw = storage
                .get_item(&self.index_key())
                .map_err(|e| CatalogError::Io(format!("get_item(index) failed: {:?}", e)))?;

            let Some(raw) = raw else {
                return Ok(Vec::new());
            };
            if raw.trim().is_empty() {
                return Ok(Vec::new());
            }
            let mut ids = serde_json::from_str::<Vec<String>>(&raw)
                .map_err(|e| CatalogError::Corrupt(e.to_string()))?;
            ids.sort();
            ids.dedup();
            Ok(ids)
        }

        fn save_index(&self, mut ids: Vec<String>) -> Result<(), CatalogError> {
            ids.sort();
            ids.dedup();

            let storage = window_local_storage()?;
            let raw = serde_json::to_string(&ids).map_err(|e| CatalogError::Io(e.to_string()))?;
            storage
                .set_item(&self.index_key(), &raw)
                .map_err(|e| CatalogError::Io(format!("set_item(index) failed: {:?}", e)))?;
            Ok(())
        }

        fn load_entry(&self, id: &str) -> Result<Option<CatalogEntry>, CatalogError> {
            let storage = window_local_storage()?;
            let raw = storage
                .get_item(&self.entry_key(id))
                .map_err(|e| CatalogError::Io(format!("get_item(entry) failed: {:?}", e)))?;
            let Some(raw) = raw else {
                return Ok(None);
            };
            if raw.trim().is_empty() {
                return Ok(None);
            }
            let entry = serde_json::from_str::<CatalogEntry>(&raw)
                .map_err(|e| CatalogError::Corrupt(e.to_string()))?;
            Ok(Some(entry))
        }

        fn save_entry(&self, entry: &CatalogEntry) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;
            let raw = serde_json::to_string(entry).map_err(|e| CatalogError::Io(e.to_string()))?;
            storage
                .set_item(&self.entry_key(&entry.id), &raw)
                .map_err(|e| CatalogError::Io(format!("set_item(entry) failed: {:?}", e)))?;
            Ok(())
        }

        fn load_avc_chunk_count(&self, id: &str) -> Result<usize, CatalogError> {
            let storage = window_local_storage()?;
            let raw = storage
                .get_item(&self.avc_count_key(id))
                .map_err(|e| CatalogError::Io(format!("get_item(avc_count) failed: {:?}", e)))?;
            let Some(raw) = raw else {
                return Ok(0);
            };
            let raw = raw.trim();
            if raw.is_empty() {
                return Ok(0);
            }
            raw.parse::<usize>()
                .map_err(|e| CatalogError::Corrupt(format!("invalid avc chunk count: {e}")))
        }

        fn save_avc_chunk_count(&self, id: &str, count: usize) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;
            storage
                .set_item(&self.avc_count_key(id), &count.to_string())
                .map_err(|e| CatalogError::Io(format!("set_item(avc_count) failed: {:?}", e)))?;
            Ok(())
        }

        fn remove_avc_chunks(&self, id: &str) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;
            let count = self.load_avc_chunk_count(id)?;
            for i in 0..count {
                let _ = storage.remove_item(&self.avc_chunk_key(id, i));
            }
            let _ = storage.remove_item(&self.avc_count_key(id));
            Ok(())
        }

        fn save_avc_chunks_from_bytes(
            &self,
            id: &str,
            avc_bytes: &[u8],
        ) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;

            // Convert max output chars to a safe input chunk size.
            let chunk_bytes = (AVC_CHUNK_CHARS / 4) * 3;
            let chunk_bytes = chunk_bytes.max(3);

            // Clean up any previous chunk data for this id first.
            self.remove_avc_chunks(id)?;

            let mut count = 0usize;
            for (i, chunk) in avc_bytes.chunks(chunk_bytes).enumerate() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
                storage
                    .set_item(&self.avc_chunk_key(id, i), &b64)
                    .map_err(|e| {
                        CatalogError::Io(format!("set_item(avc_chunk) failed: {:?}", e))
                    })?;
                count = i + 1;
            }

            self.save_avc_chunk_count(id, count)?;
            Ok(())
        }

        fn save_avc_chunks_from_base64_str(
            &self,
            id: &str,
            avc_base64: &str,
        ) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;

            // Clean up any previous chunk data for this id first.
            self.remove_avc_chunks(id)?;

            let mut count = 0usize;
            for (i, chunk) in avc_base64.as_bytes().chunks(AVC_CHUNK_CHARS).enumerate() {
                // Safety: only store valid UTF-8 (base64 is ASCII).
                let s = std::str::from_utf8(chunk)
                    .map_err(|e| CatalogError::Corrupt(format!("invalid base64 utf8: {e}")))?;
                storage
                    .set_item(&self.avc_chunk_key(id, i), s)
                    .map_err(|e| {
                        CatalogError::Io(format!("set_item(avc_chunk) failed: {:?}", e))
                    })?;
                count = i + 1;
            }

            self.save_avc_chunk_count(id, count)?;
            Ok(())
        }

        pub fn upsert_avc_bytes(
            &mut self,
            mut entry: CatalogEntry,
            avc_bytes: &[u8],
        ) -> Result<(), CatalogError> {
            // Keep metadata JSON small; AVC is persisted separately in chunked base64.
            entry.avc_base64.clear();
            self.save_entry(&entry)?;
            self.save_avc_chunks_from_bytes(&entry.id, avc_bytes)?;

            let mut ids = self.load_index()?;
            if !ids.iter().any(|x| x == &entry.id) {
                ids.push(entry.id.clone());
                self.save_index(ids)?;
            }
            Ok(())
        }

        pub fn get_avc_bytes(&self, id: &str) -> Result<Option<Vec<u8>>, CatalogError> {
            let Some(entry) = self.load_entry(id)? else {
                return Ok(None);
            };

            // Back-compat: if AVC is inlined, decode it directly.
            if !entry.avc_base64.trim().is_empty() {
                return super::avc_base64_to_bytes(&entry.avc_base64).map(Some);
            }

            let count = self.load_avc_chunk_count(id)?;
            if count == 0 {
                return Err(CatalogError::Corrupt(
                    "missing AVC payload for catalog entry".to_string(),
                ));
            }

            let storage = window_local_storage()?;
            let mut out: Vec<u8> = Vec::new();
            for i in 0..count {
                let raw = storage
                    .get_item(&self.avc_chunk_key(id, i))
                    .map_err(|e| CatalogError::Io(format!("get_item(avc_chunk) failed: {:?}", e)))?
                    .ok_or_else(|| CatalogError::Corrupt("missing AVC chunk".to_string()))?;
                let bytes = super::avc_base64_to_bytes(&raw)?;
                out.extend_from_slice(&bytes);
            }
            Ok(Some(out))
        }

        fn remove_entry(&self, id: &str) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;
            storage
                .remove_item(&self.entry_key(id))
                .map_err(|e| CatalogError::Io(format!("remove_item(entry) failed: {:?}", e)))?;
            Ok(())
        }

        fn migrate_legacy_snapshot(&self) -> Result<(), CatalogError> {
            let storage = window_local_storage()?;
            let legacy_raw = storage
                .get_item(self.legacy_snapshot_key())
                .map_err(|e| CatalogError::Io(format!("get_item(legacy) failed: {:?}", e)))?;

            let Some(legacy_raw) = legacy_raw else {
                return Ok(());
            };
            if legacy_raw.trim().is_empty() {
                // Remove empty legacy payload if it exists.
                let _ = storage.remove_item(self.legacy_snapshot_key());
                return Ok(());
            }

            // If the legacy payload isn't valid JSON snapshot, don't loop on it forever.
            let snap = match serde_json::from_str::<CatalogSnapshot>(&legacy_raw) {
                Ok(s) => s,
                Err(_) => {
                    let _ = storage.remove_item(self.legacy_snapshot_key());
                    return Ok(());
                }
            };

            let mut ids: Vec<String> = Vec::with_capacity(snap.entries.len());
            for (id, entry) in snap.entries {
                // Ensure consistency.
                if entry.id != id {
                    continue;
                }
                // Persist metadata without embedding huge base64 in JSON; store AVC separately.
                let mut meta = entry.clone();
                let avc_base64 = std::mem::take(&mut meta.avc_base64);
                self.save_entry(&meta)?;
                self.save_avc_chunks_from_base64_str(&id, &avc_base64)?;
                ids.push(id);
            }
            self.save_index(ids)?;

            // Remove the legacy snapshot key after migration.
            let _ = storage.remove_item(self.legacy_snapshot_key());
            Ok(())
        }
    }

    impl CatalogStore for LocalStorageCatalogStore {
        fn list(&self) -> Result<Vec<CatalogEntry>, CatalogError> {
            let ids = self.load_index()?;
            let mut out: Vec<CatalogEntry> = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(e) = self.load_entry(&id)? {
                    out.push(e);
                }
            }
            // Deterministic presentation: most recent first, then id.
            out.sort_by(|a, b| {
                b.created_at_ms
                    .cmp(&a.created_at_ms)
                    .then_with(|| a.id.cmp(&b.id))
            });
            Ok(out)
        }

        fn get(&self, id: &str) -> Result<Option<CatalogEntry>, CatalogError> {
            self.load_entry(id)
        }

        fn upsert(&mut self, entry: CatalogEntry) -> Result<(), CatalogError> {
            // Back-compat: callers may still pass inline base64. Keep it working by chunking.
            if !entry.avc_base64.trim().is_empty() {
                // Store metadata with empty payload; store base64 separately to reduce JSON size.
                let mut meta = entry.clone();
                let avc_base64 = std::mem::take(&mut meta.avc_base64);
                self.save_entry(&meta)?;
                self.save_avc_chunks_from_base64_str(&meta.id, &avc_base64)?;
            } else {
                self.save_entry(&entry)?;
            }

            let mut ids = self.load_index()?;
            if !ids.iter().any(|x| x == &entry.id) {
                ids.push(entry.id.clone());
                self.save_index(ids)?;
            }
            Ok(())
        }

        fn delete(&mut self, id: &str) -> Result<bool, CatalogError> {
            let existed = self.load_entry(id)?.is_some();
            if existed {
                self.remove_entry(id)?;
                let _ = self.remove_avc_chunks(id);
            }
            let mut ids = self.load_index()?;
            let before = ids.len();
            ids.retain(|x| x != id);
            if ids.len() != before {
                self.save_index(ids)?;
            }
            Ok(existed)
        }
    }

    fn window_local_storage() -> Result<web_sys::Storage, CatalogError> {
        let win = web_sys::window().ok_or(CatalogError::StorageUnavailable)?;
        win.local_storage()
            .map_err(|e| CatalogError::Io(format!("localStorage error: {:?}", e)))?
            .ok_or(CatalogError::StorageUnavailable)
    }

    // Note: legacy snapshot load/save helpers were removed in favor of per-entry keys.

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::BTreeMap;

        #[test]
        fn snapshot_default_is_empty() {
            let s = CatalogSnapshot::default();
            assert!(s.entries.is_empty());
        }

        #[test]
        fn btreemap_order_is_stable() {
            let mut m: BTreeMap<String, CatalogEntry> = BTreeMap::new();
            m.insert(
                "b".to_string(),
                CatalogEntry {
                    id: "b".to_string(),
                    name: "b".to_string(),
                    avc_base64: "".to_string(),
                    count_points: 0,
                    count_lines: 0,
                    count_polys: 0,
                    created_at_ms: 0,
                },
            );
            m.insert(
                "a".to_string(),
                CatalogEntry {
                    id: "a".to_string(),
                    name: "a".to_string(),
                    avc_base64: "".to_string(),
                    count_points: 0,
                    count_lines: 0,
                    count_polys: 0,
                    created_at_ms: 0,
                },
            );
            let snap = CatalogSnapshot { entries: m };
            let json = serde_json::to_string(&snap).unwrap();
            // a should appear before b in JSON due to BTreeMap.
            assert!(json.find("\"a\"").unwrap() < json.find("\"b\"").unwrap());
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_storage::LocalStorageCatalogStore;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct LocalStorageCatalogStore;

#[cfg(not(target_arch = "wasm32"))]
impl LocalStorageCatalogStore {
    pub fn new(_key: impl Into<String>) -> Result<Self, CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }

    pub fn upsert_avc_bytes(
        &mut self,
        _entry: CatalogEntry,
        _avc_bytes: &[u8],
    ) -> Result<(), CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }

    pub fn get_avc_bytes(&self, _id: &str) -> Result<Option<Vec<u8>>, CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CatalogStore for LocalStorageCatalogStore {
    fn list(&self) -> Result<Vec<CatalogEntry>, CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }

    fn get(&self, _id: &str) -> Result<Option<CatalogEntry>, CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }

    fn upsert(&mut self, _entry: CatalogEntry) -> Result<(), CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }

    fn delete(&mut self, _id: &str) -> Result<bool, CatalogError> {
        Err(CatalogError::StorageUnavailable)
    }
}
