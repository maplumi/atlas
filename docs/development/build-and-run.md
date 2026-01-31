# Build and Run

## Native viewer
- Build: `cargo build -p native`
- Run: `cargo run -p native`

## Web viewer
- Target: `wasm32-unknown-unknown`
- Build: `cargo build -p web --target wasm32-unknown-unknown`

### Built-in datasets (web)
- Stored under `crates/apps/web/assets/` (e.g., `world.json`, `chunks/*.avc`).
- Bundled at build time by Trunk via `copy-dir` in [crates/apps/web/index.html](crates/apps/web/index.html).
- Deployed as static assets under `/assets/` alongside the compiled WASM/JS bundle.

### Surface vs. Terrain (web)
- The globe defaults to a fast vector surface from `assets/world.json`.
- A UI toggle enables streaming 3D terrain tiles on demand.

## Terrain backend (optional)
- Run: `cargo run -p server`
- Environment:
	- `TERRAIN_ROOT` (default: `/data/terrain`)
	- `TERRAIN_CACHE_ROOT` (default: `<TERRAIN_ROOT>/cache`)
	- `TERRAIN_ADDR` (default: `127.0.0.1:9100`)
	- `STAC_URL` (default: `https://copernicus-dem-30m-stac.s3.amazonaws.com`)
	- `TERRAIN_COLLECTION` (STAC collection id; default `dem_cop_30`)
	- `TERRAIN_TILE_SIZE` (default `256`)
	- `TERRAIN_ZOOM_MIN` / `TERRAIN_ZOOM_MAX` (default `0` / `8`)
	- `TERRAIN_SAMPLE_STEP` (default `4`)
	- `TERRAIN_NO_DATA` (default `-9999`)
	- `TERRAIN_MIN_LON` / `TERRAIN_MAX_LON` (default `-180` / `180`)
	- `TERRAIN_MIN_LAT` / `TERRAIN_MAX_LAT` (default `-90` / `90`)
	- `TERRAIN_MAX_COGS_PER_TILE` (default `16`)
- Endpoints:
	- `GET /terrain/tileset.json`
	- `GET /terrain/tiles/{z}/{x}/{y}.bin`
	- `GET /terrain/status`

The terrain backend generates tiles on demand using GDAL CLI tools (`gdalbuildvrt`, `gdal_translate`).

## Docker deployment (UI + server)

- Build and run both services:
	- `docker compose up --build`

- Web UI: http://127.0.0.1:8082/
- Terrain server: http://127.0.0.1:9102/

The terrain server streams tiles into a container volume at `/data/terrain/cache`.

## Web ↔ backend integration (fallback-first)

The web UI is designed to work without a backend and will default to built-in assets.
When a backend is available, the UI will use it automatically.

### How it works
- On startup the web UI probes `GET /healthz` on the backend.
- If the backend responds, the UI considers the backend **connected** and sets:
	- `window.__atlasBackendUrl` (base URL, e.g. `http://<host>:9100`)
	- `window.__atlasStacUrl` (proxy endpoint, e.g. `http://<host>:9100/stac`)
- If the backend is unavailable, the UI falls back to defaults:
	- `window.__atlasBackendUrl` is empty
	- `window.__atlasStacUrl` is `https://copernicus-dem-30m-stac.s3.amazonaws.com`

### Override backend URL
You can force a specific backend by defining `window.ATLAS_BACKEND_URL` before the app loads
(e.g. via a small inline script or a hosting template). Example:

```html
<script>
	window.ATLAS_BACKEND_URL = "http://my-host:9100";
</script>
```
