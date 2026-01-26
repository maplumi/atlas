# Atlas Viewer MVP Plan

Goal: minimal spatiotemporal viewer aligned with the Constitution (deterministic, precise, programmable) with 2D/3D toggle sharing the same data and controls. Use WebGPU for 3D from day one; keep 2D canvas for fast UI overlays. Datasets are loaded once and reused across modes.

## Milestones

1) Data & Formats
- Provide a sample scene package (manifest + chunks) with real-world WGS84 data: points (cities), lines (sample air corridors), areas (country/region polygons simplified).
- Loader (wasm) to fetch manifest/chunks, normalize into `SceneManifest` and populate `scene::World` with 2D/3D drawables and transforms.
- Keep deterministic transforms and reuse the same world for 2D and 3D.

2) 2D Viewer (Canvas)
- Pan/zoom camera (deterministic, no inertia): mouse drag pans; wheel zooms about cursor; reset control.
- Render pipeline: collect render commands from `gpu::Renderer` (2D path) and draw via canvas with camera transform.
- Visibility toggles (grid, overlays) and dataset selection without reload of code.

3) 3D Viewer (WebGPU)
- Separate WebGPU canvas; initialize wgpu with the same scene data.
- Minimal globe render: clear + draw Earth sphere proxy; place point/line/area markers projected to globe (initially billboards/lines in clip space).
- Orbit controls: yaw/pitch, zoom; deterministic (no inertia) with reset.
- Mode switch (2D/3D) swaps active surface but keeps the same world/data and camera state per mode.

4) UI/Controls
- Left panel: mode toggle, dataset chooser, checkboxes (grid, cube/markers), camera reset buttons (2D/3D), load custom dataset URL.
- Status line showing mode, dataset, and camera state.

5) Stretch (later)
- DuckDB-WASM ingest path to convert arbitrary user CSV/GeoJSON into engine chunk format.
- Real chunk parsers and GPU path for large datasets; doc tooling to publish releases into docs pages.

## Implementation Order
1. Add sample scene data under `crates/apps/viewer_web/assets/` + manifest.
2. Implement loader and world builder (shared for 2D/3D) using existing engine components.
3. Add 2D camera (pan/zoom) and render pipeline on canvas.
4. Integrate WebGPU surface; draw minimal globe + markers; hook orbit controls and mode toggle.
5. Polish UI controls and status; ensure deterministic behavior.

## Testing
- Workspace `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`.
- For web: `trunk serve --release` and manual verify 2D/3D toggles share the same dataset; pan/zoom/orbit/reset; load sample dataset from assets.
