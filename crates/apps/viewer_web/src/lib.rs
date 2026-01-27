use console_error_panic_hook::set_once;
use gloo_net::http::Request;
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use formats::SceneManifest;
use foundation::math::{WGS84_A, WGS84_B, ecef_to_geodetic};
use layers::vector::VectorLayer;
use scene::components::VectorGeometryKind;
mod wgpu;
use wgpu::{
    CityVertex, CorridorVertex, OverlayVertex, WgpuContext, init_wgpu_from_canvas_id, render_mesh,
    resize_wgpu, set_cities_points, set_corridors_points, set_regions_points,
};

#[derive(Debug, Copy, Clone)]
struct LayerStyle {
    visible: bool,
    color: [f32; 4],
    // UI lift is a fraction of Earth radius (legacy semantics).
    lift: f32,
}

#[derive(Debug, Copy, Clone)]
pub struct CameraState {
    pub yaw_rad: f64,
    pub pitch_rad: f64,
    pub distance: f64,
    pub target: [f64; 3],
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            yaw_rad: 0.6,
            pitch_rad: 0.3,
            distance: 3.0 * WGS84_A,
            target: [0.0, 0.0, 0.0],
        }
    }
}

#[derive(Debug)]
struct ViewerState {
    dataset: String,
    canvas_width: f64,
    canvas_height: f64,
    wgpu: Option<WgpuContext>,
    show_graticule: bool,
    sun_follow_real_time: bool,

    // Point marker size in screen pixels.
    city_marker_size: f32,

    // Line width in screen pixels (applies to all line layers).
    line_width_px: f32,

    // Layer styles and visibility.
    cities_style: LayerStyle,
    corridors_style: LayerStyle,
    regions_style: LayerStyle,
    uploaded_points_style: LayerStyle,
    uploaded_corridors_style: LayerStyle,
    uploaded_regions_style: LayerStyle,
    selection_style: LayerStyle,

    // Engine worlds (source-of-truth). Renderable geometry must be derived via `layers`.
    cities_world: Option<scene::World>,
    corridors_world: Option<scene::World>,
    regions_world: Option<scene::World>,
    uploaded_world: Option<scene::World>,

    // CPU-side cached geometry in viewer coordinates.
    cities_centers: Option<Vec<[f32; 3]>>,
    corridors_positions: Option<Vec<[f32; 3]>>,
    regions_positions: Option<Vec<[f32; 3]>>,

    uploaded_name: Option<String>,
    uploaded_centers: Option<Vec<[f32; 3]>>,
    uploaded_corridors_positions: Option<Vec<[f32; 3]>>,
    uploaded_regions_positions: Option<Vec<[f32; 3]>>,
    uploaded_count_points: usize,
    uploaded_count_lines: usize,
    uploaded_count_polys: usize,

    selection_center: Option<[f32; 3]>,
    selection_line_positions: Option<Vec<[f32; 3]>>,
    selection_poly_positions: Option<Vec<[f32; 3]>>,

    // Combined buffers (all visible layers), uploaded into the shared GPU buffers.
    pending_cities: Option<Vec<CityVertex>>,
    pending_corridors: Option<Vec<CorridorVertex>>,
    pending_regions: Option<Vec<OverlayVertex>>,
    frame_index: u64,
    dt_s: f64,
    time_s: f64,
    time_end_s: f64,
    camera: CameraState,
}

