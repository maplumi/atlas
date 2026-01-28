#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayerStyle {
    pub visible: bool,
    pub color: [f32; 4],
    /// Lift as a fraction of Earth radius (WGS84_A).
    pub lift: f32,
}

impl LayerStyle {
    pub const fn new(visible: bool, color: [f32; 4], lift: f32) -> Self {
        Self {
            visible,
            color,
            lift,
        }
    }
}

impl Default for LayerStyle {
    fn default() -> Self {
        Self {
            visible: true,
            color: [1.0, 1.0, 1.0, 1.0],
            lift: 0.0,
        }
    }
}
