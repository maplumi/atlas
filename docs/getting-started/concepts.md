# Core Concepts

## Scene
A scene is a reproducible artifact that defines:
- Datasets (chunks)
- Programs (symbolization/behavior)
- Temporal metadata
- Initial view state

## Layer
A layer is a data/render/analysis adapter:
- Raster, vector, terrain, objects, labels
- Each layer participates in streaming, rendering, and queries

## Time window
All queries are implicitly time-aware:
- Active features are those overlapping the engine time window.

## Program
Programs control:
- Symbolization (color/size/visibility)
- Layer behavior hooks (future)
Programs are deterministic and sandboxed.
