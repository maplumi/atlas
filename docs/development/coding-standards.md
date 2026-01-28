# Coding Standards

## Principles
- deterministic behavior by default
- stable ordering
- avoid heap allocation in hot paths
- document invariants and performance assumptions

## Determinism checklist

Ordering:
- Do not rely on iteration order of unordered containers.
- Any API returning a list/set must document its ordering contract.
- Prefer ascending `EntityId` ordering for entity collections.

Floats:
- Do not sort/key on raw `f32`/`f64` using partial ordering.
- Use `foundation::math::precision::StableF64` (or `stable_total_cmp_f64`) when ordering is required.

Scheduling:
- Any work queue must have a stable, total ordering key.
- Equal-priority work must have a deterministic tie-break (typically insertion order).
- Budgeting/time-slicing must use deterministic "work units" (avoid wall-clock checks in core scheduling).

Metrics:
- Metrics must be deterministic to record/replay and compare runs (stable key ordering; no wall-clock time).
- Metrics must not be used to gate semantics (never change behavior based on metrics values).

Handles:
- Use generational handles for IDs that can be deleted/reused.
- Prefer `foundation::handles::HandleAllocator` over ad-hoc index reuse.

Arenas:
- Prefer `foundation::Arena<T>` for owning, reusable storage that needs stable IDs.
- Treat `Handle` values as arena-local; never mix handles across arenas.
- Freeing increments the generation; stale handles must fail lookups.

GPU precision:
- Prefer camera-relative GPU positions (subtract a high-precision origin in `f64`, then cast to `f32`).
- Use `foundation::math::precision::CameraRelative` helpers for this pattern.
