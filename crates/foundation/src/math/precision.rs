//! Precision policies.
//!
//! This module is intentionally small and conservative. It provides:
//! - A canonical CPU precision type (`HighPrecision`).
//! - A deterministic float ordering wrapper (`StableF64`) for sorting and keys.

use core::cmp::Ordering;

use super::Vec3;

/// CPU-authoritative precision type.
pub type HighPrecision = f64;

/// GPU-friendly, camera-relative position in `f32`.
///
/// Convention: positions sent to the GPU should typically be expressed as
/// `world_pos - camera_origin`, then cast to `f32`.
pub type CameraRelativeF32 = [f32; 3];

/// Camera-relative precision model.
///
/// Store a high-precision `origin` (typically the camera eye or target), and
/// express all GPU positions relative to it.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct CameraRelative {
    pub origin: Vec3,
}

impl CameraRelative {
    pub fn new(origin: Vec3) -> Self {
        Self { origin }
    }

    /// Convert a world-space point (f64) to a camera-relative `f32` offset.
    #[inline]
    pub fn to_f32(self, world: Vec3) -> CameraRelativeF32 {
        let d = world - self.origin;
        [d.x as f32, d.y as f32, d.z as f32]
    }
}

/// Convert a world-space point (f64) to a camera-relative `f32` offset.
#[inline]
pub fn camera_relative_f32(world: Vec3, origin: Vec3) -> CameraRelativeF32 {
    CameraRelative::new(origin).to_f32(world)
}

/// Canonicalize a floating-point value for deterministic ordering.
///
/// Rules:
/// - `-0.0` becomes `0.0`
/// - all NaNs become a single canonical NaN
pub fn canonical_f64(v: f64) -> f64 {
    if v == 0.0 {
        // Handles +0.0 and -0.0.
        0.0
    } else if v.is_nan() {
        f64::NAN
    } else {
        v
    }
}

/// Deterministic total ordering for floats.
///
/// Prefer this any time you sort floats or use them in ordered keys.
pub fn stable_total_cmp_f64(a: f64, b: f64) -> Ordering {
    canonical_f64(a).total_cmp(&canonical_f64(b))
}

/// A float wrapper with a deterministic total ordering.
///
/// - Uses `f64::total_cmp` (after canonicalization) for `Ord`.
/// - Treats NaN as equal to NaN for `Eq` (after canonicalization), enabling
///   use in deterministic ordered structures.
#[derive(Debug, Copy, Clone, Default)]
pub struct StableF64(pub f64);

impl PartialEq for StableF64 {
    fn eq(&self, other: &Self) -> bool {
        stable_total_cmp_f64(self.0, other.0) == Ordering::Equal
    }
}

impl Eq for StableF64 {}

impl PartialOrd for StableF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StableF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        stable_total_cmp_f64(self.0, other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CameraRelative, StableF64, camera_relative_f32, canonical_f64, stable_total_cmp_f64,
    };
    use core::cmp::Ordering;

    use crate::math::Vec3;

    #[test]
    fn canonicalizes_negative_zero() {
        assert_eq!(canonical_f64(-0.0), 0.0);
        assert_eq!(canonical_f64(0.0), 0.0);
    }

    #[test]
    fn stable_cmp_is_total_and_deterministic() {
        assert_eq!(stable_total_cmp_f64(1.0, 2.0), Ordering::Less);
        assert_eq!(stable_total_cmp_f64(f64::NAN, f64::NAN), Ordering::Equal);
        assert!(StableF64(f64::NAN) == StableF64(f64::NAN));
    }

    #[test]
    fn camera_relative_preserves_small_offsets() {
        // Large Earth-scale magnitudes, small delta.
        let origin = Vec3::new(6_378_137.0, -2_000_000.0, 1_000_000.0);
        let world = Vec3::new(6_378_138.25, -2_000_001.0, 999_999.5);
        let rel = CameraRelative::new(origin).to_f32(world);
        assert_eq!(rel, [1.25, -1.0, -0.5]);

        let rel2 = camera_relative_f32(world, origin);
        assert_eq!(rel2, rel);
    }
}
