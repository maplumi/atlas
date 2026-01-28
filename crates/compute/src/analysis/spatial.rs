use foundation::bounds::Aabb3;
use foundation::math::Vec3;

pub struct SpatialAnalysis;

impl SpatialAnalysis {
    pub fn aabb3_points(points: &[Vec3]) -> Option<Aabb3> {
        let first = points.first()?;
        let mut min = [first.x, first.y, first.z];
        let mut max = [first.x, first.y, first.z];
        for p in points.iter().skip(1) {
            min[0] = min[0].min(p.x);
            min[1] = min[1].min(p.y);
            min[2] = min[2].min(p.z);
            max[0] = max[0].max(p.x);
            max[1] = max[1].max(p.y);
            max[2] = max[2].max(p.z);
        }
        Some(Aabb3::new(min, max))
    }

    /// Returns (index, squared distance).
    pub fn nearest_point(points: &[Vec3], target: Vec3) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in points.iter().enumerate() {
            let dx = p.x - target.x;
            let dy = p.y - target.y;
            let dz = p.z - target.z;
            let d2 = dx * dx + dy * dy + dz * dz;
            if best.map(|(_, bd2)| d2 < bd2).unwrap_or(true) {
                best = Some((i, d2));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::SpatialAnalysis;
    use foundation::math::Vec3;

    #[test]
    fn nearest_point_picks_closest() {
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)];
        let (i, d2) = SpatialAnalysis::nearest_point(&pts, Vec3::new(9.0, 0.0, 0.0)).unwrap();
        assert_eq!(i, 1);
        assert!(d2 < 2.0);
    }
}
