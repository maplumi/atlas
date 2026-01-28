/// Axis-aligned bounding boxes
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb2 {
    pub min: [f64; 2],
    pub max: [f64; 2],
}
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb2 {
    pub fn new(min: [f64; 2], max: [f64; 2]) -> Self {
        Aabb2 { min, max }
    }

    pub fn contains_point(&self, p: [f64; 2]) -> bool {
        p[0] >= self.min[0] && p[0] <= self.max[0] && p[1] >= self.min[1] && p[1] <= self.max[1]
    }

    pub fn intersects(&self, other: &Self) -> bool {
        !(self.max[0] < other.min[0]
            || self.min[0] > other.max[0]
            || self.max[1] < other.min[1]
            || self.min[1] > other.max[1])
    }
}
impl Aabb3 {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Aabb3 { min, max }
    }

    pub fn contains_point(&self, p: [f64; 3]) -> bool {
        p[0] >= self.min[0]
            && p[0] <= self.max[0]
            && p[1] >= self.min[1]
            && p[1] <= self.max[1]
            && p[2] >= self.min[2]
            && p[2] <= self.max[2]
    }

    pub fn intersects(&self, other: &Self) -> bool {
        !(self.max[0] < other.min[0]
            || self.min[0] > other.max[0]
            || self.max[1] < other.min[1]
            || self.min[1] > other.max[1]
            || self.max[2] < other.min[2]
            || self.min[2] > other.max[2])
    }

    pub fn expand_to_include(&mut self, p: [f64; 3]) {
        self.min[0] = self.min[0].min(p[0]);
        self.min[1] = self.min[1].min(p[1]);
        self.min[2] = self.min[2].min(p[2]);
        self.max[0] = self.max[0].max(p[0]);
        self.max[1] = self.max[1].max(p[1]);
        self.max[2] = self.max[2].max(p[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::{Aabb2, Aabb3};

    #[test]
    fn aabb3_contains_and_intersects() {
        let a = Aabb3::new([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        assert!(a.contains_point([0.0, 0.5, -0.5]));
        assert!(!a.contains_point([2.0, 0.0, 0.0]));

        let b = Aabb3::new([0.5, 0.5, 0.5], [2.0, 2.0, 2.0]);
        let c = Aabb3::new([2.1, 2.1, 2.1], [3.0, 3.0, 3.0]);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn aabb2_contains_and_intersects() {
        let a = Aabb2::new([0.0, 0.0], [10.0, 10.0]);
        assert!(a.contains_point([5.0, 5.0]));
        assert!(!a.contains_point([-1.0, 5.0]));

        let b = Aabb2::new([10.0, 10.0], [11.0, 11.0]);
        let c = Aabb2::new([10.1, 0.0], [11.0, 1.0]);
        assert!(a.intersects(&b)); // touching counts
        assert!(!a.intersects(&c));
    }
}
