# System Overview

Atlas is a spatiotemporal compute engine with:
- Deterministic runtime scheduling
- Spatial + temporal indexing
- Streaming + caching with explicit residency states
- WebGPU renderer
- Compute subsystem for programs and analysis

Atlas treats scenes as reproducible scientific artifacts (scene packages).

## End-to-end workflow (current)

This project is converging on a single core workflow that supports both analysis and visualization.

### 1) Ingest (data enters the system)
- Inputs (today): scene packages (manifest + chunks) and GeoJSON uploads.
- Parser/decoder lives in `crates/formats`.
- Ingestion standardizes into a `scene::World`:
  - Geometry becomes `VectorGeometry` entities.
  - Each entity gets a `ComponentTimeSpan` (defaulting to `TimeSpan::forever()` if no time is present).
  - Each entity gets `ComponentProperties` (key/value pairs from the source feature).

### 2) Standardize + store (internal representation)
- The `scene::World` is the internal “standard” representation for spatiotemporal features.
- Time is always present via `ComponentTimeSpan`.
- Attributes are available via `ComponentProperties`.

### 3) Index + cache (fast retrieval)
- Next step: persistent spatial indexes over standardized worlds and store them in `streaming` cache/residency.
- Next step: dataset versioning and compact chunk formats in `formats` + `streaming` cache keys.

### 4) Query + filter (core API)
- Query/filter APIs live in core crates (not the apps):
  - `layers::query::query_vector` supports filtering by geometry kind, time, bounding box, and properties.

### 5) Analyze (core API)
- Analysis utilities live in `crates/compute`.

### 6) Render (2D + 3D)
- Apps own UI and presentation, but do not re-define query/symbology/analysis.

## AI-native target

Long-term: attach an AI model that converts natural-language requests into:
1. a core query (filters + time window),
2. one or more compute analyses (spatial/temporal/statistical), and
3. a renderable overlay/layer result.
