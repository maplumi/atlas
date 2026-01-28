use std::collections::BTreeMap;

use crate::request::Request;
use crate::residency::{Residency, ResidencyState};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    pub dataset_id: String,
    pub resource_id: String,
}

impl CacheKey {
    pub fn new(dataset_id: impl Into<String>, resource_id: impl Into<String>) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            resource_id: resource_id.into(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MemoryBudget {
    pub max_bytes: usize,
}

impl MemoryBudget {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    residency: Residency,
    bytes: usize,
    last_used_tick: u64,
    pin_count: u32,
    dataset_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    BudgetExceeded { requested: usize, max: usize },
    NoEvictableEntries,
    UnknownKey,
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::BudgetExceeded { requested, max } => {
                write!(
                    f,
                    "resource too large for budget: requested={requested} max={max}"
                )
            }
            CacheError::NoEvictableEntries => write!(f, "no evictable entries (all pinned?)"),
            CacheError::UnknownKey => write!(f, "unknown cache key"),
        }
    }
}

impl std::error::Error for CacheError {}

/// Deterministic in-memory cache with explicit residency and a byte budget.
///
/// Notes on determinism:
/// - Entries are keyed in a `BTreeMap` for stable traversal order.
/// - Eviction is LRU by `last_used_tick`, with a tie-break by key ordering.
#[derive(Debug)]
pub struct Cache {
    budget: MemoryBudget,
    used_bytes: usize,
    tick: u64,
    next_request: u64,
    entries: BTreeMap<CacheKey, CacheEntry>,
    requests: BTreeMap<Request, CacheKey>,
    pinned_versions: BTreeMap<String, String>,
}

impl Cache {
    pub fn new(budget: MemoryBudget) -> Self {
        Self {
            budget,
            used_bytes: 0,
            tick: 0,
            next_request: 1,
            entries: BTreeMap::new(),
            requests: BTreeMap::new(),
            pinned_versions: BTreeMap::new(),
        }
    }

