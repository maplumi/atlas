#[derive(Debug, Copy, Clone, Default)]
pub struct WgpuPerfSnapshot {
    pub upload_calls: u32,
    pub upload_bytes: u64,
    pub render_passes: u32,
    pub draw_calls: u32,
    pub draw_instances: u64,
    pub draw_vertices: u64,
    pub draw_indices: u64,
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use ::wgpu::util::DeviceExt;
    use std::borrow::Cow;
    use std::cell::Cell;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    use foundation::math::{WGS84_A, WGS84_B};

    #[derive(Debug)]
    pub struct WgpuContext {
        pub _instance: &'static ::wgpu::Instance,
        pub surface: ::wgpu::Surface<'static>,
        pub device: ::wgpu::Device,
        pub queue: ::wgpu::Queue,
        pub config: ::wgpu::SurfaceConfiguration,
        pub _canvas: web_sys::HtmlCanvasElement,
        pub clear_color: ::wgpu::Color,
        pub globe_color: [f32; 3],
        pub globe_alpha: f32,
        pub globe_transparent: bool,
        pub stars_alpha: f32,
        pub stars_pipeline: ::wgpu::RenderPipeline,
        pub stars_count: u32,
        pub globe_pipeline_solid: ::wgpu::RenderPipeline,
        pub globe_pipeline_transparent: ::wgpu::RenderPipeline,
        pub graticule_pipeline: ::wgpu::RenderPipeline,
        pub cities_pipeline: ::wgpu::RenderPipeline,
        pub corridors_pipeline: ::wgpu::RenderPipeline,
        pub overlays_pipeline: ::wgpu::RenderPipeline,
        pub base_overlays_pipeline: ::wgpu::RenderPipeline,
        pub terrain_pipeline: ::wgpu::RenderPipeline,

        // 2D (Web Mercator) pipelines.
        pub map2d_polys_pipeline: ::wgpu::RenderPipeline,
        pub map2d_lines_pipeline: ::wgpu::RenderPipeline,
        pub map2d_points_pipeline: ::wgpu::RenderPipeline,

        // 2D globals + bind group (shares the same bind group layout as 3D).
        pub uniform2d_buffer: ::wgpu::Buffer,
        pub uniform2d_bind_group: ::wgpu::BindGroup,
        pub uniform_buffer: ::wgpu::Buffer,
        pub styles_buffer: ::wgpu::Buffer,
        pub styles_capacity_bytes: u64,
        pub styles_count: u32,
        pub uniform_bind_group_layout: ::wgpu::BindGroupLayout,
        pub uniform_bind_group: ::wgpu::BindGroup,
        pub depth_view: ::wgpu::TextureView,
        pub vertex_buffer: ::wgpu::Buffer,
        pub index_buffer: ::wgpu::Buffer,
        pub index_count: u32,
        pub graticule_vertex_buffer: ::wgpu::Buffer,
        pub graticule_vertex_count: u32,
        pub cities_quad_vertex_buffer: ::wgpu::Buffer,
        pub cities_instance_buffer: ::wgpu::Buffer,
        pub cities_instance_capacity_bytes: u64,
        pub cities_instance_count: u32,
        pub corridor_quad_vertex_buffer: ::wgpu::Buffer,
        pub corridors_instance_buffer: ::wgpu::Buffer,
        pub corridors_instance_capacity_bytes: u64,
        pub corridors_instance_count: u32,
        pub regions_vertex_buffer: ::wgpu::Buffer,
        pub regions_vertex_capacity_bytes: u64,
        pub regions_vertex_count: u32,
        pub terrain_vertex_buffer: ::wgpu::Buffer,
        pub terrain_vertex_capacity_bytes: u64,
        pub terrain_vertex_count: u32,
        pub base_regions_vertex_buffer: ::wgpu::Buffer,
        pub base_regions_vertex_capacity_bytes: u64,
        pub base_regions_vertex_count: u32,

        // 2D geometry buffers (Mercator meters).
        pub base_regions2d_vertex_buffer: ::wgpu::Buffer,
        pub base_regions2d_vertex_capacity_bytes: u64,
        pub base_regions2d_vertex_count: u32,
        pub regions2d_vertex_buffer: ::wgpu::Buffer,
        pub regions2d_vertex_capacity_bytes: u64,
        pub regions2d_vertex_count: u32,
        pub points2d_instance_buffer: ::wgpu::Buffer,
        pub points2d_instance_capacity_bytes: u64,
        pub points2d_instance_count: u32,
        pub lines2d_instance_buffer: ::wgpu::Buffer,
        pub lines2d_instance_capacity_bytes: u64,
        pub lines2d_instance_count: u32,
        pub grid2d_instance_buffer: ::wgpu::Buffer,
        pub grid2d_instance_capacity_bytes: u64,
        pub grid2d_instance_count: u32,

        // Lightweight per-frame perf counters (reset externally per frame).
        pub perf_upload_calls: Cell<u32>,
        pub perf_upload_bytes: Cell<u64>,
        pub perf_render_passes: Cell<u32>,
        pub perf_draw_calls: Cell<u32>,
        pub perf_draw_instances: Cell<u64>,
        pub perf_draw_vertices: Cell<u64>,
        pub perf_draw_indices: Cell<u64>,
    }

    const GLOBE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    return VsOut(
        globals.view_proj * vec4<f32>(position, 1.0),
        normal,
    );
}

@fragment
fn fs_main(fs_in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(fs_in.normal);
    let l = normalize(globals.light_dir);
    let ndotl = max(dot(n, l), 0.0);

    // Simple globe-ish color ramp.
    let base = globals.globe_color;
    let shade = 0.25 + 0.75 * ndotl;
    return vec4<f32>(base * shade, globals.globe_alpha);
}
"#;

    const GRATICULE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return globals.view_proj * vec4<f32>(position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Light blue overlay lines (keep semi-transparent so it doesn't overpower data layers).
    return vec4<f32>(0.65, 0.85, 1.0, 0.35);
}
"#;

    const STARS_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

fn hash_u32(x_in: u32) -> u32 {
    // 32-bit integer mix (non-linear) to avoid visible correlation patterns.
    var x = x_in;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

fn hash01(x: u32) -> f32 {
    return f32(hash_u32(x)) / 4294967295.0;
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) a: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Deterministic pseudo-random star directions on the unit sphere.
    // These are rendered as an "infinite" background by using w=0 so camera
    // translation does not affect them, but camera rotation does.
    let rx = hash01(vid ^ 0x68bc21ebu);
    let ry = hash01(vid ^ 0x02e5be93u);
    let rb = hash01(vid ^ 0x9e3779b9u);

    // Sample uniformly on sphere via cos(theta) and phi.
    let z = ry * 2.0 - 1.0;
    let phi = 6.2831853 * rx;
    let r = sqrt(max(1.0 - z * z, 0.0));
    let dir = vec3<f32>(r * cos(phi), r * sin(phi), z);
    // Slightly vary brightness; keep faint stars common.
    // Keep overall brightness conservative to avoid a "snow" look.
    let a = 0.03 + 0.22 * rb * rb;

    // Project as a direction-at-infinity.
    var clip = globals.view_proj * vec4<f32>(dir, 0.0);
    // Push to far plane to keep it behind everything.
    clip = vec4<f32>(clip.x, clip.y, clip.w, clip.w);
    return VsOut(clip, a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, in.a * globals.stars_alpha);
}
"#;

    const MAP2D_SHADER: &str = r#"
struct Globals2D {
    center_m: vec2<f32>,
    scale_px_per_m: f32,
    world_width_m: f32,
    viewport_px: vec2<f32>,
    _pad0: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> globals2d: Globals2D;

struct Style {
    color: vec4<f32>,
    lift_m: f32,
    size_px: f32,
    width_px: f32,
    _pad0: f32,
};

@group(0) @binding(1)
var<storage, read> styles: array<Style>;

fn wrap_dx(dx: f32, ww: f32) -> f32 {
    // dx wrapped into [-ww/2, +ww/2] with Euclidean modulo.
    let t = (dx + 0.5 * ww) / ww;
    return (dx + 0.5 * ww) - ww * floor(t) - 0.5 * ww;
}

fn mercator_to_clip_from_anchor(x_m: f32, y_m: f32, anchor_x_m: f32) -> vec4<f32> {
    let ww = globals2d.world_width_m;
    let center_x = globals2d.center_m.x;
    let center_y = globals2d.center_m.y;

    let anchor_adj_x = center_x + wrap_dx(anchor_x_m - center_x, ww);
    let x_adj = anchor_adj_x + (x_m - anchor_x_m);
    let dx_px = (x_adj - center_x) * globals2d.scale_px_per_m;
    let dy_px = (y_m - center_y) * globals2d.scale_px_per_m;

    let ndc_x = dx_px / (globals2d.viewport_px.x * 0.5);
    // Positive Mercator Y is north; positive NDC Y is up -> same sign for north-up orientation
    let ndc_y = dy_px / (globals2d.viewport_px.y * 0.5);
    return vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
}

fn mercator_to_clip(x_m: f32, y_m: f32) -> vec4<f32> {
    return mercator_to_clip_from_anchor(x_m, y_m, x_m);
}

struct PolyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) style_id: u32,
};

