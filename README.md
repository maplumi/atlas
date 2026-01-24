
# A Spatiotemporal Compute and Rendering Engine

Repository for a Rust + WebAssembly spatiotemporal compute and rendering engine (Atlas).

This repository is organized as a Cargo workspace. The engine code lives in the `crates/` directory; the workspace root contains only meta files and workspace configuration.

Workspace members (core crates):

- `crates/foundation` — math, ids, handles, arenas, AABB, time primitives
- `crates/runtime` — scheduler, jobs, frame/timing
- `crates/scene` — entities, components, spatial & temporal indices
- `crates/streaming` — fetching, decoding, residency/LRU
- `crates/formats` — scene and chunk formats, codecs
- `crates/gpu` — wgpu wrapper, render passes, buffers
- `crates/layers` — layer abstractions (raster, vector, terrain, labels)
- `crates/compute` — analysis, VM, programmable cartography
- `crates/apps/viewer_web` — WASM/web viewer
- `crates/apps/viewer_native` — native viewer binary
- `crates/tools` — utilities and format tooling

Quickstart

Build the entire workspace:

```bash
cargo build --workspace
```

Build the native viewer:

```bash
cargo build -p viewer_native
```

Notes

- The root `Cargo.toml` is sacred: it only contains the workspace manifest. Engine implementation must live inside crates.
- Follow the dependency direction rules in project docs to avoid cycles.

**Why Atlas?**

Atlas is a spatiotemporal compute engine first — not just a renderer. It exists to make
spatial and temporal data authoritative, repeatable, and scientifically defensible: deterministic
processing, double-precision spatial math, explicit CRS transforms, and reproducible programs
make Atlas suitable for large-scale analysis, streaming datasets, and programmatic cartography.