    /// Pin a dataset to a specific immutable version (typically a content hash).
    ///
    /// Any resident entries from older versions are deterministically evicted.
    pub fn pin_dataset_version(
        &mut self,
        dataset_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Vec<CacheKey> {
        let dataset_id = dataset_id.into();
        let version = version.into();

        self.pinned_versions
            .insert(dataset_id.clone(), version.clone());

        // Deterministically invalidate any entries that don't match the pinned version.
        // - Always update the entry's recorded version to the pinned one.
        // - Evict resident stale entries to free memory and prevent accidental use.
        let mut evicted: Vec<CacheKey> = Vec::new();

        let keys_to_check: Vec<CacheKey> = self
            .entries
            .keys()
            .filter(|k| k.dataset_id == dataset_id)
            .cloned()
            .collect();

        for k in keys_to_check {
            let Some(e) = self.entries.get_mut(&k) else {
                continue;
            };

            if e.dataset_version.as_deref() == Some(version.as_str()) {
                continue;
            }

            e.dataset_version = Some(version.clone());

            if e.residency.state == ResidencyState::Resident {
                // `evict` also updates used_bytes deterministically.
                let _ = self.evict(&k);
                evicted.push(k);
            }
        }

        evicted
    }

    pub fn pinned_dataset_version(&self, dataset_id: &str) -> Option<&str> {
        self.pinned_versions.get(dataset_id).map(|s| s.as_str())
    }

    pub fn budget(&self) -> MemoryBudget {
        self.budget
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn state(&self, key: &CacheKey) -> Option<ResidencyState> {
        self.entries.get(key).map(|e| e.residency.state)
    }

    pub fn request(&mut self, key: CacheKey) -> Request {
        self.tick += 1;

        let dataset_version = self.pinned_versions.get(&key.dataset_id).cloned();
        let entry = self
            .entries
            .entry(key.clone())
            .or_insert_with(|| CacheEntry {
                residency: Residency::new(),
                bytes: 0,
                last_used_tick: self.tick,
                pin_count: 0,
                dataset_version: dataset_version.clone(),
            });
        entry.dataset_version = dataset_version;
        entry.residency.state = ResidencyState::Requested;
        entry.last_used_tick = self.tick;

        let req = Request(self.next_request);
        self.next_request += 1;
        self.requests.insert(req, key);
        req
    }

    pub fn key_for_request(&self, req: Request) -> Option<&CacheKey> {
        self.requests.get(&req)
    }

    pub fn touch(&mut self, key: &CacheKey) -> Result<(), CacheError> {
        self.tick += 1;
        let entry = self.entries.get_mut(key).ok_or(CacheError::UnknownKey)?;
        entry.last_used_tick = self.tick;
        Ok(())
    }

    pub fn pin(&mut self, key: &CacheKey) -> Result<(), CacheError> {
        let entry = self.entries.get_mut(key).ok_or(CacheError::UnknownKey)?;
        entry.pin_count = entry.pin_count.saturating_add(1);
        Ok(())
    }

    pub fn unpin(&mut self, key: &CacheKey) -> Result<(), CacheError> {
        let entry = self.entries.get_mut(key).ok_or(CacheError::UnknownKey)?;
        entry.pin_count = entry.pin_count.saturating_sub(1);
        Ok(())
    }

    pub fn set_state(&mut self, key: &CacheKey, state: ResidencyState) -> Result<(), CacheError> {
        let entry = self.entries.get_mut(key).ok_or(CacheError::UnknownKey)?;
        entry.residency.state = state;
        Ok(())
    }

    pub fn mark_resident(
        &mut self,
        key: &CacheKey,
        bytes: usize,
    ) -> Result<Vec<CacheKey>, CacheError> {
        if bytes > self.budget.max_bytes {
            return Err(CacheError::BudgetExceeded {
                requested: bytes,
                max: self.budget.max_bytes,
            });
        }

        self.tick += 1;

        let pinned_version = self.pinned_versions.get(&key.dataset_id).cloned();

        let entry = self
            .entries
            .entry(key.clone())
            .or_insert_with(|| CacheEntry {
                residency: Residency::new(),
                bytes: 0,
                last_used_tick: self.tick,
                pin_count: 0,
                dataset_version: pinned_version.clone(),
            });

        // If the dataset pin changed since this entry was created, invalidate the old contents.
        if entry.dataset_version != pinned_version {
            if entry.residency.state == ResidencyState::Resident {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            }
            entry.bytes = 0;
            entry.residency.state = ResidencyState::Evicted;
            entry.dataset_version = pinned_version.clone();
        }

        // If re-sizing an existing resident entry, adjust used bytes.
        if entry.residency.state == ResidencyState::Resident {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
        }

        entry.bytes = bytes;
        entry.residency.state = ResidencyState::Resident;
        entry.last_used_tick = self.tick;
        self.used_bytes += bytes;

        self.evict_as_needed(Some(key))
    }

    pub fn evict(&mut self, key: &CacheKey) -> Result<(), CacheError> {
        let entry = self.entries.get_mut(key).ok_or(CacheError::UnknownKey)?;
        if entry.residency.state == ResidencyState::Resident {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
        }
        entry.bytes = 0;
        entry.residency.state = ResidencyState::Evicted;
        Ok(())
    }

    fn evict_as_needed(
        &mut self,
        protected: Option<&CacheKey>,
    ) -> Result<Vec<CacheKey>, CacheError> {
        let mut evicted: Vec<CacheKey> = Vec::new();
        while self.used_bytes > self.budget.max_bytes {
            let pick = |exclude: Option<&CacheKey>| {
                self.entries
                    .iter()
                    .filter(|(k, e)| {
                        e.residency.state == ResidencyState::Resident
                            && e.pin_count == 0
                            && exclude.map(|p| p != *k).unwrap_or(true)
                    })
                    .min_by(|(ka, ea), (kb, eb)| {
                        ea.last_used_tick
                            .cmp(&eb.last_used_tick)
                            .then_with(|| ka.cmp(kb))
                    })
                    .map(|(k, _)| k.clone())
            };

            // Prefer not to evict the just-produced resident entry, but allow it
            // if everything else is pinned (deterministic fallback).
            let candidate = pick(protected).or_else(|| pick(None));

            let Some(key) = candidate else {
                return Err(CacheError::NoEvictableEntries);
            };

            self.evict(&key)?;
            evicted.push(key);
        }
        Ok(evicted)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cache, CacheKey, MemoryBudget};
    use crate::residency::ResidencyState;

    #[test]
    fn lru_eviction_is_deterministic() {
        let mut cache = Cache::new(MemoryBudget::new(10));
        let a = CacheKey::new("ds", "a");
        let b = CacheKey::new("ds", "b");

        cache.mark_resident(&a, 6).unwrap();
        cache.mark_resident(&b, 6).unwrap();

        // Total 12 > 10, so one entry must be evicted; 'a' is older.
        assert_eq!(cache.state(&a), Some(ResidencyState::Evicted));
        assert_eq!(cache.state(&b), Some(ResidencyState::Resident));
        assert!(cache.used_bytes() <= cache.budget().max_bytes);
    }

    #[test]
    fn pinned_entries_are_not_evicted() {
        let mut cache = Cache::new(MemoryBudget::new(10));
        let a = CacheKey::new("ds", "a");
        let b = CacheKey::new("ds", "b");

        cache.mark_resident(&a, 6).unwrap();
        cache.pin(&a).unwrap();

        cache.mark_resident(&b, 6).unwrap();

        // 'a' cannot be evicted due to pin; 'b' should be evicted instead.
        assert_eq!(cache.state(&a), Some(ResidencyState::Resident));
        assert_eq!(cache.state(&b), Some(ResidencyState::Evicted));
    }

    #[test]
    fn pinning_dataset_version_invalidates_stale_resident_entries() {
        let mut cache = Cache::new(MemoryBudget::new(100));
        let a = CacheKey::new("ds", "a");

        cache.mark_resident(&a, 10).unwrap();
        assert_eq!(cache.state(&a), Some(ResidencyState::Resident));
        assert_eq!(cache.used_bytes(), 10);

        let evicted = cache.pin_dataset_version("ds", "v1");
        assert_eq!(evicted, vec![a.clone()]);
        assert_eq!(cache.pinned_dataset_version("ds"), Some("v1"));
        assert_eq!(cache.state(&a), Some(ResidencyState::Evicted));
        assert_eq!(cache.used_bytes(), 0);

        cache.mark_resident(&a, 10).unwrap();
        assert_eq!(cache.state(&a), Some(ResidencyState::Resident));
        assert_eq!(cache.used_bytes(), 10);
    }
}
