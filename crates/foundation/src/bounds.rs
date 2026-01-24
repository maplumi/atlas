/// Axis-aligned bounding boxes
#[derive(Copy, Clone, Debug)]
pub struct Aabb2 {
    pub min: [f64; 2],
    pub max: [f64; 2],
}
#[derive(Copy, Clone, Debug)]
pub struct Aabb3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb2 {
    pub fn new(min: [f64; 2], max: [f64; 2]) -> Self {
        Aabb2 { min, max }
    }
}
impl Aabb3 {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Aabb3 { min, max }
    }
}
