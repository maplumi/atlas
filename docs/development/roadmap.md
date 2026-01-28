# Roadmap

This page is the current MVP implementation checklist.

## Atlas MVP Implementation Checklist

This plan is subsystem-ordered and core-first: search/filter/symbolization/analysis live in core crates, and apps are presentation shells.

Status legend:
- [x] implemented and working
- [ ] not yet implemented (or still a placeholder/stub)

## 0) Non-negotiables (MVP contract)
- [x] Single standardized in-memory representation (`scene::World`) shared by 2D and 3D.
- [x] Every ingested feature is time-tagged (defaults to “forever” if missing).
- [x] Every ingested feature preserves properties for querying/filtering.
- [ ] Determinism policy: stable iteration ordering + stable floats policy + deterministic streaming/compute scheduling.
- [ ] Dataset identity/versioning: content hash + immutable package IDs.

## 1) Foundation (math, time, ids)
- [x] WGS84 geodesy: Geodetic ↔ ECEF (`crates/foundation/src/math/geodesy.rs`)
- [x] Local tangent frame: ECEF ↔ ENU (`crates/foundation/src/math/local.rs`)
- [x] Time primitives: `Time`, `TimeSpan`, `forever()`, `instant()` (`crates/foundation/src/time.rs`)
- [x] AABB primitives: `Aabb2`, `Aabb3` (basic structs) (`crates/foundation/src/bounds.rs`)
- [ ] Generational handles with validity + free-list reuse (currently minimal stub) (`crates/foundation/src/handles.rs`)
- [ ] Arena allocator strategy (currently minimal placeholder) (`crates/foundation/src/arena.rs`)
- [ ] Camera-relative precision model (currently only `type HighPrecision = f64`) (`crates/foundation/src/math/precision.rs`)

## 2) Runtime (deterministic scheduling + observability)
- [x] Deterministic job ordering by ID (basic scheduler) (`crates/runtime/src/scheduler.rs`)
- [ ] Frame budget management (time slicing / prioritization)
- [ ] Streaming + compute work queues with backpressure
- [ ] Metrics system (currently stub) (`crates/runtime/src/metrics.rs`)

## 3) Scene (world model, components, indices)
- [x] Minimal ECS-style `World` with sparse component vectors + visibility gating + time filtering (`crates/scene/src/world.rs`)
- [x] Feature properties component (key/value pairs) (`crates/scene/src/components/properties.rs`)
- [x] Vector geometry storage (points/lines/areas) + transforms (ECEF)
- [ ] Spatial index (quadtree/BVH) (currently stub types) (`crates/scene/src/spatial/`)
- [ ] Temporal index (interval tree) (currently stub type) (`crates/scene/src/temporal/interval_tree.rs`)
- [ ] Selection sets as bitsets + set operations (union/intersect/diff)
- [ ] Unified query API in core (spatial + temporal + attribute) with deterministic ordering
- [ ] Picking API (`pick(ray)` / `pick(screen)`) in core
- [ ] Visibility volumes / frustum culling in core

## 4) Formats (packages, chunks, determinism)
- [x] Scene manifest with version + chunk entries (minimal) (`crates/formats/src/manifest.rs`)
- [x] Vector chunk ingestion into `scene::World` (points/lines/areas) (`crates/formats/src/scene_ingest.rs`)
- [x] Ingestion time-tagging convention: `time|timestamp` (instant) or `start/end` (range), else forever (`crates/formats/src/scene_ingest.rs`)
- [x] Vector chunk binary format (fast/compact) with lon/lat quantization + semantic round-trip export
- [x] Optional blob storage for original source payloads when a blob store is configured (store hash refs in manifest)
- [x] Chunk schemas include: time domain, spatial bounds, feature count, content hash
- [ ] Binary + streamable chunk encodings (vs. JSON-only)
- [ ] Deterministic decoding and canonicalization rules

## 5) Streaming (cache + residency)
- [ ] Cache + residency lifecycle with memory budgets (currently stub) (`crates/streaming/src/cache.rs`)
- [ ] Deterministic request ordering + cancellation
- [ ] Dataset version pinning and cache invalidation rules

## 6) GPU (core renderer crate)
- [ ] Core WebGPU context/device setup in `gpu` crate (currently stub) (`crates/gpu/src/context.rs`)
- [ ] Render graph skeleton + pass scheduling
- [ ] Shared GPU buffers/textures lifecycle + upload queue

## 7) Layers (core rendering-facing data + filtering + styling)
- [x] Layer styling model: visibility/color/lift (first pass) (`crates/layers/src/symbology.rs`)
- [x] Core vector query API: kind + time + bbox + property filters (first pass) (`crates/layers/src/query.rs`)
- [ ] Spatial filtering uses real geometry/index (currently bbox proxy uses entity transform)
- [x] 3D point rendering fixed (screen-space pixel quads in WebGPU) (viewer_web)
- [x] 2D point sizing is screen-pixel based (view-scale aware default)

## 8) Compute (analysis primitives)
- [x] Minimal spatial analysis helpers (AABB, nearest-point) (`crates/compute/src/analysis/spatial.rs`)
- [ ] Geodesic distance measurement APIs
- [ ] Area measurement APIs
- [ ] Selection overlay analysis (intersects/contains)

## 9) Programmable symbolization (VM)
- [ ] Symbolization VM bytecode + interpreter (currently stub) (`crates/compute/src/vm/`)
- [ ] VM inputs: attributes/time/zoom; outputs: color/size/visibility
- [ ] Deterministic execution + resource limits

## 10) Viewer (apps: UX + wiring only)
- [x] Web viewer: 2D/3D toggle with shared dataset/world (`crates/apps/viewer_web/`)
- [x] Map controls: toggle above zoom/home controls
- [x] North/South orientation indicator (compass) wired to camera yaw
- [x] 2D pan/zoom + 3D orbit controls (deterministic/no inertia)
- [ ] Time slider (drives `Time` window)
- [ ] Attribute filter UI (drives core query filters)
- [ ] Analysis panel (calls `compute` APIs)
- [ ] Debug/metrics overlay

## 11) AI readiness (hook points)
- [ ] `QueryPlanner` trait + rule-based MVP planner implementation
- [ ] Traceable query plans (explain output) and deterministic plan execution
- [ ] Canonical “engine transcript” format for LLM tooling (inputs/outputs/logs)

## 12) Ingestion rules (hard requirements)
- [x] Every dataset gets a `TimeSpan` on ingest (even if forever)
- [x] Every dataset preserves properties for later filter/symbolization
- [ ] Dataset content hash computed and stored in manifest
- [x] Chunk-level spatial/temporal metadata baked during packaging

## Validation
- [x] `cargo fmt`, `cargo clippy`, `cargo test` (workspace)
- [x] `trunk serve` works for the web viewer
