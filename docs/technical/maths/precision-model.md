# Precision Model

Principle:
- CPU authoritative coordinates in f64
- GPU camera-relative coordinates in f32

Goal: avoid global precision collapse at Earth scale.

## Camera-relative positions

When sending positions to the GPU:
- choose a high-precision origin (typically the camera eye or target),
- compute offsets in `f64` as `world - origin`,
- cast the offsets to `f32`.

This keeps GPU values near zero and preserves detail even at Earth scale.

Code hook:
- `foundation::math::precision::CameraRelative` and `foundation::math::precision::camera_relative_f32`

## Deterministic float policy

Atlas distinguishes between:
- doing floating point math (allowed and expected), and
- ordering/keying floats (must be deterministic).

Rules:
- Never sort or key on raw floats via partial ordering.
- When ordering is required, use total ordering and canonicalization.

Code hook:
- `foundation::math::precision::{StableF64, stable_total_cmp_f64, canonical_f64}`