thread_local! {
    static STATE: RefCell<ViewerState> = RefCell::new(ViewerState {
        dataset: "cities".to_string(),
        canvas_width: 1280.0,
        canvas_height: 720.0,
        wgpu: None,
        show_graticule: false,
        sun_follow_real_time: true,
        city_marker_size: 6.0,
        line_width_px: 2.5,

        cities_style: LayerStyle { visible: false, color: [1.0, 0.25, 0.25, 0.95], lift: 0.02 },
        corridors_style: LayerStyle { visible: false, color: [1.0, 0.85, 0.25, 0.90], lift: 0.0 },
        regions_style: LayerStyle { visible: false, color: [0.10, 0.90, 0.75, 0.30], lift: 0.0 },
        uploaded_points_style: LayerStyle { visible: false, color: [0.60, 0.95, 1.00, 0.95], lift: 0.02 },
        uploaded_corridors_style: LayerStyle { visible: false, color: [0.85, 0.95, 0.60, 0.90], lift: 0.0 },
        uploaded_regions_style: LayerStyle { visible: false, color: [0.45, 0.75, 1.00, 0.25], lift: 0.0 },
        selection_style: LayerStyle { visible: true, color: [1.0, 1.0, 1.0, 0.95], lift: 0.02 },

        cities_world: None,
        corridors_world: None,
        regions_world: None,
        uploaded_world: None,

        cities_centers: None,
        corridors_positions: None,
        regions_positions: None,

        uploaded_name: None,
        uploaded_centers: None,
        uploaded_corridors_positions: None,
        uploaded_regions_positions: None,
        uploaded_count_points: 0,
        uploaded_count_lines: 0,
        uploaded_count_polys: 0,

        selection_center: None,
        selection_line_positions: None,
        selection_poly_positions: None,
        pending_cities: None,
        pending_corridors: None,
        pending_regions: None,
        frame_index: 0,
        dt_s: 1.0 / 60.0,
        time_s: 0.0,
        time_end_s: 10.0,
        camera: CameraState::default(),
    });
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

fn vec3_add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn vec3_sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vec3_mul(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn vec3_dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_norm(a: [f64; 3]) -> f64 {
    vec3_dot(a, a).sqrt()
}

fn vec3_normalize(a: [f64; 3]) -> [f64; 3] {
    let n = vec3_norm(a);
    if n <= 0.0 {
        [0.0, 0.0, 0.0]
    } else {
        vec3_mul(a, 1.0 / n)
    }
}

fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Column-major matrix multiply: c = a * b
    let mut c = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            c[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    c
}

fn mat4_perspective_rh_z0(fov_y_rad: f64, aspect: f64, near: f64, far: f64) -> [[f32; 4]; 4] {
    let f = 1.0 / (0.5 * fov_y_rad).tan();
    let m00 = (f / aspect) as f32;
    let m11 = f as f32;
    let m22 = (far / (near - far)) as f32;
    let m23 = ((near * far) / (near - far)) as f32;

    // Column-major (WGSL) perspective matrix, RH, depth range [0, 1].
    [
        [m00, 0.0, 0.0, 0.0],
        [0.0, m11, 0.0, 0.0],
        [0.0, 0.0, m22, -1.0],
        [0.0, 0.0, m23, 0.0],
    ]
}

fn mat4_look_at_rh(eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> [[f32; 4]; 4] {
    let f = vec3_normalize(vec3_sub(target, eye));
    let s = vec3_normalize(vec3_cross(f, up));
    let u = vec3_cross(s, f);

    let ex = -vec3_dot(s, eye);
    let ey = -vec3_dot(u, eye);
    let ez = vec3_dot(f, eye);

    // Column-major (WGSL) view matrix.
    [
        [s[0] as f32, s[1] as f32, s[2] as f32, 0.0],
        [u[0] as f32, u[1] as f32, u[2] as f32, 0.0],
        [(-f[0]) as f32, (-f[1]) as f32, (-f[2]) as f32, 0.0],
        [ex as f32, ey as f32, ez as f32, 1.0],
    ]
}

fn camera_view_proj(camera: CameraState, canvas_width: f64, canvas_height: f64) -> [[f32; 4]; 4] {
    let aspect = if canvas_height <= 0.0 {
        1.0
    } else {
        (canvas_width / canvas_height).max(1e-6)
    };

    let dir = [
        camera.pitch_rad.cos() * camera.yaw_rad.cos(),
        camera.pitch_rad.sin(),
        camera.pitch_rad.cos() * camera.yaw_rad.sin(),
    ];
    let eye = vec3_add(camera.target, vec3_mul(dir, camera.distance));
    let view = mat4_look_at_rh(eye, camera.target, [0.0, 1.0, 0.0]);

    // Dynamic clipping planes to keep depth precision reasonable across Earth-scale zoom.
    // A fixed far plane (Earth * 100s) makes the depth buffer too coarse and causes
    // severe z-fighting between the globe and draped overlays.
    let near = (camera.distance * 0.001).max(10.0);
    let far = (camera.distance * 4.0 + 4.0 * WGS84_A).max(near + 1.0);
    let proj = mat4_perspective_rh_z0(45f64.to_radians(), aspect, near, far);
    mat4_mul(proj, view)
}

fn current_sun_direction_world() -> Option<[f32; 3]> {
    // Low-cost solar position approximation.
    fn wrap_360(mut d: f64) -> f64 {
        d %= 360.0;
        if d < 0.0 {
            d += 360.0;
        }
        d
    }
    fn wrap_180(mut d: f64) -> f64 {
        d = wrap_360(d);
        if d > 180.0 {
            d -= 360.0;
        }
        d
    }

    let ms = js_sys::Date::new_0().get_time();
    if !ms.is_finite() {
        return None;
    }

    // Julian Day from Unix epoch (1970-01-01T00:00:00Z) == 2440587.5
    let jd = 2440587.5 + (ms / 86_400_000.0);
    let n = jd - 2451545.0; // days since J2000

    // Mean longitude and anomaly (degrees)
    let l = wrap_360(280.46 + 0.9856474 * n);
    let g = wrap_360(357.528 + 0.9856003 * n);

    // Ecliptic longitude (degrees)
    let lambda = wrap_360(l + 1.915 * g.to_radians().sin() + 0.020 * (2.0 * g).to_radians().sin());
    // Obliquity of the ecliptic (degrees)
    let epsilon = 23.439 - 0.0000004 * n;

    // Right ascension (alpha) and declination (delta)
    let lambda_rad = lambda.to_radians();
    let eps_rad = epsilon.to_radians();
    let alpha = (eps_rad.cos() * lambda_rad.sin())
        .atan2(lambda_rad.cos())
        .to_degrees();
    let delta = (eps_rad.sin() * lambda_rad.sin()).asin().to_degrees();

    // Greenwich Mean Sidereal Time (degrees)
    let t = (jd - 2451545.0) / 36525.0;
    let gmst = wrap_360(
        280.46061837 + 360.98564736629 * (jd - 2451545.0) + 0.000387933 * t * t
            - (t * t * t) / 38710000.0,
    );

    // Subsolar point
    let subsolar_lon = wrap_180(alpha - gmst);
    let subsolar_lat = delta;

    // Convert to a unit direction in ECEF, then map to viewer coords: (x, z, -y)
    let lon = subsolar_lon.to_radians();
    let lat = subsolar_lat.to_radians();
    let cos_lat = lat.cos();
    let ecef = [cos_lat * lon.cos(), cos_lat * lon.sin(), lat.sin()];
    Some([ecef[0] as f32, ecef[2] as f32, (-ecef[1]) as f32])
}

fn render_scene() -> Result<(), JsValue> {
    STATE.with(|state_ref| {
        let state = state_ref.borrow();
        if let Some(ctx) = &state.wgpu {
            let view_proj = camera_view_proj(state.camera, state.canvas_width, state.canvas_height);

            let light_dir = if state.sun_follow_real_time {
                current_sun_direction_world().unwrap_or([0.4, 0.7, 0.2])
            } else {
                [0.4, 0.7, 0.2]
            };

            // Layer visibility is baked into the combined overlay buffers; we can always
            // attempt to draw and rely on vertex counts to early-out.
            let show_cities = true;
            let show_corridors = true;
            let show_regions = true;
            let _ = render_mesh(
                ctx,
                view_proj,
                light_dir,
                state.show_graticule,
                show_cities,
                show_corridors,
                show_regions,
            );
        }
    });
    Ok(())
}

fn build_city_vertices(
    centers: &[[f32; 3]],
    size_px: f32,
    color: [f32; 4],
    lift: f32,
) -> Vec<CityVertex> {
    let size_px = size_px.clamp(1.0, 64.0);
    let mut out: Vec<CityVertex> = Vec::with_capacity(centers.len() * 6);
    for &c in centers {
        let v0 = CityVertex {
            center: c,
            lift,
            offset_px: [-size_px, -size_px],
            color,
        };
        let v1 = CityVertex {
            center: c,
            lift,
            offset_px: [size_px, -size_px],
            color,
        };
        let v2 = CityVertex {
            center: c,
            lift,
            offset_px: [size_px, size_px],
            color,
        };
        let v3 = CityVertex {
            center: c,
            lift,
            offset_px: [-size_px, size_px],
            color,
        };

        // Two triangles: (v0,v1,v2) and (v0,v2,v3)
        out.extend([v0, v1, v2, v0, v2, v3]);
    }
    out
}

fn ecef_vec3_to_viewer_f32(p: foundation::math::Vec3) -> [f32; 3] {
    // Viewer coordinates are a permuted ECEF: viewer = (x, z, -y)
    [p.x as f32, p.z as f32, (-p.y) as f32]
}

fn world_from_vector_chunk(
    chunk: &formats::VectorChunk,
    expected_kind: Option<VectorGeometryKind>,
) -> scene::World {
    let mut world = scene::World::new();
    scene::prefabs::spawn_wgs84_globe(&mut world);
    formats::ingest_vector_chunk(&mut world, chunk, expected_kind);
    world
}

fn build_corridor_vertices(
    positions_line_list: &[[f32; 3]],
    width_px: f32,
    color: [f32; 4],
    lift: f32,
) -> Vec<CorridorVertex> {
    let width_px = width_px.clamp(1.0, 24.0);
    // `lift` is a fraction of Earth radius (legacy UI semantics); convert to meters.
    let mut lift_m = lift * (WGS84_A as f32);
    if lift_m <= 0.0 {
        // Keep corridors slightly above the surface to avoid z-fighting.
        lift_m = 50.0;
    }
    let seg_count = positions_line_list.len() / 2;
    let mut out: Vec<CorridorVertex> = Vec::with_capacity(seg_count.saturating_mul(6));

    for seg in positions_line_list.chunks_exact(2).take(250_000) {
        let a = seg[0];
        let b = seg[1];

        // Skip degenerate segments.
        let dx = a[0] - b[0];
        let dy = a[1] - b[1];
        let dz = a[2] - b[2];
        if dx * dx + dy * dy + dz * dz < 1e-12 {
            continue;
        }

        let v00 = CorridorVertex {
            a,
            b,
            along: 0.0,
            side: -1.0,
            lift: lift_m,
            width_px,
            color,
        };
        let v01 = CorridorVertex { side: 1.0, ..v00 };
        let v11 = CorridorVertex {
            along: 1.0,
            side: 1.0,
            ..v00
        };
        let v10 = CorridorVertex {
            along: 1.0,
            side: -1.0,
            ..v00
        };

        // Two triangles for the segment quad.
        out.extend([v00, v01, v11, v00, v11, v10]);
    }

    out
}

fn rebuild_overlays_and_upload() {
    STATE.with(|state| {
        let mut s = state.borrow_mut();

        // Refresh cached viewer-space geometry from the engine worlds.
        // This ensures all rendered features flow through `layers`.
        let layer = VectorLayer::new(1);

        // Built-ins
        s.cities_centers = s.cities_world.as_ref().map(|w| {
            let snap = layer.extract(w);
            snap.points
                .into_iter()
                .map(ecef_vec3_to_viewer_f32)
                .collect::<Vec<_>>()
        });

        s.corridors_positions = s.corridors_world.as_ref().map(|w| {
            let snap = layer.extract(w);
            let mut out: Vec<[f32; 3]> = Vec::new();
            for line in snap.lines {
                for seg in line.windows(2) {
                    out.push(ecef_vec3_to_viewer_f32(seg[0]));
                    out.push(ecef_vec3_to_viewer_f32(seg[1]));
                }
            }
            out
        });

        s.regions_positions = s.regions_world.as_ref().map(|w| {
            let snap = layer.extract(w);
            snap.area_triangles
                .into_iter()
                .map(ecef_vec3_to_viewer_f32)
                .collect::<Vec<_>>()
        });

        // Uploaded
        if let Some(w) = s.uploaded_world.as_ref() {
            let snap = layer.extract(w);
            s.uploaded_centers = Some(
                snap.points
                    .into_iter()
                    .map(ecef_vec3_to_viewer_f32)
                    .collect::<Vec<_>>(),
            );
            s.uploaded_corridors_positions = Some({
                let mut out: Vec<[f32; 3]> = Vec::new();
                for line in snap.lines {
                    for seg in line.windows(2) {
                        out.push(ecef_vec3_to_viewer_f32(seg[0]));
                        out.push(ecef_vec3_to_viewer_f32(seg[1]));
                    }
                }
                out
            });
            s.uploaded_regions_positions = Some(
                snap.area_triangles
                    .into_iter()
                    .map(ecef_vec3_to_viewer_f32)
                    .collect::<Vec<_>>(),
            );
        } else {
            s.uploaded_centers = None;
            s.uploaded_corridors_positions = None;
            s.uploaded_regions_positions = None;
        }

        let mut points: Vec<CityVertex> = Vec::new();
        let mut lines: Vec<CorridorVertex> = Vec::new();
        let mut polys: Vec<OverlayVertex> = Vec::new();

        // Points
        if s.cities_style.visible
            && let Some(centers) = s.cities_centers.as_deref()
        {
            points.extend(build_city_vertices(
                centers,
                s.city_marker_size,
                s.cities_style.color,
                s.cities_style.lift,
            ));
        }
        if s.uploaded_points_style.visible
            && let Some(centers) = s.uploaded_centers.as_deref()
        {
            points.extend(build_city_vertices(
                centers,
                s.city_marker_size,
                s.uploaded_points_style.color,
                s.uploaded_points_style.lift,
            ));
        }
        if s.selection_style.visible
            && let Some(c) = s.selection_center
        {
            points.extend(build_city_vertices(
                std::slice::from_ref(&c),
                s.city_marker_size * 1.35,
                s.selection_style.color,
                s.selection_style.lift,
            ));
        }

        // Lines (thick quads)
        if s.corridors_style.visible
            && let Some(pos) = s.corridors_positions.as_deref()
        {
            lines.extend(build_corridor_vertices(
                pos,
                s.line_width_px,
                s.corridors_style.color,
                s.corridors_style.lift,
            ));
        }
        if s.uploaded_corridors_style.visible
            && let Some(pos) = s.uploaded_corridors_positions.as_deref()
        {
            lines.extend(build_corridor_vertices(
                pos,
                s.line_width_px,
                s.uploaded_corridors_style.color,
                s.uploaded_corridors_style.lift,
            ));
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_line_positions.as_deref()
            && !pos.is_empty()
        {
            lines.extend(build_corridor_vertices(
                pos,
                (s.line_width_px * 1.6).clamp(1.0, 24.0),
                s.selection_style.color,
                s.selection_style.lift + 0.03,
            ));
        }

        // Polygons (triangles)
        if s.regions_style.visible
            && let Some(pos) = s.regions_positions.as_deref()
        {
            let mut lift_m = s.regions_style.lift * (WGS84_A as f32);
            if lift_m <= 0.0 {
                // Keep area fills slightly above the surface to avoid z-fighting.
                lift_m = 50.0;
            }
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                lift: lift_m,
                color: s.regions_style.color,
            }));
        }
        if s.uploaded_regions_style.visible
            && let Some(pos) = s.uploaded_regions_positions.as_deref()
        {
            let mut lift_m = s.uploaded_regions_style.lift * (WGS84_A as f32);
            if lift_m <= 0.0 {
                lift_m = 50.0;
            }
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                lift: lift_m,
                color: s.uploaded_regions_style.color,
            }));
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_poly_positions.as_deref()
            && !pos.is_empty()
        {
            let lift_m = (s.selection_style.lift + 0.03) * (WGS84_A as f32);
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                lift: lift_m,
                color: s.selection_style.color,
            }));
        }

        if let Some(ctx) = &mut s.wgpu {
            set_cities_points(ctx, &points);
            set_corridors_points(ctx, &lines);
            set_regions_points(ctx, &polys);
            s.pending_cities = None;
            s.pending_corridors = None;
            s.pending_regions = None;
        } else {
            s.pending_cities = Some(points);
            s.pending_corridors = Some(lines);
            s.pending_regions = Some(polys);
        }
    });
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    set_once();
    Ok(())
}

