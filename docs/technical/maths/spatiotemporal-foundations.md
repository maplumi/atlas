# Spatiotemporal Foundations

Atlas queries are defined as intersections over four domains:
- Spatial
- Temporal
- Visibility
- Attributes

Query = Spatial ∩ Temporal ∩ Visibility ∩ Attribute

Temporal membership uses interval overlap:

I = [t_start, t_end]

Active in [t1, t2] if:

I ∩ [t1, t2] ≠ ∅
