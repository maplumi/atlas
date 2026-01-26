use foundation::math::Vec3;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Transform {
    pub position: Vec3,
}

impl Transform {
    pub fn identity() -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 0.0),
        }
    }

    pub fn translate(position: Vec3) -> Self {
        Self { position }
    }
}

#[cfg(test)]
mod tests {
    use super::Transform;
    use foundation::math::Vec3;

    #[test]
    fn identity_is_origin() {
        let transform = Transform::identity();
        assert_eq!(transform.position, Vec3::new(0.0, 0.0, 0.0));
    }
}
