use foundation::math::Vec3;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ComponentBounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl ComponentBounds {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentBounds;
    use foundation::math::Vec3;

    #[test]
    fn contains_point_inside() {
        let bounds = ComponentBounds::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        assert!(bounds.contains(Vec3::new(0.5, 0.0, -0.5)));
    }

    #[test]
    fn rejects_point_outside() {
        let bounds = ComponentBounds::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        assert!(!bounds.contains(Vec3::new(2.0, 0.0, 0.0)));
    }
}
