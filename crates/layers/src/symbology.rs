#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayerStyle {
    pub visible: bool,
    /// Primary color. For polygons this is the fill color; for points/lines it is the main stroke.
    pub color: [f32; 4],
    /// Optional stroke (outline) color for polygon rendering.
    pub stroke_color: [f32; 4],
    /// Optional stroke width in pixels for polygon outlines. 0 disables outlines.
    pub stroke_width_px: f32,
    /// Optional point size override in pixels. 0 uses the global point-size setting.
    pub size_px: f32,
    /// Optional line width override in pixels. 0 uses the global line-width setting.
    pub width_px: f32,
    /// Lift as a fraction of Earth radius (WGS84_A).
    pub lift: f32,
}

impl LayerStyle {
    pub const fn new(visible: bool, color: [f32; 4], lift: f32) -> Self {
        Self {
            visible,
            color,
            stroke_color: color,
            stroke_width_px: 0.0,
            size_px: 0.0,
            width_px: 0.0,
            lift,
        }
    }
}

impl Default for LayerStyle {
    fn default() -> Self {
        Self {
            visible: true,
            color: [1.0, 1.0, 1.0, 1.0],
            stroke_color: [1.0, 1.0, 1.0, 1.0],
            stroke_width_px: 0.0,
            size_px: 0.0,
            width_px: 0.0,
            lift: 0.0,
        }
    }
}
