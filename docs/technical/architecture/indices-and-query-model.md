# Indices and Query Model

Atlas queries are intersections over:
- spatial
- temporal
- visibility
- attributes

Implemented core entry point:
- `scene::query::query_vector_entities` (spatial + temporal + attribute; deterministic ordering)

Implemented visibility culling:
- `scene::visibility::cull_vector_entities_in_frustum` (frustum vs entity bounds; deterministic ordering)

Planned indices:
- spatial: quadtree/BVH (BVH implemented in `scene::spatial::Bvh`)
- temporal: interval tree (implemented in `scene::temporal::IntervalTree`)
- attribute: dictionary/bitmap indices
