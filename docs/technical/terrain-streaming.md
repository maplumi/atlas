# Streaming Terrain Strategy (Atlas)

This document describes a production-grade approach for scalable terrain in Atlas using on-demand sampling of Copernicus DEM COGs via STAC and building a multi-resolution tile pyramid. The goal is to avoid multi‑TB downloads and store only derived terrain tiles.

## Goals

- Minimize data footprint and bandwidth.
- Keep client rendering fast with view-dependent LOD.
- Support AOI-first (high-resolution only where needed).
- Make the pipeline cacheable and cloud‑friendly.

## Core Principles

1. **Never download full DEMs**: read only needed windows from COGs via HTTP range requests.
2. **Multi‑LOD pyramid**: globe renders different resolutions per zoom.
3. **Tiles, not raws**: store derived tiles/meshes, not source DEMs.
4. **AOI-first**: generate high-res only for target areas.

## Default Surface (Fast Load)

Atlas ships a lightweight base surface from `assets/world.json` (land polygons). This loads instantly and provides a drape‑friendly globe even when terrain is disabled.

To avoid chopped polygons at the antimeridian, Atlas unwraps polygon longitudes at load time so rings remain continuous before triangulation.

For faster startup in WebGPU, Atlas can also load a **pre‑tessellated surface tile pyramid** from the backend (`/surface/tileset.json` plus `/surface/tiles/{z}/{x}/{y}.bin`). Use the `atlas surface-tiles` tool to generate tiles from GeoJSON (writes `tileset.json` plus `tiles/{z}/{x}/{y}.bin` triangle buffers in viewer coordinates), then place the output under `data/terrain/surface` (not committed) and point the server at it via `SURFACE_ROOT`.

## Data Sources

- **STAC** for discovery (Copernicus DEM 30m STAC).
- **COG** access for windowed reads (GDAL / Rasterio).

## Proposed LOD Pyramid (baseline)

| Zoom | Approx Resolution | Coverage |
|------|-------------------|----------|
| Z0–Z4 | 5–10 km           | Global   |
| Z5–Z8 | 500 m – 1 km      | Continents |
| Z9–Z11 | 90–250 m         | Countries |
| Z12+ | 30 m              | AOIs only |

## Tile Formats

Pick one (recommended order):

1. **Quantized-Mesh** (Cesium-compatible, efficient)
2. **Heightmap tiles** (PNG / LERC / WebP)
3. **Custom mesh tiles** (Atlas-native)

## Architecture

```
STAC index  ->  windowed COG reads  ->  DEM resampling
                                         | 
                                         v
                               tile pyramid generator
                                         |
                                         v
                               object storage (tiles)
                                         |
                                         v
                               Atlas server -> client
```

### Storage

- Use object storage (S3 / MinIO / filesystem cache).
- Only store tiles; raw DEM remains remote.
- Cache hot tiles locally with TTL.

### Processing

- Batch job for global low‑LOD pyramid.
- AOI jobs for high‑LOD tiles.
- Incremental updates by tile key.

### Serving

- Server exposes `/{z}/{x}/{y}` tile endpoints.
- Client requests tiles based on camera & zoom.

## Implementation Plan (Atlas)

### Phase 1 — Minimal streaming prototype

- Add a **tile request API** in the server: `GET /terrain/{z}/{x}/{y}`.
- On request:
  - Compute tile bbox.
  - Query STAC for intersecting COGs.
  - Window-read pixels via GDAL/Rasterio.
  - Resample to tile resolution.
  - Emit a heightmap tile (PNG/LERC) or mesh.
  - Cache result (disk + memory) with TTL.

### Phase 2 — Offline pyramid builder

- Create a `terrain_tiler` tool:
  - Generate Z0–Z4 globally.
  - Generate Z5+ for AOIs.
  - Save tiles in object storage.

### Phase 3 — Production

- Store tiles in S3/MinIO, serve via CDN.
- Add tile metadata (tileset.json + bounds).
- Add cache warming & invalidation.

## Atlas Code Touchpoints (expected)

- **Server**
  - New terrain tile endpoint.
  - STAC query helper + COG windowed read.
  - Cache layer (filesystem + memory).

- **Formats**
  - Add/extend terrain tile format (heightmap or mesh).

- **Web/Native**
  - Request tiles per view; decode tile format.
  - LOD selection on camera distance.

## Defaults for Local Testing

- Small AOI bbox (e.g., `36.7,-1.6,37.2,-1.1`).
- Z0–Z8 only to keep build time low.
- Use local filesystem cache before S3.

## Open Questions

- Preferred tile format for Atlas (`TCH` vs quantized-mesh)?
- Caching policy & storage backend?
- Target max LOD per AOI?

## Next Steps

1. Choose tile format.
2. Implement server tile endpoint with COG window reads.
3. Add a minimal tiler CLI for AOIs.
4. Integrate client-side LOD selection.