#[wasm_bindgen]
pub fn init_wgpu() {
    spawn_local(async move {
        if let Err(err) = init_wgpu_inner().await {
            web_sys::console::log_1(&JsValue::from_str(&format!("wgpu init error: {:?}", err)));
        }
    });
}

#[wasm_bindgen]
pub fn set_canvas_sizes(width: f64, height: f64) {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.canvas_width = width;
        s.canvas_height = height;
        if let Some(ctx) = &mut s.wgpu {
            resize_wgpu(ctx, width as u32, height as u32);
        }
    });
}

#[wasm_bindgen]
pub fn camera_reset() -> Result<(), JsValue> {
    STATE.with(|state| {
        state.borrow_mut().camera = CameraState::default();
    });
    render_scene()
}

/// Orbit around the globe.
///
/// Intended usage: call with pointer delta in pixels.
#[wasm_bindgen]
pub fn camera_orbit(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let speed = 0.005;
        s.camera.yaw_rad += delta_x_px * speed;
        s.camera.pitch_rad = clamp(s.camera.pitch_rad + delta_y_px * speed, -1.55, 1.55);
    });
    render_scene()
}

/// Pan the camera target.
///
/// Intended usage: call with pointer delta in pixels.
#[wasm_bindgen]
pub fn camera_pan(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let cam = s.camera;

        let dir = [
            cam.pitch_rad.cos() * cam.yaw_rad.cos(),
            cam.pitch_rad.sin(),
            cam.pitch_rad.cos() * cam.yaw_rad.sin(),
        ];
        let forward = vec3_normalize(vec3_mul(dir, -1.0));
        let up = [0.0, 1.0, 0.0];
        let right = vec3_normalize(vec3_cross(forward, up));
        let real_up = vec3_cross(right, forward);

        let pan_scale = cam.distance * 0.002;
        let delta = vec3_add(
            vec3_mul(right, -delta_x_px * pan_scale),
            vec3_mul(real_up, delta_y_px * pan_scale),
        );
        s.camera.target = vec3_add(s.camera.target, delta);
    });
    render_scene()
}

