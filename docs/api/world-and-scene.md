# World and Scene API

The `scene::World` is the canonical in-memory representation of standardized spatiotemporal data.

## Core components (current)
- bounds
- time spans
- properties
- vector geometry

## Implemented
- selection sets: `scene::selection::SelectionSet`
- indices: `scene::spatial::Bvh`, `scene::temporal::IntervalTree`
- unified query API: `scene::query::VectorEntityQuery` + `scene::query::query_vector_entities`
- picking: `scene::picking::pick_ray` + `scene::picking::pick_screen`
- visibility culling: `scene::visibility::Frustum` + `scene::visibility::cull_vector_entities_in_frustum`
