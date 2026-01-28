# Spatial Indexing

Implemented indices:
- BVH for AABB queries and 3D/picking-style pruning (`scene::spatial::Bvh`)

Planned indices:
- quadtree for 2D queries (not yet implemented)

Goal: fast pruning + deterministic results.

Determinism contract:
- BVH queries return hits in ascending `EntityId` order.
- BVH build is deterministic (stable float ordering + stable tie-breaks).