/// Zoom (dolly) in/out.
///
/// Intended usage: call with wheel deltaY.
#[wasm_bindgen]
pub fn camera_zoom(wheel_delta_y: f64) -> Result<(), JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let cam = s.camera;
        let zoom = (wheel_delta_y * 0.0015).exp();

        // Keep this a soft clamp (mostly for UX), but also ensure the *camera eye*
        // cannot go inside the globe when orbiting around/near it.
        let min_dist = 10.0;
        let max_dist = 200.0 * WGS84_A;
        let mut dist = clamp(cam.distance * zoom, min_dist, max_dist);

        let dir_cam = [
            cam.pitch_rad.cos() * cam.yaw_rad.cos(),
            cam.pitch_rad.sin(),
            cam.pitch_rad.cos() * cam.yaw_rad.sin(),
        ];
        let dir_cam = vec3_normalize(dir_cam);
        let eye = vec3_add(cam.target, vec3_mul(dir_cam, dist));

        // Conservative: enforce the camera is outside a sphere of radius WGS84_A.
        // (Good enough to prevent pathological inside-the-globe behavior, even
        // though the visual globe is an ellipsoid.)
        let min_eye_r = 1.001 * WGS84_A;
        let eye_r = vec3_dot(eye, eye).sqrt();
        if eye_r < min_eye_r {
            // Solve |target + dir*t| = min_eye_r for t, and move to the exiting root.
            let b = 2.0 * vec3_dot(cam.target, dir_cam);
            let c = vec3_dot(cam.target, cam.target) - min_eye_r * min_eye_r;
            let disc = b * b - 4.0 * c;
            if disc >= 0.0 {
                let sdisc = disc.sqrt();
                let t0 = (-b - sdisc) / 2.0;
                let t1 = (-b + sdisc) / 2.0;
                let t = t0.max(t1);
                if t.is_finite() && t > 0.0 {
                    dist = clamp(t, min_dist, max_dist);
                }
            }
        }

        s.camera.distance = dist;
    });
    render_scene()
}

