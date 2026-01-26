use foundation::math::Vec2;

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Shape2D {
    Rect { size: Vec2 },
    Circle { radius: f64 },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Drawable2D {
    pub shape: Shape2D,
}

impl Drawable2D {
    pub fn rect(size: Vec2) -> Self {
        Self {
            shape: Shape2D::Rect { size },
        }
    }

    pub fn circle(radius: f64) -> Self {
        Self {
            shape: Shape2D::Circle { radius },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawable2D, Shape2D};
    use foundation::math::Vec2;

    #[test]
    fn create_rect_drawable() {
        let drawable = Drawable2D::rect(Vec2::new(2.0, 3.0));
        assert!(matches!(drawable.shape, Shape2D::Rect { .. }));
    }
}