@vertex
fn vs_poly(
    @location(0) position_m: vec2<f32>,
    @location(1) anchor_x_m: f32,
    @location(2) style_id: u32,
) -> PolyOut {
    var out: PolyOut;
    out.pos = mercator_to_clip_from_anchor(position_m.x, position_m.y, anchor_x_m);
    out.style_id = style_id;
    return out;
}

@fragment
fn fs_poly(in: PolyOut) -> @location(0) vec4<f32> {
    return styles[in.style_id].color;
}

struct PointOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) style_id: u32,
};

@vertex
fn vs_point(
    @location(0) corner: vec2<f32>,
    @location(1) center_m: vec2<f32>,
    @location(2) style_id: u32,
) -> PointOut {
    let s = styles[style_id];
    let base = mercator_to_clip(center_m.x, center_m.y);
    let offset_px = corner * s.size_px;
    let offset_ndc = vec2<f32>(
        offset_px.x / (globals2d.viewport_px.x * 0.5),
        -offset_px.y / (globals2d.viewport_px.y * 0.5)
    );

    var out: PointOut;
    out.pos = vec4<f32>(base.xy + offset_ndc, 0.0, 1.0);
    out.style_id = style_id;
    return out;
}

@fragment
fn fs_point(in: PointOut) -> @location(0) vec4<f32> {
    return styles[in.style_id].color;
}

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) style_id: u32,
};

@vertex
fn vs_line(
    @location(0) along: f32,
    @location(1) side: f32,
    @location(2) a_m: vec2<f32>,
    @location(3) b_m: vec2<f32>,
    @location(4) style_id: u32,
) -> LineOut {
    let s = styles[style_id];
    let ww = globals2d.world_width_m;
    let cx = globals2d.center_m.x;

    // Keep segment local around the camera center.
    let ax_adj = cx + wrap_dx(a_m.x - cx, ww);
    let bx_adj = ax_adj + (b_m.x - a_m.x);

    let a_adj = vec2<f32>(ax_adj, a_m.y);
    let b_adj = vec2<f32>(bx_adj, b_m.y);
    let p = a_adj + (b_adj - a_adj) * along;

    let base = mercator_to_clip_from_anchor(p.x, p.y, ax_adj);

    let d_m = b_adj - a_adj;
    let d_px = d_m * globals2d.scale_px_per_m;
    let len = max(length(d_px), 1e-6);
    let dir = d_px / len;
    let perp = vec2<f32>(-dir.y, dir.x);
    let offset_px = perp * (side * 0.5 * s.width_px);
    let offset_ndc = vec2<f32>(
        offset_px.x / (globals2d.viewport_px.x * 0.5),
        -offset_px.y / (globals2d.viewport_px.y * 0.5)
    );

    var out: LineOut;
    out.pos = vec4<f32>(base.xy + offset_ndc, 0.0, 1.0);
    out.style_id = style_id;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return styles[in.style_id].color;
}
"#;

    const CITIES_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct Style {
    color: vec4<f32>,
    lift_m: f32,
    size_px: f32,
    width_px: f32,
    _pad0: f32,
};

@group(0) @binding(1)
var<storage, read> styles: array<Style>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn ellipsoid_normal(p: vec3<f32>) -> vec3<f32> {
    // Viewer coordinates use radii: x=A, y=B, z=A.
    let a: f32 = 6378137.0;
    let b: f32 = 6356752.314245179;
    let a2 = a * a;
    let b2 = b * b;
    return normalize(vec3<f32>(p.x / a2, p.y / b2, p.z / a2));
}

