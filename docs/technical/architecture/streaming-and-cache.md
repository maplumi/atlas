# Streaming and Cache

Streaming is explicit and budgeted.

Resource lifecycle (target model):
Requested → Downloading → Decoding → Building → Uploading → Resident → Evicted

Implemented core primitives:
- `streaming::Cache` with an explicit byte budget and deterministic LRU eviction
- `streaming::ResidencyState` for the lifecycle states above
- `streaming::Pipeline` to submit/cancel requests deterministically under a `FrameBudget`

Dataset version pinning:
- `streaming::Cache::pin_dataset_version(dataset_id, version)` records an immutable version (typically a content hash).
- Pinning deterministically invalidates stale resident entries for that dataset (stable traversal order + deterministic eviction).
- New `request()` / `mark_resident()` calls record the currently pinned version on the entry.
