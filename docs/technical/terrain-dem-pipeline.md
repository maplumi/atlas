# Terrain DEM Pipeline (Local Static + Optional Backend)

This document outlines a local‑static hosting approach for global DEM tiles, with an optional backend, and how STAC can be used for discovery.

## Goals
- Global DEM (30m preferred) for terrain mesh/height sampling.
- Local static hosting for dev/prototyping.
- Optional backend service for caching, authorization, or preprocessing.
- Support STAC for discovery and metadata, but avoid pulling the full global dataset into the app.

## Recommended Source
- Copernicus DEM GLO‑30 Public (DSM) via AWS Open Data bucket `copernicus-dem-30m`.
- Format: Cloud Optimized GeoTIFF (COG) tiles.
- STAC endpoint (metadata): `https://copernicus-dem-30m-stac.s3.amazonaws.com/`.

## Local Static Hosting (Dev)

### Workflow
1) Query STAC for tiles that intersect a region or a bounding box.
2) Download tiles as COGs to a local cache.
3) Preprocess into a **viewer‑friendly tile format** (heightmap tiles).
4) Serve tiles via a static HTTP server (local).

### Pipeline tool (GDAL)
Convert the downloaded COGs into EPSG:4326 heightmap tiles:

```
./scripts/dem_pipeline.py \
  --input data/terrain/raw \
  --output data/terrain \
  --zoom-min 0 \
  --zoom-max 2 \
  --tile-size 256 \
  --sample-step 4
```

This produces:
- `data/terrain/tiles/{z}/{x}/{y}.bin`
- `data/terrain/metadata/tileset.json`

### Pipeline container (deployment)
Use the `dem-pipeline` service in docker-compose to run downloads + tiling during deployment.
It is idempotent and skips existing downloads and tiles unless forced.

### Suggested Tile Format
- Heightmap tiles (e.g., 256x256 or 512x512) in a compact binary format.
- Per‑tile metadata: bounds, min/max elevation, and tile resolution.

#### Tile binary
- Format: little‑endian Float32 (meters)
- Layout: row‑major, north→south rows, west→east columns
- No‑data value: configurable (default: -9999)

#### Tileset JSON
`data/terrain/metadata/tileset.json`

Example schema:
```json
{
  "version": 1,
  "tile_size": 256,
  "zoom_min": 0,
  "zoom_max": 2,
  "data_type": "f32",
  "tile_path_template": "tiles/{z}/{x}/{y}.bin",
  "min_lon": -180.0,
  "max_lon": 180.0,
  "min_lat": -90.0,
  "max_lat": 90.0,
  "min_height": -1000.0,
  "max_height": 9000.0,
  "no_data": -9999.0,
  "sample_step": 4
}
```

### Directory Layout (proposal)
```
data/terrain/
  tiles/
    z/x/y.bin
  metadata/
    tileset.json
```

## Optional Backend Service

Use a backend only if we need:
- Caching and range requests for large tiles.
- Authentication / access control.
- On‑the‑fly reprojection or resampling.
- Stitching and pyramid generation for custom terrain levels.

Backend can expose:
- `/terrain/tiles/{z}/{x}/{y}.bin`
- `/terrain/tileset.json`
- `/terrain/status` (download/progress)

### Auto-download (server)
The terrain backend can optionally auto-download DEM COGs on startup.

Environment:
- `TERRAIN_AUTO_DOWNLOAD=1`
- `TERRAIN_COLLECTION=<stac-collection-id>`
- `TERRAIN_BBOX=minLon,minLat,maxLon,maxLat`
- `TERRAIN_LIMIT=200`

Progress is exposed via `/terrain/status`.

## STAC Usage (Discovery)

STAC helps with:
- Searching by bbox/time.
- Enumerating tile URLs.
- Metadata on elevation tiles.

We should use STAC **only for indexing**, then keep a local cached index for fast runtime access.

## Runtime Integration (Viewer)

1) On camera move, determine visible terrain tiles.
2) Fetch tiles via local static host (or backend).
3) Build or update terrain mesh.
4) Sample heights for layer‑0 (world base) and for features.

### Backend detection & fallback
- The web UI probes `GET /healthz` on the terrain backend.
- If available, the UI uses backend endpoints such as `/terrain/*` and `/stac/*`.
- If unavailable, the UI falls back to built-in assets and the public STAC catalog.

## Performance Considerations
- Tile caching (LRU) in memory.
- Level‑of‑detail selection by screen‑space error.
- Avoid fetching high‑resolution tiles when zoomed out.

## Next Steps
1) Implement STAC tile discovery + local cache tool.
2) Define tile format and convert COG → tiles.
3) Add terrain tile loader in viewer.
4) Wire layer‑0 height sampling.
