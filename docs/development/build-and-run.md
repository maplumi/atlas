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

## Terrain backend (optional)
- Run: `cargo run -p server`
- Environment:
	- `TERRAIN_ROOT` (default: `data/terrain`)
	- `TERRAIN_ADDR` (default: `127.0.0.1:9100`)
	- `STAC_URL` (default: `https://copernicus-dem-30m-stac.s3.amazonaws.com`)
	- `TERRAIN_AUTO_DOWNLOAD` (set to `1` to auto-download DEM COGs on startup)
	- `TERRAIN_COLLECTION` (STAC collection id for auto-download)
	- `TERRAIN_BBOX` (minLon,minLat,maxLon,maxLat for auto-download)
	- `TERRAIN_LIMIT` (max items per STAC page; default `200`)
- Endpoints:
	- `GET /terrain/tileset.json`
	- `GET /terrain/tiles/{z}/{x}/{y}.bin`
	- `GET /terrain/status`
	- `GET /stac/collections`
	- `POST /stac/search`

### DEM downloader (local cache)
- List collections:
	- `cargo run -p server --bin terrain_fetch -- list-collections`
- Download a region (example bbox):
	- `cargo run -p server --bin terrain_fetch -- download --collection <COLLECTION_ID> --bbox -10,35,10,45 --out data/terrain/raw --limit 200`
- Download global in chunks:
	- `cargo run -p server --bin terrain_fetch -- download-global --collection <COLLECTION_ID> --chunk-deg 10 --out data/terrain/raw --limit 200`

### DEM tiling (GDAL)
Convert downloaded COGs into viewer tiles + tileset:

Requires GDAL CLI tools (`gdalinfo`, `gdalbuildvrt`, `gdalwarp`, `gdal_translate`) in PATH.

```
./scripts/dem_pipeline.py --input data/terrain/raw --output data/terrain --zoom-min 0 --zoom-max 2 --tile-size 256 --sample-step 4
```

## Docker deployment (UI + server)

- Build and run both services:
	- `docker compose up --build`
- Web UI: http://127.0.0.1:8080/
- Terrain server: http://127.0.0.1:9100/

The terrain server serves tiles from `./data/terrain` on the host. Place preprocessed
tiles under `data/terrain/tiles/{z}/{x}/{y}.bin` and a metadata file at
`data/terrain/metadata/tileset.json`.

### DEM pipeline (containerized)
Use the DEM pipeline container to download COGs and generate tiles during deployment.
It is idempotent and skips existing downloads and tiles unless forced.

Required environment variables:
- `TERRAIN_COLLECTION`
- `TERRAIN_BBOX`

Optional variables:
- `TERRAIN_LIMIT` (default: `200`)
- `TERRAIN_ZOOM_MIN` (default: `0`)
- `TERRAIN_ZOOM_MAX` (default: `2`)
- `TERRAIN_TILE_SIZE` (default: `256`)
- `TERRAIN_SAMPLE_STEP` (default: `4`)
- `TERRAIN_NO_DATA` (default: `-9999`)
- `TERRAIN_FORCE_REBUILD` (default: `0`)

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
