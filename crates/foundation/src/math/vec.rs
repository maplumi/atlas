#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

#[cfg(test)]
mod tests {
    use super::{Vec2, Vec3};

    #[test]
    fn vec2_add_sub() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(-0.5, 4.0);
        assert_eq!(a + b, Vec2::new(0.5, 6.0));
        assert_eq!(a - b, Vec2::new(1.5, -2.0));
    }

    #[test]
    fn vec3_add_sub_dot() {
        let a = Vec3::new(1.0, 2.0, -1.0);
        let b = Vec3::new(0.5, -2.0, 3.0);
        assert_eq!(a + b, Vec3::new(1.5, 0.0, 2.0));
        assert_eq!(a - b, Vec3::new(0.5, 4.0, -4.0));
        assert_eq!(a.dot(b), -6.5);
    }
}
