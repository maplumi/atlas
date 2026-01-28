# Temporal Indexing

Temporal membership is interval overlap.

Implemented index:
- interval tree for active-set queries over a time window (`scene::temporal::IntervalTree`)

Determinism contract:
- Queries return hits in ascending `EntityId` order.
- Build is deterministic (stable float ordering + stable tie-breaks).
