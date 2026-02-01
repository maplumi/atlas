#[cfg(target_arch = "wasm32")]
mod imp {
    use ::wgpu::util::DeviceExt;
    use std::borrow::Cow;
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
        pub stars_alpha: f32,
        pub stars_pipeline: ::wgpu::RenderPipeline,
        pub stars_count: u32,
        pub pipeline: ::wgpu::RenderPipeline,
        pub graticule_pipeline: ::wgpu::RenderPipeline,
        pub cities_pipeline: ::wgpu::RenderPipeline,
        pub corridors_pipeline: ::wgpu::RenderPipeline,
        pub regions_pipeline: ::wgpu::RenderPipeline,
        pub uniform_buffer: ::wgpu::Buffer,
        pub uniform_bind_group: ::wgpu::BindGroup,
        pub depth_view: ::wgpu::TextureView,
        pub vertex_buffer: ::wgpu::Buffer,
        pub index_buffer: ::wgpu::Buffer,
        pub index_count: u32,
        pub graticule_vertex_buffer: ::wgpu::Buffer,
        pub graticule_vertex_count: u32,
        pub cities_vertex_buffer: ::wgpu::Buffer,
        pub cities_vertex_count: u32,
        pub corridors_vertex_buffer: ::wgpu::Buffer,
        pub corridors_vertex_count: u32,
        pub base_regions_vertex_buffer: ::wgpu::Buffer,
        pub base_regions_vertex_count: u32,
        pub regions_vertex_buffer: ::wgpu::Buffer,
        pub regions_vertex_count: u32,
        pub terrain_vertex_buffer: ::wgpu::Buffer,
        pub terrain_vertex_count: u32,
    }

    const GLOBE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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
    return vec4<f32>(base * shade, 1.0);
}
"#;

    const GRATICULE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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

    const CITIES_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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
    @location(0) center: vec3<f32>,
    @location(1) lift: f32,
    // Half-size offset in screen pixels.
    @location(2) offset_px: vec2<f32>,
    @location(3) color: vec4<f32>,
) -> VsOut {
    let n = ellipsoid_normal(center);
    let world_center = center + n * lift;

    // Project the center, then offset in clip space so size stays constant in pixels.
    let clip_center = globals.view_proj * vec4<f32>(world_center, 1.0);
    let vp = max(globals.viewport, vec2<f32>(1.0, 1.0));
    let ndc_offset = vec2<f32>(
        (offset_px.x * 2.0) / vp.x,
        (-offset_px.y * 2.0) / vp.y,
    );
    let clip = clip_center + vec4<f32>(ndc_offset * clip_center.w, 0.0, 0.0);
    return VsOut(clip, color);
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
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) along: f32,
    @location(3) side: f32,
    @location(4) lift: f32,
    @location(5) width_px: f32,
    @location(6) color: vec4<f32>,
) -> VsOut {
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
    return VsOut(clip, color);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

    const REGIONS_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    globe_color: vec3<f32>,
    stars_alpha: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

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
    @location(1) lift: f32,
    @location(2) color: vec4<f32>,
) -> VsOut {
    let n = ellipsoid_normal(position);
    let world_pos = position + n * lift;
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
    pub struct CityVertex {
        pub center: [f32; 3],
        pub lift: f32,
        pub offset_px: [f32; 2],
        pub color: [f32; 4],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct OverlayVertex {
        pub position: [f32; 3],
        pub lift: f32,
        pub color: [f32; 4],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct CorridorVertex {
        pub a: [f32; 3],
        pub b: [f32; 3],
        pub along: f32,
        pub side: f32,
        pub lift: f32,
        pub width_px: f32,
        pub color: [f32; 4],
    }

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Globals {
        view_proj: [[f32; 4]; 4],
        light_dir: [f32; 3],
        _pad0: f32,
        viewport: [f32; 2],
        _pad1: [f32; 2],
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
                pass_op: ::wgpu::StencilOperation::Replace,
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

                indices.push(i0 as u16);
                indices.push(i2 as u16);
                indices.push(i1 as u16);
                indices.push(i1 as u16);
                indices.push(i2 as u16);
                indices.push(i3 as u16);
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

        let config = ::wgpu::SurfaceConfiguration {
            usage: ::wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            desired_maximum_frame_latency: 2,
            present_mode: ::wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
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

        let regions_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-regions-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(REGIONS_SHADER)),
        });

        let uniform_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: ::wgpu::BufferUsages::STORAGE | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&::wgpu::BindGroupLayoutDescriptor {
                label: Some("atlas-globals-bgl"),
                entries: &[::wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ::wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: ::wgpu::BindingType::Buffer {
                        ty: ::wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&::wgpu::BindGroupDescriptor {
            label: Some("atlas-globals-bg"),
            layout: &uniform_bind_group_layout,
            entries: &[::wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
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

        let pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-globe-pipeline"),
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
                // Disable culling for now. If winding ends up opposite what we expect
                // (common when generating sphere indices), culling will make the globe
                // completely disappear and you'll only see the clear color.
                cull_mode: None,
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
            // Depth-test against the globe so back-side lines don't show through.
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
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CityVertex>() as ::wgpu::BufferAddress,
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
                            format: ::wgpu::VertexFormat::Float32x2,
                            offset: 16,
                            shader_location: 2,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x4,
                            offset: 24,
                            shader_location: 3,
                        },
                    ],
                }],
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
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CorridorVertex>() as ::wgpu::BufferAddress,
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
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32,
                            offset: 24,
                            shader_location: 2,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32,
                            offset: 28,
                            shader_location: 3,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32,
                            offset: 32,
                            shader_location: 4,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32,
                            offset: 36,
                            shader_location: 5,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x4,
                            offset: 40,
                            shader_location: 6,
                        },
                    ],
                }],
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

        let regions_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-regions-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &regions_shader,
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
                module: &regions_shader,
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

        let cities_vertex_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-cities-vertices"),
            contents: bytemuck::bytes_of(&CityVertex {
                center: [0.0, 0.0, 0.0],
                lift: 0.0,
                offset_px: [0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
            }),
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
        });

        let corridors_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-corridors-vertices"),
                contents: bytemuck::bytes_of(&CorridorVertex {
                    a: [0.0, 0.0, 0.0],
                    b: [0.0, 0.0, 0.0],
                    along: 0.0,
                    side: 0.0,
                    lift: 0.0,
                    width_px: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                }),
                usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            });

        let regions_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-regions-vertices"),
                contents: bytemuck::bytes_of(&OverlayVertex {
                    position: [0.0, 0.0, 0.0],
                    lift: 0.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                }),
                usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            });

        let terrain_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-terrain-vertices"),
                contents: bytemuck::bytes_of(&OverlayVertex {
                    position: [0.0, 0.0, 0.0],
                    lift: 0.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                }),
                usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            });

        let base_regions_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-base-regions-vertices"),
                contents: bytemuck::bytes_of(&OverlayVertex {
                    position: [0.0, 0.0, 0.0],
                    lift: 0.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                }),
                usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
            });

        // Initialize uniforms so the first render doesn't read uninitialized memory.
        let globals = Globals {
            view_proj: [[0.0; 4]; 4],
            light_dir: [0.4, 0.7, 0.2],
            _pad0: 0.0,
            viewport: [1.0, 1.0],
            _pad1: [0.0, 0.0],
            globe_color: [0.10, 0.55, 0.85],
            stars_alpha: 1.0,
        };
        queue.write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&globals));

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
            stars_alpha: 1.0,
            stars_pipeline,
            stars_count: 1200,
            pipeline,
            graticule_pipeline,
            cities_pipeline,
            corridors_pipeline,
            regions_pipeline,
            uniform_buffer,
            uniform_bind_group,
            depth_view,
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            graticule_vertex_buffer,
            graticule_vertex_count: graticule_vertices.len() as u32,
            cities_vertex_buffer,
            cities_vertex_count: 0,
            corridors_vertex_buffer,
            corridors_vertex_count: 0,
            terrain_vertex_buffer,
            terrain_vertex_count: 0,
            base_regions_vertex_buffer,
            base_regions_vertex_count: 0,
            regions_vertex_buffer,
            regions_vertex_count: 0,
        })
    }

    pub fn set_cities_points(ctx: &mut WgpuContext, points: &[CityVertex]) {
        if points.is_empty() {
            ctx.cities_vertex_count = 0;
            return;
        }

        ctx.cities_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-cities-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.cities_vertex_count = points.len() as u32;
    }

    pub fn set_corridors_points(ctx: &mut WgpuContext, points: &[CorridorVertex]) {
        if points.is_empty() {
            ctx.corridors_vertex_count = 0;
            return;
        }

        ctx.corridors_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-corridors-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.corridors_vertex_count = points.len() as u32;
    }

    pub fn set_regions_points(ctx: &mut WgpuContext, points: &[OverlayVertex]) {
        if points.is_empty() {
            ctx.regions_vertex_count = 0;
            return;
        }

        ctx.regions_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-regions-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.regions_vertex_count = points.len() as u32;
    }

    pub fn set_terrain_points(ctx: &mut WgpuContext, points: &[OverlayVertex]) {
        if points.is_empty() {
            ctx.terrain_vertex_count = 0;
            return;
        }

        ctx.terrain_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-terrain-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.terrain_vertex_count = points.len() as u32;
    }

    pub fn set_base_regions_points(ctx: &mut WgpuContext, points: &[OverlayVertex]) {
        if points.is_empty() {
            ctx.base_regions_vertex_count = 0;
            return;
        }

        ctx.base_regions_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-base-regions-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.base_regions_vertex_count = points.len() as u32;
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
            _pad1: [0.0, 0.0],
            globe_color: ctx.globe_color,
            stars_alpha: ctx.stars_alpha,
        };
        ctx.queue
            .write_buffer(&ctx.uniform_buffer, 0, bytemuck::bytes_of(&globals));

        let mut encoder = ctx
            .device
            .create_command_encoder(&::wgpu::CommandEncoderDescriptor {
                label: Some("atlas-mesh-encoder"),
            });

        // Pass 1: clear to deep space and draw stars (no depth attachment).
        {
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
        }

        // Pass 2: draw globe with depth, preserving the starfield color.
        {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-globe-pass"),
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

            rpass.set_pipeline(&ctx.pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            // Globe writes stencil=1 anywhere it draws.
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.set_index_buffer(ctx.index_buffer.slice(..), ::wgpu::IndexFormat::Uint16);
            rpass.draw_indexed(0..ctx.index_count, 0, 0..1);
        }

        // Pass 3 (optional): terrain mesh (depth-tested, alpha blended).
        if show_terrain && ctx.terrain_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-terrain-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.regions_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.terrain_vertex_buffer.slice(..));
            rpass.draw(0..ctx.terrain_vertex_count, 0..1);
        }

        // Pass 4 (optional): graticule overlay (depth-tested, alpha blended).
        if show_graticule {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-graticule-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.graticule_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.graticule_vertex_buffer.slice(..));
            rpass.draw(0..ctx.graticule_vertex_count, 0..1);
        }

        // Pass 5 (optional): base world polygons (depth-tested, alpha blended).
        if show_base_regions && ctx.base_regions_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-base-regions-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.regions_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.base_regions_vertex_buffer.slice(..));
            rpass.draw(0..ctx.base_regions_vertex_count, 0..1);
        }

        // Pass 6 (optional): region polygons (depth-tested, alpha blended).
        if show_regions && ctx.regions_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-regions-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.regions_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.regions_vertex_buffer.slice(..));
            rpass.draw(0..ctx.regions_vertex_count, 0..1);
        }

        // Pass 7 (optional): air corridors (depth-tested, alpha blended).
        if show_corridors && ctx.corridors_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-corridors-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.corridors_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.corridors_vertex_buffer.slice(..));
            rpass.draw(0..ctx.corridors_vertex_count, 0..1);
        }

        // Pass 8 (optional): city markers (depth-tested triangles).
        if show_cities && ctx.cities_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-cities-pass"),
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
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.cities_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_stencil_reference(1);
            rpass.set_vertex_buffer(0, ctx.cities_vertex_buffer.slice(..));
            rpass.draw(0..ctx.cities_vertex_count, 0..1);
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
        pub lift: f32,
        pub offset_px: [f32; 2],
        pub color: [f32; 4],
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct OverlayVertex {
        pub position: [f32; 3],
        pub lift: f32,
        pub color: [f32; 4],
    }

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct CorridorVertex {
        pub a: [f32; 3],
        pub b: [f32; 3],
        pub along: f32,
        pub side: f32,
        pub lift: f32,
        pub width_px: f32,
        pub color: [f32; 4],
    }

    pub fn set_cities_points(_ctx: &mut WgpuContext, _points: &[CityVertex]) {}

    pub fn set_corridors_points(_ctx: &mut WgpuContext, _points: &[CorridorVertex]) {}

    pub fn set_regions_points(_ctx: &mut WgpuContext, _points: &[OverlayVertex]) {}

    pub fn set_terrain_points(_ctx: &mut WgpuContext, _points: &[OverlayVertex]) {}

    pub fn set_base_regions_points(_ctx: &mut WgpuContext, _points: &[OverlayVertex]) {}

    pub fn set_theme(
        _ctx: &mut WgpuContext,
        _clear_color: wgpu::Color,
        _globe_color: [f32; 3],
        _stars_alpha: f32,
    ) {
    }

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
}

pub use imp::{
    CityVertex, CorridorVertex, OverlayVertex, WgpuContext, init_wgpu_from_canvas_id, render_mesh,
    resize_wgpu, set_base_regions_points, set_cities_points, set_corridors_points,
    set_regions_points, set_terrain_points, set_theme,
};
