# Build and Run

## Native viewer
- Build: `cargo build -p viewer_native`
- Run: `cargo run -p viewer_native`

## Web viewer
- Target: `wasm32-unknown-unknown`
- Build: `cargo build -p viewer_web --target wasm32-unknown-unknown`

### Built-in datasets (web)
- Stored under `crates/apps/viewer_web/assets/` (e.g., `world.json`, `chunks/*.avc`).
- Bundled at build time by Trunk via `copy-dir` in [crates/apps/viewer_web/index.html](crates/apps/viewer_web/index.html).
- Deployed as static assets under `/assets/` alongside the compiled WASM/JS bundle.

## Terrain backend (optional)
- Run: `cargo run -p terrain_server`
- Environment:
	- `TERRAIN_ROOT` (default: `data/terrain`)
	- `TERRAIN_ADDR` (default: `127.0.0.1:9100`)
	- `STAC_URL` (default: `https://copernicus-dem-30m-stac.s3.amazonaws.com`)
- Endpoints:
	- `GET /terrain/tileset.json`
	- `GET /terrain/tiles/{z}/{x}/{y}.bin`
	- `GET /stac/collections`
	- `POST /stac/search`

### DEM downloader (local cache)
- List collections:
	- `cargo run -p terrain_server --bin terrain_fetch -- list-collections`
- Download a region (example bbox):
	- `cargo run -p terrain_server --bin terrain_fetch -- download --collection <COLLECTION_ID> --bbox -10,35,10,45 --out data/terrain/raw --limit 200`
- Download global in chunks:
	- `cargo run -p terrain_server --bin terrain_fetch -- download-global --collection <COLLECTION_ID> --chunk-deg 10 --out data/terrain/raw --limit 200`

## Docker deployment (UI + server)

- Build and run both services:
	- `docker compose up --build`
- Web UI: http://127.0.0.1:8080/
- Terrain server: http://127.0.0.1:9100/

The terrain server serves tiles from `./data/terrain` on the host. Place preprocessed
tiles under `data/terrain/tiles/{z}/{x}/{y}.bin` and a metadata file at
`data/terrain/metadata/tileset.json`.

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