#[wasm_bindgen]
pub fn set_dataset(dataset: &str) -> Result<(), JsValue> {
    let dataset = dataset.to_string();
    let should_load_cities = dataset == "cities";
    let should_load_corridors = dataset == "air_corridors";
    let should_load_regions = dataset == "regions";
    let should_load_uploaded = dataset == "__uploaded__";

    STATE.with(|state| {
        state.borrow_mut().dataset = dataset;
    });

    if should_load_cities {
        spawn_local(async move {
            let chunk = match fetch_vector_chunk("assets/chunks/cities.json").await {
                Ok(c) => c,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch cities: {:?}",
                        err
                    )));
                    return;
                }
            };

            let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Point));

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.cities_world = Some(world);
                s.cities_style.visible = true;
            });
            rebuild_overlays_and_upload();
            let _ = render_scene();
        });
    }

    if should_load_corridors {
        spawn_local(async move {
            let chunk = match fetch_vector_chunk("assets/chunks/air_corridors.json").await {
                Ok(c) => c,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch air corridors: {:?}",
                        err
                    )));
                    return;
                }
            };

            let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Line));

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.corridors_world = Some(world);
                s.corridors_style.visible = true;
            });
            rebuild_overlays_and_upload();
            let _ = render_scene();
        });
    }

    if should_load_regions {
        spawn_local(async move {
            let chunk = match fetch_vector_chunk("assets/chunks/regions.json").await {
                Ok(c) => c,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch regions: {:?}",
                        err
                    )));
                    return;
                }
            };

            let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Area));

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.regions_world = Some(world);
                s.regions_style.visible = true;
            });
            rebuild_overlays_and_upload();
            let _ = render_scene();
        });
    }

    if should_load_uploaded {
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.uploaded_points_style.visible = s.uploaded_count_points > 0;
            s.uploaded_corridors_style.visible = s.uploaded_count_lines > 0;
            s.uploaded_regions_style.visible = s.uploaded_count_polys > 0;
        });
        rebuild_overlays_and_upload();
    }

    // Render immediately so selection changes are responsive.
    render_scene()?;
    Ok(())
}

#[wasm_bindgen]
pub fn set_city_marker_size(size: f64) -> Result<(), JsValue> {
    // Size is in screen pixels.
    let size = (size as f32).clamp(1.0, 64.0);

    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.city_marker_size = size;
    });

    rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn set_line_width_px(width_px: f64) -> Result<(), JsValue> {
    let width_px = (width_px as f32).clamp(1.0, 24.0);
    STATE.with(|state| {
        state.borrow_mut().line_width_px = width_px;
    });
    rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn set_graticule_enabled(enabled: bool) -> Result<(), JsValue> {
    STATE.with(|state| {
        state.borrow_mut().show_graticule = enabled;
    });
    // Render immediately so the toggle feels responsive.
    render_scene()
}

#[wasm_bindgen]
pub fn set_real_time_sun_enabled(enabled: bool) -> Result<(), JsValue> {
    STATE.with(|state| {
        state.borrow_mut().sun_follow_real_time = enabled;
    });
    render_scene()
}

fn layer_style_mut<'a>(s: &'a mut ViewerState, id: &str) -> Option<&'a mut LayerStyle> {
    match id {
        "cities" => Some(&mut s.cities_style),
        "air_corridors" => Some(&mut s.corridors_style),
        "regions" => Some(&mut s.regions_style),
        "uploaded_points" => Some(&mut s.uploaded_points_style),
        "uploaded_corridors" => Some(&mut s.uploaded_corridors_style),
        "uploaded_regions" => Some(&mut s.uploaded_regions_style),
        "selection" => Some(&mut s.selection_style),
        _ => None,
    }
}

#[wasm_bindgen]
pub fn set_layer_visible(id: &str, visible: bool) -> Result<(), JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.visible = visible;
        }
    });
    rebuild_overlays_and_upload();
    render_scene()
}

fn parse_hex_color(s: &str) -> Option<[f32; 3]> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

#[wasm_bindgen]
pub fn set_layer_color_hex(id: &str, hex: &str) -> Result<(), JsValue> {
    let rgb = parse_hex_color(hex).ok_or_else(|| JsValue::from_str("Invalid color"))?;
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.color[0] = rgb[0];
            st.color[1] = rgb[1];
            st.color[2] = rgb[2];
        }
    });
    rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn set_layer_opacity(id: &str, opacity: f64) -> Result<(), JsValue> {
    let a = (opacity as f32).clamp(0.0, 1.0);
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.color[3] = a;
        }
    });
    rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn set_layer_lift(id: &str, lift: f64) -> Result<(), JsValue> {
    let lift = (lift as f32).clamp(-0.1, 0.2);
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.lift = lift;
        }
    });
    rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn get_uploaded_summary() -> Result<JsValue, JsValue> {
    let summary = js_sys::Object::new();
    STATE.with(|state| {
        let s = state.borrow();
        let name = s.uploaded_name.clone().unwrap_or_else(|| "".to_string());
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("name"),
            &JsValue::from_str(&name),
        );
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("points"),
            &JsValue::from_f64(s.uploaded_count_points as f64),
        );
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("lines"),
            &JsValue::from_f64(s.uploaded_count_lines as f64),
        );
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("polygons"),
            &JsValue::from_f64(s.uploaded_count_polys as f64),
        );
    });
    Ok(summary.into())
}

