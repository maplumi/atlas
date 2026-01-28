# Determinism and Replay

Determinism is a first-class requirement.

Atlas aims for **semantic determinism**:

Given the same dataset package(s), initial state, program inputs, and input stream, the engine must produce identical results. The engine may differ in *incidental* outputs (for example, debug text formatting) as long as the semantic scene state and query results remain identical.

## Determinism targets

Targets:
- Stable iteration ordering
- Stable decoding and canonicalization
- Deterministic scheduling (runtime + streaming + compute)
- Stable float behavior for ordering and keys

## Stable iteration ordering

Rules:
- Any API that returns a collection of entities/hits must define its ordering contract.
- Prefer **ascending `EntityId`** ordering when returning entity sets.
- Avoid relying on unordered container iteration order.

Current contract points:
- `scene::World` stores components in index-addressed vectors; iteration by index is deterministic.
- Query output must remain deterministic even when filtering/indexing implementations evolve.

## Stable floats policy

Floating-point math is required, but float *ordering* must never rely on partial ordering.

Rules:
- Never sort or key on raw `f32`/`f64` using partial ordering.
- When ordering is required (sorting, ordered keys), use total ordering.
- Canonicalize `-0.0` and NaNs to avoid hidden representational variance.

Implementation hook:
- `foundation::math::precision::{StableF64, stable_total_cmp_f64, canonical_f64}`

Notes:
- Geometry formats should prefer quantized/canonical representations at ingest boundaries.
- The CPU uses `f64` for authoritative coordinates; the GPU may use camera-relative `f32`.

## Deterministic scheduling

Determinism requires that scheduling does not depend on incidental ordering.

Rules:
- All queued work must have a stable, total ordering key.
- Equal-priority work must run in a deterministic tie-break order (for example, insertion order).
- Cancellation must not perturb the remaining queue order.
- Frame budgeting must be expressed in deterministic units (not wall-clock time).

Implementation hooks:
- `runtime::Scheduler` orders jobs by `(priority, id, insertion_order)`.
- `runtime::FrameBudget` provides deterministic time-slicing in abstract work units.
- `runtime::WorkQueue` orders tasks by `(priority, id)` where `id` is insertion order, supports backpressure (`try_push*`) and budgeted popping.

## Replay

Replay is the practical validation mechanism for determinism.

Minimum replay contract:

- Record fixed timestep frames (index + `dt_s`).
- Record inputs/events that mutate state.
- Replaying must produce identical semantic results.

Current hook:
- `runtime::Frame` is intentionally pure and deterministic.