@vertex
fn vs_main(
    // Quad corner in [-1,+1] (per-vertex)
    @location(0) corner: vec2<f32>,
    // Per-instance
    @location(1) center: vec3<f32>,
    @location(2) style_id: u32,
) -> VsOut {
    let st = styles[style_id];
    let n = ellipsoid_normal(center);
    let world_center = center + n * st.lift_m;

    // Project the center, then offset in clip space so size stays constant in pixels.
    let clip_center = globals.view_proj * vec4<f32>(world_center, 1.0);
    let vp = max(globals.viewport, vec2<f32>(1.0, 1.0));
    let offset_px = corner * st.size_px;
    let ndc_offset = vec2<f32>(
        (offset_px.x * 2.0) / vp.x,
        (-offset_px.y * 2.0) / vp.y,
    );
    let clip = clip_center + vec4<f32>(ndc_offset * clip_center.w, 0.0, 0.0);
    return VsOut(clip, st.color);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

    const CORRIDORS_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct Style {
    color: vec4<f32>,
    lift_m: f32,
    size_px: f32,
    width_px: f32,
    _pad0: f32,
};

@group(0) @binding(1)
var<storage, read> styles: array<Style>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn ellipsoid_normal(p: vec3<f32>) -> vec3<f32> {
    // Viewer coordinates use radii: x=A, y=B, z=A.
    let a: f32 = 6378137.0;
    let b: f32 = 6356752.314245179;
    let a2 = a * a;
    let b2 = b * b;
    return normalize(vec3<f32>(p.x / a2, p.y / b2, p.z / a2));
}

@vertex
fn vs_main(
    // Per-vertex for the segment quad
    @location(0) along: f32,
    @location(1) side: f32,
    // Per-instance
    @location(2) a: vec3<f32>,
    @location(3) b: vec3<f32>,
    @location(4) style_id: u32,
) -> VsOut {
    let st = styles[style_id];
    let lift = st.lift_m;
    let width_px = st.width_px;
    // Apply lift along the ellipsoid normal at each endpoint.
    let a_world = a + ellipsoid_normal(a) * lift;
    let b_world = b + ellipsoid_normal(b) * lift;

    let clip_a = globals.view_proj * vec4<f32>(a_world, 1.0);
    let clip_b = globals.view_proj * vec4<f32>(b_world, 1.0);

    let ndc_a = clip_a.xy / max(clip_a.w, 1e-6);
    let ndc_b = clip_b.xy / max(clip_b.w, 1e-6);
    let dir = ndc_b - ndc_a;
    let dir_len = max(length(dir), 1e-6);
    let perp = vec2<f32>(-dir.y, dir.x) / dir_len;

    let half_w = 0.5 * width_px;
    let px_to_ndc = vec2<f32>(2.0 / globals.viewport.x, 2.0 / globals.viewport.y);
    let offset_ndc = perp * (half_w * px_to_ndc) * side;

    var clip = mix(clip_a, clip_b, along);
    let delta = offset_ndc * clip.w;
    clip = vec4<f32>(clip.x + delta.x, clip.y + delta.y, clip.z, clip.w);
    return VsOut(clip, st.color);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

    const OVERLAYS_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct Style {
    color: vec4<f32>,
    lift_m: f32,
    size_px: f32,
    width_px: f32,
    _pad0: f32,
};

@group(0) @binding(1)
var<storage, read> styles: array<Style>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn ellipsoid_normal(p: vec3<f32>) -> vec3<f32> {
    // Viewer coordinates use radii: x=A, y=B, z=A.
    let a: f32 = 6378137.0;
    let b: f32 = 6356752.314245179;
    let a2 = a * a;
    let b2 = b * b;
    return normalize(vec3<f32>(p.x / a2, p.y / b2, p.z / a2));
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) style_id: u32,
) -> VsOut {
    let st = styles[style_id];
    let n = ellipsoid_normal(position);
    let world_pos = position + n * st.lift_m;
    return VsOut(globals.view_proj * vec4<f32>(world_pos, 1.0), st.color);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

    const TERRAIN_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    globe_alpha: f32,
    _pad1: f32,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn ellipsoid_normal(p: vec3<f32>) -> vec3<f32> {
    let a: f32 = 6378137.0;
    let b: f32 = 6356752.314245179;
    let a2 = a * a;
    let b2 = b * b;
    return normalize(vec3<f32>(p.x / a2, p.y / b2, p.z / a2));
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) lift_m: f32,
    @location(2) color: vec4<f32>,
) -> VsOut {
    let world_pos = position + ellipsoid_normal(position) * lift_m;
    return VsOut(globals.view_proj * vec4<f32>(world_pos, 1.0), color);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Vertex {
        position: [f32; 3],
        normal: [f32; 3],
    }

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct LineVertex {
        position: [f32; 3],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct CityInstance {
        pub center: [f32; 3],
        pub style_id: u32,
    }

    pub type CityVertex = CityInstance;

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct OverlayVertex {
        pub position: [f32; 3],
        pub style_id: u32,
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct TerrainVertex {
        pub position: [f32; 3],
        pub lift_m: f32,
        pub color: [f32; 4],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct CorridorInstance {
        pub a: [f32; 3],
        pub _pad0: u32,
        pub b: [f32; 3],
        pub style_id: u32,
    }

    pub type CorridorVertex = CorridorInstance;

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct Style {
        pub color: [f32; 4],
        pub lift_m: f32,
        pub size_px: f32,
        pub width_px: f32,
        pub _pad0: f32,
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct Globals2D {
        pub center_m: [f32; 2],
        pub scale_px_per_m: f32,
        pub world_width_m: f32,
        pub viewport_px: [f32; 2],
        pub _pad0: [f32; 2],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct Overlay2DVertex {
        pub position_m: [f32; 2],
        pub anchor_x_m: f32,
        pub style_id: u32,
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct Point2DInstance {
        pub center_m: [f32; 2],
        pub style_id: u32,
        pub _pad0: u32,
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct Segment2DInstance {
        pub a_m: [f32; 2],
        pub b_m: [f32; 2],
        pub style_id: u32,
        pub _pad0: u32,
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct QuadVertex {
        corner: [f32; 2],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct SegmentVertex {
        along: f32,
        side: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Globals {
        view_proj: [[f32; 4]; 4],
        light_dir: [f32; 3],
        _pad0: f32,
        viewport: [f32; 2],
        globe_alpha: f32,
        _pad1: f32,
        globe_color: [f32; 3],
        stars_alpha: f32,
    }

    pub fn set_theme(
        ctx: &mut WgpuContext,
        clear_color: ::wgpu::Color,
        globe_color: [f32; 3],
        stars_alpha: f32,
    ) {
        ctx.clear_color = clear_color;
        ctx.globe_color = globe_color;
        ctx.stars_alpha = stars_alpha;
    }

    pub fn set_styles(ctx: &mut WgpuContext, styles: &[Style]) {
        let required_bytes = (styles.len() * std::mem::size_of::<Style>()) as u64;
        if required_bytes > ctx.styles_capacity_bytes {
            ensure_buffer_capacity(
                &ctx.device,
                &mut ctx.styles_buffer,
                &mut ctx.styles_capacity_bytes,
                required_bytes.max(std::mem::size_of::<Style>() as u64),
                ::wgpu::BufferUsages::STORAGE | ::wgpu::BufferUsages::COPY_DST,
                "atlas-styles",
            );

            ctx.uniform_bind_group = ctx.device.create_bind_group(&::wgpu::BindGroupDescriptor {
                label: Some("atlas-unified-bg"),
                layout: &ctx.uniform_bind_group_layout,
                entries: &[
                    ::wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.uniform_buffer.as_entire_binding(),
                    },
                    ::wgpu::BindGroupEntry {
                        binding: 1,
                        resource: ctx.styles_buffer.as_entire_binding(),
                    },
                ],
            });

            ctx.uniform2d_bind_group = ctx.device.create_bind_group(&::wgpu::BindGroupDescriptor {
                label: Some("atlas-unified-bg-2d"),
                layout: &ctx.uniform_bind_group_layout,
                entries: &[
                    ::wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.uniform2d_buffer.as_entire_binding(),
                    },
                    ::wgpu::BindGroupEntry {
                        binding: 1,
                        resource: ctx.styles_buffer.as_entire_binding(),
                    },
                ],
            });
        }

        if !styles.is_empty() {
            ctx.queue
                .write_buffer(&ctx.styles_buffer, 0, bytemuck::cast_slice(styles));
            perf_on_write(ctx, (styles.len() * std::mem::size_of::<Style>()) as u64);
        }
        ctx.styles_count = styles.len() as u32;
    }

    #[inline]
    fn perf_on_write(ctx: &WgpuContext, bytes: u64) {
        ctx.perf_upload_calls
            .set(ctx.perf_upload_calls.get().saturating_add(1));
        ctx.perf_upload_bytes
            .set(ctx.perf_upload_bytes.get().saturating_add(bytes));
    }

    #[inline]
    fn perf_on_pass(ctx: &WgpuContext) {
        ctx.perf_render_passes
            .set(ctx.perf_render_passes.get().saturating_add(1));
    }

    #[inline]
    fn perf_on_draw(ctx: &WgpuContext, vertices: u64, indices: u64, instances: u64) {
        ctx.perf_draw_calls
            .set(ctx.perf_draw_calls.get().saturating_add(1));
        ctx.perf_draw_vertices
            .set(ctx.perf_draw_vertices.get().saturating_add(vertices));
        ctx.perf_draw_indices
            .set(ctx.perf_draw_indices.get().saturating_add(indices));
        ctx.perf_draw_instances
            .set(ctx.perf_draw_instances.get().saturating_add(instances));
    }

    pub fn perf_reset(ctx: &WgpuContext) {
        ctx.perf_upload_calls.set(0);
        ctx.perf_upload_bytes.set(0);
        ctx.perf_render_passes.set(0);
        ctx.perf_draw_calls.set(0);
        ctx.perf_draw_instances.set(0);
        ctx.perf_draw_vertices.set(0);
        ctx.perf_draw_indices.set(0);
    }

    pub fn perf_snapshot(ctx: &WgpuContext) -> super::WgpuPerfSnapshot {
        super::WgpuPerfSnapshot {
            upload_calls: ctx.perf_upload_calls.get(),
            upload_bytes: ctx.perf_upload_bytes.get(),
            render_passes: ctx.perf_render_passes.get(),
            draw_calls: ctx.perf_draw_calls.get(),
            draw_instances: ctx.perf_draw_instances.get(),
            draw_vertices: ctx.perf_draw_vertices.get(),
            draw_indices: ctx.perf_draw_indices.get(),
        }
    }

    fn ensure_buffer_capacity(
        device: &::wgpu::Device,
        buffer: &mut ::wgpu::Buffer,
        capacity_bytes: &mut u64,
        required_bytes: u64,
        usage: ::wgpu::BufferUsages,
        label: &str,
    ) {
        if required_bytes <= *capacity_bytes {
            return;
        }

        let mut new_cap = (*capacity_bytes).max(1) * 2;
        if new_cap < required_bytes {
            new_cap = required_bytes.next_power_of_two();
        }

        *buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some(label),
            size: new_cap,
            usage,
            mapped_at_creation: false,
        });
        *capacity_bytes = new_cap;
    }

    pub fn set_globe_transparent(ctx: &mut WgpuContext, transparent: bool) {
        ctx.globe_transparent = transparent;
        ctx.globe_alpha = if transparent { 0.40 } else { 1.0 };
    }

    fn create_depth_view(
        device: &::wgpu::Device,
        config: &::wgpu::SurfaceConfiguration,
    ) -> ::wgpu::TextureView {
        let tex = device.create_texture(&::wgpu::TextureDescriptor {
            label: Some("atlas-depth"),
            size: ::wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: ::wgpu::TextureDimension::D2,
            format: ::wgpu::TextureFormat::Depth24PlusStencil8,
            usage: ::wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        tex.create_view(&::wgpu::TextureViewDescriptor::default())
    }

    fn stencil_write_1() -> ::wgpu::StencilState {
        ::wgpu::StencilState {
            front: ::wgpu::StencilFaceState {
                compare: ::wgpu::CompareFunction::Always,
                fail_op: ::wgpu::StencilOperation::Keep,
                depth_fail_op: ::wgpu::StencilOperation::Keep,
                pass_op: ::wgpu::StencilOperation::Replace,
            },
            back: ::wgpu::StencilFaceState {
                compare: ::wgpu::CompareFunction::Always,
                fail_op: ::wgpu::StencilOperation::Keep,
                depth_fail_op: ::wgpu::StencilOperation::Keep,
                pass_op: ::wgpu::StencilOperation::Keep,
            },
            read_mask: 0xff,
            write_mask: 0xff,
        }
    }

    fn stencil_test_eq_1() -> ::wgpu::StencilState {
        ::wgpu::StencilState {
            front: ::wgpu::StencilFaceState {
                compare: ::wgpu::CompareFunction::Equal,
                fail_op: ::wgpu::StencilOperation::Keep,
                depth_fail_op: ::wgpu::StencilOperation::Keep,
                pass_op: ::wgpu::StencilOperation::Keep,
            },
            back: ::wgpu::StencilFaceState {
                compare: ::wgpu::CompareFunction::Equal,
                fail_op: ::wgpu::StencilOperation::Keep,
                depth_fail_op: ::wgpu::StencilOperation::Keep,
                pass_op: ::wgpu::StencilOperation::Keep,
            },
            read_mask: 0xff,
            write_mask: 0x00,
        }
    }

    fn generate_ellipsoid_mesh(
        lat_segments: u32,
        lon_segments: u32,
        radii: [f32; 3],
    ) -> (Vec<Vertex>, Vec<u16>) {
        let lat_segments = lat_segments.max(3);
        let lon_segments = lon_segments.max(3);

        let mut vertices = Vec::with_capacity(((lat_segments + 1) * (lon_segments + 1)) as usize);
        for lat in 0..=lat_segments {
            let v = lat as f32 / lat_segments as f32;
            let theta = v * std::f32::consts::PI;
            let sin_t = theta.sin();
            let cos_t = theta.cos();

            for lon in 0..=lon_segments {
                let u = lon as f32 / lon_segments as f32;
                let phi = u * std::f32::consts::TAU;
                let sin_p = phi.sin();
                let cos_p = phi.cos();

                // Unit sphere with +Y as north axis.
                let ux = sin_t * cos_p;
                let uy = cos_t;
                let uz = sin_t * sin_p;

                // Scale into an oblate spheroid / ellipsoid (meters).
                let x = ux * radii[0];
                let y = uy * radii[1];
                let z = uz * radii[2];

                // Ellipsoid normal via gradient of implicit surface.
                let nx = x / (radii[0] * radii[0]);
                let ny = y / (radii[1] * radii[1]);
                let nz = z / (radii[2] * radii[2]);
                let nlen = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);
                vertices.push(Vertex {
                    position: [x, y, z],
                    normal: [nx / nlen, ny / nlen, nz / nlen],
                });
            }
        }

        let stride = lon_segments + 1;
        let mut indices = Vec::with_capacity((lat_segments * lon_segments * 6) as usize);
        for lat in 0..lat_segments {
            for lon in 0..lon_segments {
                let i0 = lat * stride + lon;
                let i1 = i0 + 1;
                let i2 = i0 + stride;
                let i3 = i2 + 1;

                // CCW winding when viewed from outside the sphere.
                indices.push(i0 as u16);
                indices.push(i1 as u16);
                indices.push(i2 as u16);
                indices.push(i1 as u16);
                indices.push(i3 as u16);
                indices.push(i2 as u16);
            }
        }

        (vertices, indices)
    }

    fn lat_lon_to_unit(lat_rad: f32, lon_rad: f32) -> [f32; 3] {
        let cos_lat = lat_rad.cos();
        let sin_lat = lat_rad.sin();
        let cos_lon = lon_rad.cos();
        let sin_lon = lon_rad.sin();
        [cos_lat * cos_lon, sin_lat, cos_lat * sin_lon]
    }

    fn generate_graticule_lines() -> Vec<LineVertex> {
        // Simple graticule: meridians every 15°, parallels every 15°.
        // Built as a LineList: (p0,p1),(p1,p2),...
        let mut verts = Vec::new();

        let meridian_step_deg: i32 = 15;
        let parallel_step_deg: i32 = 15;
        let samples: i32 = 128;

        // Lift the graticule slightly above the globe to avoid z-fighting.
        let lift = 1.002_f32;

        // WGS84 ellipsoid in viewer coordinates: equatorial radius on X/Z, polar radius on Y.
        let radii = [WGS84_A as f32, WGS84_B as f32, WGS84_A as f32];

        // Meridians: lon fixed, lat varies -90..90.
        for lon_deg in (0..360).step_by(meridian_step_deg as usize) {
            let lon = (lon_deg as f32).to_radians();
            let mut prev = None;
            for i in 0..=samples {
                let t = i as f32 / samples as f32;
                let lat = -90.0 + 180.0 * t;
                let u = lat_lon_to_unit(lat.to_radians(), lon);
                let mut p = [u[0] * radii[0], u[1] * radii[1], u[2] * radii[2]];
                p[0] *= lift;
                p[1] *= lift;
                p[2] *= lift;
                if let Some(prev_p) = prev {
                    verts.push(LineVertex { position: prev_p });
                    verts.push(LineVertex { position: p });
                }
                prev = Some(p);
            }
        }

        // Parallels: lat fixed, lon varies -180..180.
        for lat_deg in (-75..=75).step_by(parallel_step_deg as usize) {
            let lat = (lat_deg as f32).to_radians();
            let mut prev = None;
            for i in 0..=samples {
                let t = i as f32 / samples as f32;
                let lon = -180.0 + 360.0 * t;
                let u = lat_lon_to_unit(lat, lon.to_radians());
                let mut p = [u[0] * radii[0], u[1] * radii[1], u[2] * radii[2]];
                p[0] *= lift;
                p[1] *= lift;
                p[2] *= lift;
                if let Some(prev_p) = prev {
                    verts.push(LineVertex { position: prev_p });
                    verts.push(LineVertex { position: p });
                }
                prev = Some(p);
            }
        }

        verts
    }

    pub async fn init_wgpu_from_canvas_id(canvas_id: &str) -> Result<WgpuContext, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("window missing"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("document missing"))?;
        let canvas_elem = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("canvas missing"))?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;

        let width = canvas_elem.width();
        let height = canvas_elem.height();

        // IMPORTANT: `wgpu::Surface` must not outlive its `wgpu::Instance`.
        // To avoid UB, we leak the instance for the lifetime of the app.
        //
        // Prefer WebGPU when available, but allow WebGL as a fallback.
        let instance: &'static ::wgpu::Instance = Box::leak(Box::new(::wgpu::Instance::new(
            &::wgpu::InstanceDescriptor {
                backends: ::wgpu::Backends::BROWSER_WEBGPU | ::wgpu::Backends::GL,
                ..Default::default()
            },
        )));

        let surface = instance
            .create_surface(::wgpu::SurfaceTarget::Canvas(canvas_elem.clone()))
            .map_err(|e| JsValue::from_str(&format!("surface error: {e}")))?;

        let adapter = instance
            .request_adapter(&::wgpu::RequestAdapterOptions {
                power_preference: ::wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("adapter error: {e}")))?;

        let (device, queue) = adapter
            .request_device(&::wgpu::DeviceDescriptor {
                label: Some("atlas-wgpu-device"),
                required_features: ::wgpu::Features::empty(),
                required_limits: ::wgpu::Limits::downlevel_webgl2_defaults(),
                ..Default::default()
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("device error: {e}")))?;

        // `adapter`, `device`, `queue` created above.

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .cloned()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let alpha_mode = surface_caps
            .alpha_modes
            .iter()
            .copied()
            .find(|m| *m == ::wgpu::CompositeAlphaMode::Opaque)
            .unwrap_or(surface_caps.alpha_modes[0]);

        let config = ::wgpu::SurfaceConfiguration {
            usage: ::wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            desired_maximum_frame_latency: 2,
            present_mode: ::wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);

        let shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-globe-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(GLOBE_SHADER)),
        });

        let graticule_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-graticule-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(GRATICULE_SHADER)),
        });

        let stars_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-stars-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(STARS_SHADER)),
        });

        let cities_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-cities-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(CITIES_SHADER)),
        });

        let corridors_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-corridors-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(CORRIDORS_SHADER)),
        });

        let overlays_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-overlays-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(OVERLAYS_SHADER)),
        });

        let terrain_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-terrain-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(TERRAIN_SHADER)),
        });

        let map2d_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-map2d-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(MAP2D_SHADER)),
        });

        let uniform_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: ::wgpu::BufferUsages::UNIFORM | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform2d_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-globals-2d"),
            size: std::mem::size_of::<Globals2D>() as u64,
            usage: ::wgpu::BufferUsages::UNIFORM | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // A small style lookup table; grows as needed.
        let styles_capacity_bytes = (std::mem::size_of::<Style>().max(16)) as u64;
        let styles_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-styles"),
            size: styles_capacity_bytes,
            usage: ::wgpu::BufferUsages::STORAGE | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&::wgpu::BindGroupLayoutDescriptor {
                label: Some("atlas-unified-bgl"),
                entries: &[
                    ::wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ::wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: ::wgpu::BindingType::Buffer {
                            ty: ::wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    ::wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ::wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: ::wgpu::BindingType::Buffer {
                            ty: ::wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let uniform_bind_group = device.create_bind_group(&::wgpu::BindGroupDescriptor {
            label: Some("atlas-unified-bg"),
            layout: &uniform_bind_group_layout,
            entries: &[
                ::wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                ::wgpu::BindGroupEntry {
                    binding: 1,
                    resource: styles_buffer.as_entire_binding(),
                },
            ],
        });

        let uniform2d_bind_group = device.create_bind_group(&::wgpu::BindGroupDescriptor {
            label: Some("atlas-unified-bg-2d"),
            layout: &uniform_bind_group_layout,
            entries: &[
                ::wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform2d_buffer.as_entire_binding(),
                },
                ::wgpu::BindGroupEntry {
                    binding: 1,
                    resource: styles_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&::wgpu::PipelineLayoutDescriptor {
            label: Some("atlas-globe-pipeline-layout"),
            bind_group_layouts: &[&uniform_bind_group_layout],
            immediate_size: 0,
        });

        let stars_pipeline_layout =
            device.create_pipeline_layout(&::wgpu::PipelineLayoutDescriptor {
                label: Some("atlas-stars-pipeline-layout"),
                bind_group_layouts: &[&uniform_bind_group_layout],
                immediate_size: 0,
            });

        // Starfield background: generated procedurally via vertex_index.
        let stars_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-stars-pipeline"),
            layout: Some(&stars_pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &stars_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &stars_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::PointList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let globe_pipeline_solid =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-globe-pipeline-solid"),
                layout: Some(&pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 12,
                                shader_location: 1,
                            },
                        ],
                    }],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::REPLACE),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    // Cull back faces so overlays don't show through the far side of the globe.
                    cull_mode: Some(::wgpu::Face::Back),
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(::wgpu::DepthStencilState {
                    format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: true,
                    depth_compare: ::wgpu::CompareFunction::Less,
                    stencil: stencil_write_1(),
                    bias: ::wgpu::DepthBiasState::default(),
                }),
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        // Transparent globe: alpha blend over the already-rendered background, but still
        // write depth+stencil so overlays can be depth-tested and masked correctly.
        let globe_pipeline_transparent =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-globe-pipeline-transparent"),
                layout: Some(&pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 12,
                                shader_location: 1,
                            },
                        ],
                    }],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    cull_mode: Some(::wgpu::Face::Back),
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(::wgpu::DepthStencilState {
                    format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: true,
                    depth_compare: ::wgpu::CompareFunction::Less,
                    stencil: stencil_write_1(),
                    bias: ::wgpu::DepthBiasState::default(),
                }),
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let graticule_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-graticule-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &graticule_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[::wgpu::VertexAttribute {
                        format: ::wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &graticule_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: stencil_test_eq_1(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cities_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-cities-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &cities_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    ::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<QuadVertex>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    },
                    ::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CityInstance>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Instance,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 1,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Uint32,
                                offset: 12,
                                shader_location: 2,
                            },
                        ],
                    },
                ],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &cities_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: stencil_test_eq_1(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let corridors_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-corridors-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &corridors_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    ::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<SegmentVertex>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32,
                                offset: 0,
                                shader_location: 0,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32,
                                offset: 4,
                                shader_location: 1,
                            },
                        ],
                    },
                    ::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CorridorInstance>()
                            as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Instance,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 2,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 16,
                                shader_location: 3,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Uint32,
                                offset: 28,
                                shader_location: 4,
                            },
                        ],
                    },
                ],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &corridors_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: stencil_test_eq_1(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let overlays_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-overlays-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &overlays_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Uint32,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &overlays_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: stencil_test_eq_1(),
                bias: ::wgpu::DepthBiasState {
                    constant: -2,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let base_overlays_pipeline =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-base-overlays-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &overlays_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<OverlayVertex>() as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Uint32,
                                offset: 12,
                                shader_location: 1,
                            },
                        ],
                    }],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &overlays_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(::wgpu::DepthStencilState {
                    format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: false,
                    depth_compare: ::wgpu::CompareFunction::Always,
                    stencil: stencil_test_eq_1(),
                    bias: ::wgpu::DepthBiasState::default(),
                }),
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let terrain_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-terrain-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &terrain_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainVertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32,
                            offset: 12,
                            shader_location: 1,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &terrain_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: stencil_test_eq_1(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let map2d_pipeline_layout =
            device.create_pipeline_layout(&::wgpu::PipelineLayoutDescriptor {
                label: Some("atlas-map2d-pipeline-layout"),
                bind_group_layouts: &[&uniform_bind_group_layout],
                immediate_size: 0,
            });

        let map2d_polys_pipeline =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-map2d-polys-pipeline"),
                layout: Some(&map2d_pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &map2d_shader,
                    entry_point: Some("vs_poly"),
                    compilation_options: Default::default(),
                    buffers: &[::wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Overlay2DVertex>()
                            as ::wgpu::BufferAddress,
                        step_mode: ::wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32,
                                offset: 8,
                                shader_location: 1,
                            },
                            ::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Uint32,
                                offset: 12,
                                shader_location: 2,
                            },
                        ],
                    }],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &map2d_shader,
                    entry_point: Some("fs_poly"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let map2d_points_pipeline =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-map2d-points-pipeline"),
                layout: Some(&map2d_pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &map2d_shader,
                    entry_point: Some("vs_point"),
                    compilation_options: Default::default(),
                    buffers: &[
                        ::wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<QuadVertex>()
                                as ::wgpu::BufferAddress,
                            step_mode: ::wgpu::VertexStepMode::Vertex,
                            attributes: &[::wgpu::VertexAttribute {
                                format: ::wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            }],
                        },
                        ::wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Point2DInstance>()
                                as ::wgpu::BufferAddress,
                            step_mode: ::wgpu::VertexStepMode::Instance,
                            attributes: &[
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Float32x2,
                                    offset: 0,
                                    shader_location: 1,
                                },
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Uint32,
                                    offset: 8,
                                    shader_location: 2,
                                },
                            ],
                        },
                    ],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &map2d_shader,
                    entry_point: Some("fs_point"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let map2d_lines_pipeline =
            device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
                label: Some("atlas-map2d-lines-pipeline"),
                layout: Some(&map2d_pipeline_layout),
                vertex: ::wgpu::VertexState {
                    module: &map2d_shader,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &[
                        ::wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<SegmentVertex>()
                                as ::wgpu::BufferAddress,
                            step_mode: ::wgpu::VertexStepMode::Vertex,
                            attributes: &[
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Float32,
                                    offset: 0,
                                    shader_location: 0,
                                },
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Float32,
                                    offset: 4,
                                    shader_location: 1,
                                },
                            ],
                        },
                        ::wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Segment2DInstance>()
                                as ::wgpu::BufferAddress,
                            step_mode: ::wgpu::VertexStepMode::Instance,
                            attributes: &[
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Float32x2,
                                    offset: 0,
                                    shader_location: 2,
                                },
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Float32x2,
                                    offset: 8,
                                    shader_location: 3,
                                },
                                ::wgpu::VertexAttribute {
                                    format: ::wgpu::VertexFormat::Uint32,
                                    offset: 16,
                                    shader_location: 4,
                                },
                            ],
                        },
                    ],
                },
                fragment: Some(::wgpu::FragmentState {
                    module: &map2d_shader,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &[Some(::wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: ::wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: ::wgpu::PrimitiveState {
                    topology: ::wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: ::wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: ::wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: ::wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let radii = [WGS84_A as f32, WGS84_B as f32, WGS84_A as f32];
        let (vertices, indices) = generate_ellipsoid_mesh(64, 128, radii);
        let vertex_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-globe-vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: ::wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-globe-indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: ::wgpu::BufferUsages::INDEX,
        });

        let graticule_vertices = generate_graticule_lines();
        let graticule_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-graticule-vertices"),
                contents: bytemuck::cast_slice(&graticule_vertices),
                usage: ::wgpu::BufferUsages::VERTEX,
            });

        let cities_quad_vertices: [QuadVertex; 6] = [
            QuadVertex {
                corner: [-1.0, -1.0],
            },
            QuadVertex {
                corner: [1.0, -1.0],
            },
            QuadVertex { corner: [1.0, 1.0] },
            QuadVertex {
                corner: [-1.0, -1.0],
            },
            QuadVertex { corner: [1.0, 1.0] },
            QuadVertex {
                corner: [-1.0, 1.0],
            },
        ];
        let cities_quad_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-cities-quad"),
                contents: bytemuck::cast_slice(&cities_quad_vertices),
                usage: ::wgpu::BufferUsages::VERTEX,
            });
        let cities_instance_capacity_bytes = (std::mem::size_of::<CityVertex>().max(16)) as u64;
        let cities_instance_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-cities-instances"),
            size: cities_instance_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let corridor_quad_vertices: [SegmentVertex; 6] = [
            SegmentVertex {
                along: 0.0,
                side: -1.0,
            },
            SegmentVertex {
                along: 1.0,
                side: -1.0,
            },
            SegmentVertex {
                along: 1.0,
                side: 1.0,
            },
            SegmentVertex {
                along: 0.0,
                side: -1.0,
            },
            SegmentVertex {
                along: 1.0,
                side: 1.0,
            },
            SegmentVertex {
                along: 0.0,
                side: 1.0,
            },
        ];
        let corridor_quad_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-corridors-quad"),
                contents: bytemuck::cast_slice(&corridor_quad_vertices),
                usage: ::wgpu::BufferUsages::VERTEX,
            });
        let corridors_instance_capacity_bytes =
            (std::mem::size_of::<CorridorVertex>().max(16)) as u64;
        let corridors_instance_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-corridors-instances"),
            size: corridors_instance_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let overlay2d_vertex_capacity_bytes =
            (std::mem::size_of::<Overlay2DVertex>().max(16)) as u64;
        let base_regions2d_vertex_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-base-regions2d-vertices"),
            size: overlay2d_vertex_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let regions2d_vertex_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-regions2d-vertices"),
            size: overlay2d_vertex_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let regions2d_vertex_capacity_bytes = overlay2d_vertex_capacity_bytes;
        let base_regions2d_vertex_capacity_bytes = overlay2d_vertex_capacity_bytes;

        let points2d_instance_capacity_bytes =
            (std::mem::size_of::<Point2DInstance>().max(16)) as u64;
        let points2d_instance_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-points2d-instances"),
            size: points2d_instance_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let lines2d_instance_capacity_bytes =
            (std::mem::size_of::<Segment2DInstance>().max(16)) as u64;
        let lines2d_instance_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-lines2d-instances"),
            size: lines2d_instance_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let grid2d_instance_capacity_bytes =
            (std::mem::size_of::<Segment2DInstance>().max(16)) as u64;
        let grid2d_instance_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-grid2d-instances"),
            size: grid2d_instance_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let overlay_vertex_capacity_bytes = (std::mem::size_of::<OverlayVertex>().max(16)) as u64;
        let regions_vertex_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-regions-vertices"),
            size: overlay_vertex_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let terrain_vertex_capacity_bytes = (std::mem::size_of::<TerrainVertex>().max(16)) as u64;
        let terrain_vertex_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-terrain-vertices"),
            size: terrain_vertex_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let base_regions_vertex_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-base-regions-vertices"),
            size: overlay_vertex_capacity_bytes,
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let regions_vertex_capacity_bytes = overlay_vertex_capacity_bytes;
        let base_regions_vertex_capacity_bytes = overlay_vertex_capacity_bytes;

        // Initialize uniforms so the first render doesn't read uninitialized memory.
        let globals = Globals {
            view_proj: [[0.0; 4]; 4],
            light_dir: [0.4, 0.7, 0.2],
            _pad0: 0.0,
            viewport: [1.0, 1.0],
            globe_alpha: 1.0,
            _pad1: 0.0,
            globe_color: [0.10, 0.55, 0.85],
            stars_alpha: 1.0,
        };
        queue.write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&globals));

        let globals2d = Globals2D {
            center_m: [0.0, 0.0],
            scale_px_per_m: 1e-6,
            world_width_m: (2.0 * std::f32::consts::PI) * (WGS84_A as f32),
            viewport_px: [1.0, 1.0],
            _pad0: [0.0, 0.0],
        };
        queue.write_buffer(&uniform2d_buffer, 0, bytemuck::bytes_of(&globals2d));

        let default_style = Style {
            color: [1.0, 1.0, 1.0, 1.0],
            lift_m: 0.0,
            size_px: 3.0,
            width_px: 1.0,
            _pad0: 0.0,
        };
        queue.write_buffer(&styles_buffer, 0, bytemuck::bytes_of(&default_style));

        Ok(WgpuContext {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            _canvas: canvas_elem,
            clear_color: ::wgpu::Color {
                r: 0.004,
                g: 0.008,
                b: 0.016,
                a: 1.0,
            },
            globe_color: [0.10, 0.55, 0.85],
            globe_alpha: 1.0,
            globe_transparent: false,
            stars_alpha: 1.0,
            stars_pipeline,
            stars_count: 1200,
            globe_pipeline_solid,
            globe_pipeline_transparent,
            graticule_pipeline,
            cities_pipeline,
            corridors_pipeline,
            overlays_pipeline,
            base_overlays_pipeline,
            terrain_pipeline,
            map2d_polys_pipeline,
            map2d_lines_pipeline,
            map2d_points_pipeline,
            uniform_buffer,
            uniform2d_buffer,
            styles_buffer,
            styles_capacity_bytes,
            styles_count: 1,
            uniform_bind_group_layout,
            uniform_bind_group,
            uniform2d_bind_group,
            depth_view,
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            graticule_vertex_buffer,
            graticule_vertex_count: graticule_vertices.len() as u32,
            cities_quad_vertex_buffer,
            cities_instance_buffer,
            cities_instance_capacity_bytes,
            cities_instance_count: 0,
            corridor_quad_vertex_buffer,
            corridors_instance_buffer,
            corridors_instance_capacity_bytes,
            corridors_instance_count: 0,
            regions_vertex_buffer,
            regions_vertex_capacity_bytes,
            regions_vertex_count: 0,
            terrain_vertex_buffer,
            terrain_vertex_capacity_bytes,
            terrain_vertex_count: 0,
            base_regions_vertex_buffer,
            base_regions_vertex_capacity_bytes,
            base_regions_vertex_count: 0,
            base_regions2d_vertex_buffer,
            base_regions2d_vertex_capacity_bytes,
            base_regions2d_vertex_count: 0,
            regions2d_vertex_buffer,
            regions2d_vertex_capacity_bytes,
            regions2d_vertex_count: 0,
            points2d_instance_buffer,
            points2d_instance_capacity_bytes,
            points2d_instance_count: 0,
            lines2d_instance_buffer,
            lines2d_instance_capacity_bytes,
            lines2d_instance_count: 0,
            grid2d_instance_buffer,
            grid2d_instance_capacity_bytes,
            grid2d_instance_count: 0,

            perf_upload_calls: Cell::new(0),
            perf_upload_bytes: Cell::new(0),
            perf_render_passes: Cell::new(0),
            perf_draw_calls: Cell::new(0),
            perf_draw_instances: Cell::new(0),
            perf_draw_vertices: Cell::new(0),
            perf_draw_indices: Cell::new(0),
        })
    }

    pub fn set_cities_points(ctx: &mut WgpuContext, points: &[CityVertex]) {
        if points.is_empty() {
            ctx.cities_instance_count = 0;
            return;
        }

        let required_bytes = (points.len() * std::mem::size_of::<CityVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.cities_instance_buffer,
            &mut ctx.cities_instance_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-cities-instances",
        );
        ctx.queue
            .write_buffer(&ctx.cities_instance_buffer, 0, bytemuck::cast_slice(points));
        perf_on_write(
            ctx,
            (points.len() * std::mem::size_of::<CityVertex>()) as u64,
        );
        ctx.cities_instance_count = points.len() as u32;
    }

    pub fn set_corridors_points(ctx: &mut WgpuContext, points: &[CorridorVertex]) {
        if points.is_empty() {
            ctx.corridors_instance_count = 0;
            return;
        }

        let required_bytes = (points.len() * std::mem::size_of::<CorridorVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.corridors_instance_buffer,
            &mut ctx.corridors_instance_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-corridors-instances",
        );
        ctx.queue.write_buffer(
            &ctx.corridors_instance_buffer,
            0,
            bytemuck::cast_slice(points),
        );
        perf_on_write(
            ctx,
            (points.len() * std::mem::size_of::<CorridorVertex>()) as u64,
        );
        ctx.corridors_instance_count = points.len() as u32;
    }

    pub fn set_regions_points(ctx: &mut WgpuContext, points: &[OverlayVertex]) {
        if points.is_empty() {
            ctx.regions_vertex_count = 0;
            return;
        }

        let required_bytes = (points.len() * std::mem::size_of::<OverlayVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.regions_vertex_buffer,
            &mut ctx.regions_vertex_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-regions-vertices",
        );
        ctx.queue
            .write_buffer(&ctx.regions_vertex_buffer, 0, bytemuck::cast_slice(points));
        perf_on_write(
            ctx,
            (points.len() * std::mem::size_of::<OverlayVertex>()) as u64,
        );
        ctx.regions_vertex_count = points.len() as u32;
    }

    pub fn set_terrain_points(ctx: &mut WgpuContext, points: &[TerrainVertex]) {
        if points.is_empty() {
            ctx.terrain_vertex_count = 0;
            return;
        }

        let required_bytes = (points.len() * std::mem::size_of::<TerrainVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.terrain_vertex_buffer,
            &mut ctx.terrain_vertex_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-terrain-vertices",
        );
        ctx.queue
            .write_buffer(&ctx.terrain_vertex_buffer, 0, bytemuck::cast_slice(points));
        perf_on_write(
            ctx,
            (points.len() * std::mem::size_of::<TerrainVertex>()) as u64,
        );
        ctx.terrain_vertex_count = points.len() as u32;
    }

    pub fn set_base_regions_points(ctx: &mut WgpuContext, points: &[OverlayVertex]) {
        if points.is_empty() {
            ctx.base_regions_vertex_count = 0;
            return;
        }

        let required_bytes = (points.len() * std::mem::size_of::<OverlayVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.base_regions_vertex_buffer,
            &mut ctx.base_regions_vertex_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-base-regions-vertices",
        );
        ctx.queue.write_buffer(
            &ctx.base_regions_vertex_buffer,
            0,
            bytemuck::cast_slice(points),
        );
        perf_on_write(
            ctx,
            (points.len() * std::mem::size_of::<OverlayVertex>()) as u64,
        );
        ctx.base_regions_vertex_count = points.len() as u32;
    }

    pub fn set_base_regions2d_vertices(ctx: &mut WgpuContext, verts: &[Overlay2DVertex]) {
        if verts.is_empty() {
            ctx.base_regions2d_vertex_count = 0;
            return;
        }

        let required_bytes = (verts.len() * std::mem::size_of::<Overlay2DVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.base_regions2d_vertex_buffer,
            &mut ctx.base_regions2d_vertex_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-base-regions2d-vertices",
        );
        ctx.queue.write_buffer(
            &ctx.base_regions2d_vertex_buffer,
            0,
            bytemuck::cast_slice(verts),
        );
        perf_on_write(
            ctx,
            (verts.len() * std::mem::size_of::<Overlay2DVertex>()) as u64,
        );
        ctx.base_regions2d_vertex_count = verts.len() as u32;
    }

    pub fn set_regions2d_vertices(ctx: &mut WgpuContext, verts: &[Overlay2DVertex]) {
        if verts.is_empty() {
            ctx.regions2d_vertex_count = 0;
            return;
        }

        let required_bytes = (verts.len() * std::mem::size_of::<Overlay2DVertex>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.regions2d_vertex_buffer,
            &mut ctx.regions2d_vertex_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-regions2d-vertices",
        );
        ctx.queue
            .write_buffer(&ctx.regions2d_vertex_buffer, 0, bytemuck::cast_slice(verts));
        perf_on_write(
            ctx,
            (verts.len() * std::mem::size_of::<Overlay2DVertex>()) as u64,
        );
        ctx.regions2d_vertex_count = verts.len() as u32;
    }

    pub fn set_points2d_instances(ctx: &mut WgpuContext, inst: &[Point2DInstance]) {
        if inst.is_empty() {
            ctx.points2d_instance_count = 0;
            return;
        }

        let required_bytes = (inst.len() * std::mem::size_of::<Point2DInstance>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.points2d_instance_buffer,
            &mut ctx.points2d_instance_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-points2d-instances",
        );
        ctx.queue
            .write_buffer(&ctx.points2d_instance_buffer, 0, bytemuck::cast_slice(inst));
        perf_on_write(
            ctx,
            (inst.len() * std::mem::size_of::<Point2DInstance>()) as u64,
        );
        ctx.points2d_instance_count = inst.len() as u32;
    }

    pub fn set_lines2d_instances(ctx: &mut WgpuContext, inst: &[Segment2DInstance]) {
        if inst.is_empty() {
            ctx.lines2d_instance_count = 0;
            return;
        }

        let required_bytes = (inst.len() * std::mem::size_of::<Segment2DInstance>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.lines2d_instance_buffer,
            &mut ctx.lines2d_instance_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-lines2d-instances",
        );
        ctx.queue
            .write_buffer(&ctx.lines2d_instance_buffer, 0, bytemuck::cast_slice(inst));
        perf_on_write(
            ctx,
            (inst.len() * std::mem::size_of::<Segment2DInstance>()) as u64,
        );
        ctx.lines2d_instance_count = inst.len() as u32;
    }

    pub fn set_grid2d_instances(ctx: &mut WgpuContext, inst: &[Segment2DInstance]) {
        if inst.is_empty() {
            ctx.grid2d_instance_count = 0;
            return;
        }

        let required_bytes = (inst.len() * std::mem::size_of::<Segment2DInstance>()) as u64;
        ensure_buffer_capacity(
            &ctx.device,
            &mut ctx.grid2d_instance_buffer,
            &mut ctx.grid2d_instance_capacity_bytes,
            required_bytes,
            ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            "atlas-grid2d-instances",
        );
        ctx.queue
            .write_buffer(&ctx.grid2d_instance_buffer, 0, bytemuck::cast_slice(inst));
        perf_on_write(
            ctx,
            (inst.len() * std::mem::size_of::<Segment2DInstance>()) as u64,
        );
        ctx.grid2d_instance_count = inst.len() as u32;
    }

    pub fn resize_wgpu(ctx: &mut WgpuContext, width: u32, height: u32) {
        ctx.config.width = width.max(1);
        ctx.config.height = height.max(1);
        ctx.surface.configure(&ctx.device, &ctx.config);
        ctx.depth_view = create_depth_view(&ctx.device, &ctx.config);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_mesh(
        ctx: &WgpuContext,
        view_proj: [[f32; 4]; 4],
        light_dir: [f32; 3],
        show_graticule: bool,
        show_base_regions: bool,
        show_terrain: bool,
        show_cities: bool,
        show_corridors: bool,
        show_regions: bool,
    ) -> Result<(), JsValue> {
        let frame = ctx
            .surface
            .get_current_texture()
            .map_err(|e| JsValue::from_str(&format!("surface acquire failed: {e}")))?;
        let view = frame
            .texture
            .create_view(&::wgpu::TextureViewDescriptor::default());

        let globals = Globals {
            view_proj,
            light_dir,
            _pad0: 0.0,
            viewport: [ctx.config.width as f32, ctx.config.height as f32],
            // IMPORTANT: even when the globe pipeline uses BlendState::REPLACE (solid mode),
            // the fragment alpha still lands in the surface and the browser composites the
            // canvas using that alpha. Force alpha=1.0 whenever transparency is disabled.
            globe_alpha: if ctx.globe_transparent {
                ctx.globe_alpha
            } else {
                1.0
            },
            _pad1: 0.0,
            globe_color: ctx.globe_color,
            stars_alpha: ctx.stars_alpha,
        };
        ctx.queue
            .write_buffer(&ctx.uniform_buffer, 0, bytemuck::bytes_of(&globals));
        perf_on_write(ctx, std::mem::size_of::<Globals>() as u64);

        let mut encoder = ctx
            .device
            .create_command_encoder(&::wgpu::CommandEncoderDescriptor {
                label: Some("atlas-mesh-encoder"),
            });

        // Pass 1: clear to deep space and draw stars (no depth attachment).
        {
            perf_on_pass(ctx);
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-stars-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(ctx.clear_color),
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.stars_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.draw(0..ctx.stars_count, 0..1);
            perf_on_draw(ctx, ctx.stars_count as u64, 0, 1);
        }

        // Pass 2: single main pass (globe + all overlays), preserving the starfield color.
        {
            perf_on_pass(ctx);
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-main-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(::wgpu::RenderPassDepthStencilAttachment {
                    view: &ctx.depth_view,
                    depth_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(1.0),
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(0),
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            // Globe writes stencil=1 anywhere it draws.
            rpass.set_stencil_reference(1);

            // Globe
            let globe_pipeline = if ctx.globe_transparent {
                &ctx.globe_pipeline_transparent
            } else {
                &ctx.globe_pipeline_solid
            };
            rpass.set_pipeline(globe_pipeline);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.set_index_buffer(ctx.index_buffer.slice(..), ::wgpu::IndexFormat::Uint16);
            rpass.draw_indexed(0..ctx.index_count, 0, 0..1);
            perf_on_draw(ctx, 0, ctx.index_count as u64, 1);

            // Terrain
            if show_terrain && ctx.terrain_vertex_count > 0 {
                rpass.set_pipeline(&ctx.terrain_pipeline);
                rpass.set_vertex_buffer(0, ctx.terrain_vertex_buffer.slice(..));
                rpass.draw(0..ctx.terrain_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.terrain_vertex_count as u64, 0, 1);
            }

            // Graticule
            if show_graticule {
                rpass.set_pipeline(&ctx.graticule_pipeline);
                rpass.set_vertex_buffer(0, ctx.graticule_vertex_buffer.slice(..));
                rpass.draw(0..ctx.graticule_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.graticule_vertex_count as u64, 0, 1);
            }

            // Base polygons
            if show_base_regions && ctx.base_regions_vertex_count > 0 {
                rpass.set_pipeline(&ctx.base_overlays_pipeline);
                rpass.set_vertex_buffer(0, ctx.base_regions_vertex_buffer.slice(..));
                rpass.draw(0..ctx.base_regions_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.base_regions_vertex_count as u64, 0, 1);
            }

            // Region polygons
            if show_regions && ctx.regions_vertex_count > 0 {
                rpass.set_pipeline(&ctx.overlays_pipeline);
                rpass.set_vertex_buffer(0, ctx.regions_vertex_buffer.slice(..));
                rpass.draw(0..ctx.regions_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.regions_vertex_count as u64, 0, 1);
            }

            // Air corridors (instanced)
            if show_corridors && ctx.corridors_instance_count > 0 {
                rpass.set_pipeline(&ctx.corridors_pipeline);
                rpass.set_vertex_buffer(0, ctx.corridor_quad_vertex_buffer.slice(..));
                rpass.set_vertex_buffer(1, ctx.corridors_instance_buffer.slice(..));
                rpass.draw(0..6, 0..ctx.corridors_instance_count);
                perf_on_draw(ctx, 6, 0, ctx.corridors_instance_count as u64);
            }

            // City markers (instanced)
            if show_cities && ctx.cities_instance_count > 0 {
                rpass.set_pipeline(&ctx.cities_pipeline);
                rpass.set_vertex_buffer(0, ctx.cities_quad_vertex_buffer.slice(..));
                rpass.set_vertex_buffer(1, ctx.cities_instance_buffer.slice(..));
                rpass.draw(0..6, 0..ctx.cities_instance_count);
                perf_on_draw(ctx, 6, 0, ctx.cities_instance_count as u64);
            }
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_map2d(
        ctx: &WgpuContext,
        globals2d: Globals2D,
        show_graticule: bool,
        show_base_regions: bool,
        show_regions: bool,
        show_lines: bool,
        show_points: bool,
    ) -> Result<(), JsValue> {
        let frame = ctx
            .surface
            .get_current_texture()
            .map_err(|e| JsValue::from_str(&format!("surface acquire failed: {e}")))?;
        let view = frame
            .texture
            .create_view(&::wgpu::TextureViewDescriptor::default());

        ctx.queue
            .write_buffer(&ctx.uniform2d_buffer, 0, bytemuck::bytes_of(&globals2d));
        perf_on_write(ctx, std::mem::size_of::<Globals2D>() as u64);

        let mut encoder = ctx
            .device
            .create_command_encoder(&::wgpu::CommandEncoderDescriptor {
                label: Some("atlas-map2d-encoder"),
            });

        {
            perf_on_pass(ctx);
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-map2d-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(ctx.clear_color),
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_bind_group(0, &ctx.uniform2d_bind_group, &[]);

            // Graticule (as instanced thick segments).
            if show_graticule && ctx.grid2d_instance_count > 0 {
                rpass.set_pipeline(&ctx.map2d_lines_pipeline);
                rpass.set_vertex_buffer(0, ctx.corridor_quad_vertex_buffer.slice(..));
                rpass.set_vertex_buffer(1, ctx.grid2d_instance_buffer.slice(..));
                rpass.draw(0..6, 0..ctx.grid2d_instance_count);
                perf_on_draw(ctx, 6, 0, ctx.grid2d_instance_count as u64);
            }

            // Base polygons.
            if show_base_regions && ctx.base_regions2d_vertex_count > 0 {
                rpass.set_pipeline(&ctx.map2d_polys_pipeline);
                rpass.set_vertex_buffer(0, ctx.base_regions2d_vertex_buffer.slice(..));
                rpass.draw(0..ctx.base_regions2d_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.base_regions2d_vertex_count as u64, 0, 1);
            }

            // Polygons.
            if show_regions && ctx.regions2d_vertex_count > 0 {
                rpass.set_pipeline(&ctx.map2d_polys_pipeline);
                rpass.set_vertex_buffer(0, ctx.regions2d_vertex_buffer.slice(..));
                rpass.draw(0..ctx.regions2d_vertex_count, 0..1);
                perf_on_draw(ctx, ctx.regions2d_vertex_count as u64, 0, 1);
            }

            // Lines.
            if show_lines && ctx.lines2d_instance_count > 0 {
                rpass.set_pipeline(&ctx.map2d_lines_pipeline);
                rpass.set_vertex_buffer(0, ctx.corridor_quad_vertex_buffer.slice(..));
                rpass.set_vertex_buffer(1, ctx.lines2d_instance_buffer.slice(..));
                rpass.draw(0..6, 0..ctx.lines2d_instance_count);
                perf_on_draw(ctx, 6, 0, ctx.lines2d_instance_count as u64);
            }

            // Points.
            if show_points && ctx.points2d_instance_count > 0 {
                rpass.set_pipeline(&ctx.map2d_points_pipeline);
                rpass.set_vertex_buffer(0, ctx.cities_quad_vertex_buffer.slice(..));
                rpass.set_vertex_buffer(1, ctx.points2d_instance_buffer.slice(..));
                rpass.draw(0..6, 0..ctx.points2d_instance_count);
                perf_on_draw(ctx, 6, 0, ctx.points2d_instance_count as u64);
            }
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use ::wgpu;
    use wasm_bindgen::prelude::JsValue;

    #[derive(Debug, Default)]
    pub struct WgpuContext;

    pub async fn init_wgpu_from_canvas_id(_canvas_id: &str) -> Result<WgpuContext, JsValue> {
        Err(JsValue::from_str(
            "wgpu initialization is only available on wasm32 targets",
        ))
    }

    pub fn resize_wgpu(_ctx: &mut WgpuContext, _width: u32, _height: u32) {}

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct CityVertex {
        pub center: [f32; 3],
        pub style_id: u32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct OverlayVertex {
        pub position: [f32; 3],
        pub style_id: u32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct TerrainVertex {
        pub position: [f32; 3],
        pub lift_m: f32,
        pub color: [f32; 4],
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct CorridorVertex {
        pub a: [f32; 3],
        pub _pad0: u32,
        pub b: [f32; 3],
        pub style_id: u32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct Style {
        pub color: [f32; 4],
        pub lift_m: f32,
        pub size_px: f32,
        pub width_px: f32,
        pub _pad0: f32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct Globals2D {
        pub center_m: [f32; 2],
        pub scale_px_per_m: f32,
        pub world_width_m: f32,
        pub viewport_px: [f32; 2],
        pub _pad0: [f32; 2],
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct Overlay2DVertex {
        pub position_m: [f32; 2],
        pub anchor_x_m: f32,
        pub style_id: u32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct Point2DInstance {
        pub center_m: [f32; 2],
        pub style_id: u32,
        pub _pad0: u32,
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct Segment2DInstance {
        pub a_m: [f32; 2],
        pub b_m: [f32; 2],
        pub style_id: u32,
        pub _pad0: u32,
    }

    pub fn set_cities_points(_ctx: &mut WgpuContext, _points: &[CityVertex]) {}

    pub fn set_corridors_points(_ctx: &mut WgpuContext, _points: &[CorridorVertex]) {}

    pub fn set_regions_points(_ctx: &mut WgpuContext, _points: &[OverlayVertex]) {}

    pub fn set_terrain_points(_ctx: &mut WgpuContext, _points: &[TerrainVertex]) {}

    pub fn set_base_regions_points(_ctx: &mut WgpuContext, _points: &[OverlayVertex]) {}

    pub fn set_base_regions2d_vertices(_ctx: &mut WgpuContext, _verts: &[Overlay2DVertex]) {}

    pub fn set_regions2d_vertices(_ctx: &mut WgpuContext, _verts: &[Overlay2DVertex]) {}

    pub fn set_points2d_instances(_ctx: &mut WgpuContext, _inst: &[Point2DInstance]) {}

    pub fn set_lines2d_instances(_ctx: &mut WgpuContext, _inst: &[Segment2DInstance]) {}

    pub fn set_grid2d_instances(_ctx: &mut WgpuContext, _inst: &[Segment2DInstance]) {}

    pub fn set_styles(_ctx: &mut WgpuContext, _styles: &[Style]) {}

    pub fn set_theme(
        _ctx: &mut WgpuContext,
        _clear_color: wgpu::Color,
        _globe_color: [f32; 3],
        _stars_alpha: f32,
    ) {
    }

    pub fn set_globe_transparent(_ctx: &mut WgpuContext, _transparent: bool) {}

    #[allow(clippy::too_many_arguments)]
    pub fn render_mesh(
        _ctx: &WgpuContext,
        _view_proj: [[f32; 4]; 4],
        _light_dir: [f32; 3],
        _show_graticule: bool,
        _show_base_regions: bool,
        _show_terrain: bool,
        _show_cities: bool,
        _show_corridors: bool,
        _show_regions: bool,
    ) -> Result<(), JsValue> {
        Err(JsValue::from_str(
            "wgpu rendering is only available on wasm32 targets",
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_map2d(
        _ctx: &WgpuContext,
        _globals2d: Globals2D,
        _show_graticule: bool,
        _show_base_regions: bool,
        _show_regions: bool,
        _show_lines: bool,
        _show_points: bool,
    ) -> Result<(), JsValue> {
        Err(JsValue::from_str(
            "wgpu rendering is only available on wasm32 targets",
        ))
    }

    pub fn perf_reset(_ctx: &WgpuContext) {}

    pub fn perf_snapshot(_ctx: &WgpuContext) -> super::WgpuPerfSnapshot {
        super::WgpuPerfSnapshot::default()
    }
}

pub use imp::{
    CityVertex, CorridorVertex, Globals2D, Overlay2DVertex, OverlayVertex, Point2DInstance,
    Segment2DInstance, Style, TerrainVertex, WgpuContext, init_wgpu_from_canvas_id, perf_reset,
    perf_snapshot, render_map2d, render_mesh, resize_wgpu, set_base_regions_points,
    set_base_regions2d_vertices, set_cities_points, set_corridors_points, set_globe_transparent,
    set_grid2d_instances, set_lines2d_instances, set_points2d_instances, set_regions_points,
    set_regions2d_vertices, set_styles, set_terrain_points, set_theme,
};