fn world_to_lon_lat_deg(p: [f64; 3]) -> (f64, f64) {
    // Convert viewer coordinates back to ECEF (z-up): ecef = (x, -z, y)
    let ecef = foundation::math::Ecef::new(p[0], -p[2], p[1]);
    let geo = ecef_to_geodetic(ecef);
    (geo.lon_rad.to_degrees(), geo.lat_rad.to_degrees())
}

fn ray_hit_globe(
    camera: CameraState,
    canvas_w: f64,
    canvas_h: f64,
    x_px: f64,
    y_px: f64,
) -> Option<[f64; 3]> {
    if canvas_w <= 1.0 || canvas_h <= 1.0 {
        return None;
    }
    let aspect = canvas_w / canvas_h;
    let fov_y = 45f64.to_radians();
    let tan = (0.5 * fov_y).tan();

    let dir_cam = [
        camera.pitch_rad.cos() * camera.yaw_rad.cos(),
        camera.pitch_rad.sin(),
        camera.pitch_rad.cos() * camera.yaw_rad.sin(),
    ];
    let eye = vec3_add(camera.target, vec3_mul(dir_cam, camera.distance));
    let forward = vec3_normalize(vec3_sub(camera.target, eye));
    let world_up = [0.0, 1.0, 0.0];
    let right = vec3_normalize(vec3_cross(forward, world_up));
    let up = vec3_cross(right, forward);

    let ndc_x = (2.0 * (x_px / canvas_w) - 1.0) * aspect;
    let ndc_y = 1.0 - 2.0 * (y_px / canvas_h);
    let px = ndc_x * tan;
    let py = ndc_y * tan;

    let ray_dir = vec3_normalize(vec3_add(
        forward,
        vec3_add(vec3_mul(right, px), vec3_mul(up, py)),
    ));

    // Ray-ellipsoid intersection for the WGS84 globe centered at origin.
    // Ellipsoid equation: (x/rx)^2 + (y/ry)^2 + (z/rz)^2 = 1.
    // Our viewer coordinates map ECEF (x,y,z) -> (x, z, -y), so the radii are:
    // x: WGS84_A, y: WGS84_B, z: WGS84_A.
    let rx = WGS84_A;
    let ry = WGS84_B;
    let rz = WGS84_A;
    let inv_rx2 = 1.0 / (rx * rx);
    let inv_ry2 = 1.0 / (ry * ry);
    let inv_rz2 = 1.0 / (rz * rz);

    let a = ray_dir[0] * ray_dir[0] * inv_rx2
        + ray_dir[1] * ray_dir[1] * inv_ry2
        + ray_dir[2] * ray_dir[2] * inv_rz2;
    if a.abs() < 1e-18 {
        return None;
    }
    let b = 2.0
        * (eye[0] * ray_dir[0] * inv_rx2
            + eye[1] * ray_dir[1] * inv_ry2
            + eye[2] * ray_dir[2] * inv_rz2);
    let c = eye[0] * eye[0] * inv_rx2 + eye[1] * eye[1] * inv_ry2 + eye[2] * eye[2] * inv_rz2 - 1.0;

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sdisc = disc.sqrt();
    let t0 = (-b - sdisc) / (2.0 * a);
    let t1 = (-b + sdisc) / (2.0 * a);

    // Choose the nearest positive hit.
    let t = if t0 > 0.0 && t1 > 0.0 {
        t0.min(t1)
    } else if t0 > 0.0 {
        t0
    } else if t1 > 0.0 {
        t1
    } else {
        return None;
    };

    Some(vec3_add(eye, vec3_mul(ray_dir, t)))
}

fn mat4_mul_vec4(m: [[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    // Column-major: m[col][row]
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0] * v[3],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1] * v[3],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2] * v[3],
        m[0][3] * v[0] + m[1][3] * v[1] + m[2][3] * v[2] + m[3][3] * v[3],
    ]
}

#[wasm_bindgen]
pub fn cursor_move(x_px: f64, y_px: f64) -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();
    let hit = STATE.with(|state| {
        let s = state.borrow();
        ray_hit_globe(s.camera, s.canvas_width, s.canvas_height, x_px, y_px)
    });
    if let Some(p) = hit {
        let (lon, lat) = world_to_lon_lat_deg(p);
        js_sys::Reflect::set(&out, &JsValue::from_str("hit"), &JsValue::TRUE)?;
        js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
        js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
    } else {
        js_sys::Reflect::set(&out, &JsValue::from_str("hit"), &JsValue::FALSE)?;
    }
    Ok(out.into())
}

