use foundation::math::{Vec3, WGS84_A, WGS84_B};

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Shape3D {
    Cube { size: f64 },
    Sphere { radius: f64 },
    Ellipsoid { radii: Vec3 },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Drawable3D {
    pub shape: Shape3D,
}

impl Drawable3D {
    pub fn cube(size: f64) -> Self {
        Self {
            shape: Shape3D::Cube { size },
        }
    }

    pub fn sphere(radius: f64) -> Self {
        Self {
            shape: Shape3D::Sphere { radius },
        }
    }

    pub fn ellipsoid(radii: Vec3) -> Self {
        Self {
            shape: Shape3D::Ellipsoid { radii },
        }
    }

    pub fn wgs84_globe() -> Self {
        Self::ellipsoid(Vec3::new(WGS84_A, WGS84_A, WGS84_B))
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawable3D, Shape3D};

    #[test]
    fn create_sphere_drawable() {
        let drawable = Drawable3D::sphere(1.5);
        assert!(matches!(drawable.shape, Shape3D::Sphere { .. }));
    }

    #[test]
    fn create_wgs84_globe_drawable() {
        let drawable = Drawable3D::wgs84_globe();
        assert!(matches!(drawable.shape, Shape3D::Ellipsoid { .. }));
    }
}