#[wasm_bindgen]
pub fn cursor_click(x_px: f64, y_px: f64) -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();

    let hit = STATE.with(|state| {
        let s = state.borrow();
        ray_hit_globe(s.camera, s.canvas_width, s.canvas_height, x_px, y_px)
    });
    if hit.is_none() {
        // Clear selection if nothing hit on globe.
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.selection_center = None;
            s.selection_line_positions = None;
            s.selection_poly_positions = None;
        });
        rebuild_overlays_and_upload();
        let _ = render_scene();
        js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::FALSE)?;
        return Ok(out.into());
    }

    // Prefer: point (near) -> line (near) -> polygon (inside).
    let picked = STATE.with(|state| {
        let s = state.borrow();
        let view_proj = camera_view_proj(s.camera, s.canvas_width, s.canvas_height);
        let w = s.canvas_width.max(1.0) as f32;
        let h = s.canvas_height.max(1.0) as f32;
        let px = x_px as f32;
        let py = y_px as f32;

        let project = |p: [f32; 3]| -> Option<(f32, f32)> {
            let clip = mat4_mul_vec4(view_proj, [p[0], p[1], p[2], 1.0]);
            if clip[3].abs() < 1e-6 {
                return None;
            }
            let ndc_x = clip[0] / clip[3];
            let ndc_y = clip[1] / clip[3];
            let sx = (ndc_x * 0.5 + 0.5) * w;
            let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h;
            Some((sx, sy))
        };

        // 1) Points
        let mut best_point: Option<([f32; 3], f32)> = None;
        let mut consider_points = |centers: &[[f32; 3]]| {
            for &c in centers.iter().take(150_000) {
                let Some((sx, sy)) = project(c) else {
                    continue;
                };
                let dx = sx - px;
                let dy = sy - py;
                let d2 = dx * dx + dy * dy;
                if best_point.map(|(_, bd2)| d2 < bd2).unwrap_or(true) {
                    best_point = Some((c, d2));
                }
            }
        };
        if s.cities_style.visible
            && let Some(centers) = s.cities_centers.as_deref()
        {
            consider_points(centers);
        }
        if s.uploaded_points_style.visible
            && let Some(centers) = s.uploaded_centers.as_deref()
        {
            consider_points(centers);
        }
        if let Some((c, d2)) = best_point {
            // Approximate marker radius in pixels by projecting a small tangent offset.
            let mut radius_px = 12.0f32;
            if let Some((cx, cy)) = project(c) {
                // World units are meters; approximate pixel radius by projecting a tangent offset.
                // Use an ellipsoid normal (viewer radii: x=A, y=B, z=A).
                let a2 = WGS84_A * WGS84_A;
                let b2 = WGS84_B * WGS84_B;
                let n =
                    vec3_normalize([(c[0] as f64) / a2, (c[1] as f64) / b2, (c[2] as f64) / a2]);
                let up = if n[1].abs() < 0.99 {
                    [0.0f64, 1.0, 0.0]
                } else {
                    [1.0f64, 0.0, 0.0]
                };
                let east = vec3_normalize(vec3_cross(up, n));
                let half_size_m = s.city_marker_size as f64;
                let p = [
                    (c[0] as f64 + east[0] * half_size_m) as f32,
                    (c[1] as f64 + east[1] * half_size_m) as f32,
                    (c[2] as f64 + east[2] * half_size_m) as f32,
                ];
                if let Some((px2, py2)) = project(p) {
                    let dx = px2 - cx;
                    let dy = py2 - cy;
                    radius_px = (dx * dx + dy * dy).sqrt().clamp(6.0, 64.0);
                }
            }
            if d2 <= radius_px * radius_px {
                return Some(("point".to_string(), Some(c), None, None));
            }
        }

        // 2) Lines (distance to segment)
        let mut best_line: Option<([f32; 3], [f32; 3], f32)> = None;
        let mut consider_lines = |pos: &[[f32; 3]]| {
            for seg in pos.chunks_exact(2).take(200_000) {
                let a = seg[0];
                let b = seg[1];
                let Some((ax, ay)) = project(a) else {
                    continue;
                };
                let Some((bx, by)) = project(b) else {
                    continue;
                };

                let abx = bx - ax;
                let aby = by - ay;
                let apx = px - ax;
                let apy = py - ay;
                let denom = abx * abx + aby * aby;
                if denom < 1e-6 {
                    continue;
                }
                let mut t = (apx * abx + apy * aby) / denom;
                t = t.clamp(0.0, 1.0);
                let cx = ax + t * abx;
                let cy = ay + t * aby;
                let dx = px - cx;
                let dy = py - cy;
                let d2 = dx * dx + dy * dy;
                if best_line.map(|(_, _, bd2)| d2 < bd2).unwrap_or(true) {
                    best_line = Some((a, b, d2));
                }
            }
        };
        if s.corridors_style.visible
            && let Some(pos) = s.corridors_positions.as_deref()
        {
            consider_lines(pos);
        }
        if s.uploaded_corridors_style.visible
            && let Some(pos) = s.uploaded_corridors_positions.as_deref()
        {
            consider_lines(pos);
        }
        if let Some((a, b, d2)) = best_line {
            let radius_px = (s.line_width_px * 0.5 + 6.0).max(6.0);
            if d2 <= radius_px * radius_px {
                return Some(("line".to_string(), None, Some(vec![a, b]), None));
            }
        }

        // 3) Polygons (test projected triangles)
        #[allow(clippy::too_many_arguments)]
        fn point_in_tri(
            px: f32,
            py: f32,
            ax: f32,
            ay: f32,
            bx: f32,
            by: f32,
            cx: f32,
            cy: f32,
        ) -> bool {
            // Barycentric technique.
            let v0x = cx - ax;
            let v0y = cy - ay;
            let v1x = bx - ax;
            let v1y = by - ay;
            let v2x = px - ax;
            let v2y = py - ay;

            let dot00 = v0x * v0x + v0y * v0y;
            let dot01 = v0x * v1x + v0y * v1y;
            let dot02 = v0x * v2x + v0y * v2y;
            let dot11 = v1x * v1x + v1y * v1y;
            let dot12 = v1x * v2x + v1y * v2y;

            let denom = dot00 * dot11 - dot01 * dot01;
            if denom.abs() < 1e-8 {
                return false;
            }
            let inv = 1.0 / denom;
            let u = (dot11 * dot02 - dot01 * dot12) * inv;
            let v = (dot00 * dot12 - dot01 * dot02) * inv;
            u >= 0.0 && v >= 0.0 && (u + v) <= 1.0
        }

        let consider_polys = |pos: &[[f32; 3]]| -> Option<Vec<[f32; 3]>> {
            for tri in pos.chunks_exact(3).take(60_000) {
                let a = tri[0];
                let b = tri[1];
                let c = tri[2];
                let Some((ax, ay)) = project(a) else {
                    continue;
                };
                let Some((bx, by)) = project(b) else {
                    continue;
                };
                let Some((cx, cy)) = project(c) else {
                    continue;
                };
                if point_in_tri(px, py, ax, ay, bx, by, cx, cy) {
                    return Some(vec![a, b, c]);
                }
            }
            None
        };

        if s.regions_style.visible
            && let Some(pos) = s.regions_positions.as_deref()
            && let Some(tri) = consider_polys(pos)
        {
            return Some(("polygon".to_string(), None, None, Some(tri)));
        }
        if s.uploaded_regions_style.visible
            && let Some(pos) = s.uploaded_regions_positions.as_deref()
            && let Some(tri) = consider_polys(pos)
        {
            return Some(("polygon".to_string(), None, None, Some(tri)));
        }

        None
    });

    if let Some((kind, point, line, poly)) = picked {
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.selection_center = point;
            s.selection_line_positions = line;
            s.selection_poly_positions = poly;
        });
        rebuild_overlays_and_upload();
        js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::TRUE)?;
        js_sys::Reflect::set(&out, &JsValue::from_str("kind"), &JsValue::from_str(&kind))?;

        // Prefer returning lon/lat for the picked point itself.
        if let Some(c) = point {
            let (lon, lat) = world_to_lon_lat_deg([c[0] as f64, c[1] as f64, c[2] as f64]);
            js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
            js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
        } else if let Some(p) = hit {
            let (lon, lat) = world_to_lon_lat_deg(p);
            js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
            js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
        }

        let _ = render_scene();
        return Ok(out.into());
    }

    // Clear selection if nothing picked.
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.selection_center = None;
        s.selection_line_positions = None;
        s.selection_poly_positions = None;
    });
    rebuild_overlays_and_upload();
    let _ = render_scene();
    js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::FALSE)?;
    Ok(out.into())
}

#[wasm_bindgen]
pub fn load_geojson_file(name: String, geojson_text: String) -> Result<JsValue, JsValue> {
    let chunk = formats::VectorChunk::from_geojson_str(&geojson_text)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    // Count primitives for UI.
    let mut count_points = 0usize;
    let mut count_lines = 0usize;
    let mut count_polys = 0usize;
    for f in &chunk.features {
        match &f.geometry {
            formats::VectorGeometry::Point(_) => count_points += 1,
            formats::VectorGeometry::MultiPoint(v) => count_points += v.len(),
            formats::VectorGeometry::LineString(_) => count_lines += 1,
            formats::VectorGeometry::MultiLineString(v) => count_lines += v.len(),
            formats::VectorGeometry::Polygon(_) => count_polys += 1,
            formats::VectorGeometry::MultiPolygon(v) => count_polys += v.len(),
        }
    }

    let world = world_from_vector_chunk(&chunk, None);

    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.uploaded_name = Some(name.clone());
        s.uploaded_world = Some(world);
        s.uploaded_count_points = count_points;
        s.uploaded_count_lines = count_lines;
        s.uploaded_count_polys = count_polys;

        // Switch to uploaded dataset immediately (matches previous behavior).
        s.dataset = "__uploaded__".to_string();
        s.uploaded_points_style.visible = s.uploaded_count_points > 0;
        s.uploaded_corridors_style.visible = s.uploaded_count_lines > 0;
        s.uploaded_regions_style.visible = s.uploaded_count_polys > 0;
    });

    rebuild_overlays_and_upload();
    let _ = render_scene();

    let summary = js_sys::Object::new();
    js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("name"),
        &JsValue::from_str(&name),
    )?;
    // Legacy permissive uploader counters (kept for UI compatibility).
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("skipped_coords"),
        &JsValue::from_f64(0.0),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("fixed_swapped"),
        &JsValue::from_f64(0.0),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("fixed_web_mercator"),
        &JsValue::from_f64(0.0),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("points"),
        &JsValue::from_f64(count_points as f64),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("lines"),
        &JsValue::from_f64(count_lines as f64),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("polygons"),
        &JsValue::from_f64(count_polys as f64),
    );

    Ok(summary.into())
}

#[wasm_bindgen]
pub fn load_dataset(url: String) {
    spawn_local(async move {
        let manifest = match fetch_manifest(&url).await {
            Ok(m) => m,
            Err(err) => {
                let msg = format!("Failed to fetch manifest: {:?}", err);
                web_sys::console::log_1(&JsValue::from_str(&msg));
                return;
            }
        };

        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.dataset = manifest
                .name
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            s.frame_index = 0;
            s.time_s = 0.0;
            s.time_end_s = (manifest.chunks.len().max(1) as f64) * 1.5;
        });

        let _ = render_scene();
    });
}

/// Advances the deterministic engine time by one fixed-timestep frame.
///
/// This is intentionally not wall-clock driven so it can be replayed.
#[wasm_bindgen]
pub fn advance_frame() -> Result<f64, JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.frame_index = s.frame_index.wrapping_add(1);
        s.time_s += s.dt_s;
        if s.time_s > s.time_end_s {
            s.time_s = 0.0;
            s.frame_index = 0;
        }
    });

    render_scene()?;
    Ok(STATE.with(|state| state.borrow().time_s))
}

#[wasm_bindgen]
pub fn set_time(time_s: f64) -> Result<(), JsValue> {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.time_s = time_s.max(0.0);
    });
    render_scene()
}

async fn init_wgpu_inner() -> Result<(), JsValue> {
    let ctx = init_wgpu_from_canvas_id("atlas-canvas-3d").await?;

    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let pending = s.pending_cities.take();
        let pending_corridors = s.pending_corridors.take();
        let pending_regions = s.pending_regions.take();
        s.wgpu = Some(ctx);

        if let Some(points) = pending
            && let Some(ctx) = &mut s.wgpu
        {
            set_cities_points(ctx, &points);
        }

        if let Some(points) = pending_corridors
            && let Some(ctx) = &mut s.wgpu
        {
            set_corridors_points(ctx, &points);
        }

        if let Some(points) = pending_regions
            && let Some(ctx) = &mut s.wgpu
        {
            set_regions_points(ctx, &points);
        }
    });

    render_scene()
}

async fn fetch_manifest(url: &str) -> Result<SceneManifest, JsValue> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))
}

async fn fetch_vector_chunk(url: &str) -> Result<formats::VectorChunk, JsValue> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    formats::VectorChunk::from_geojson_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))
}
