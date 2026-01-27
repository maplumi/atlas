use console_error_panic_hook::set_once;
use gloo_net::http::Request;
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use formats::SceneManifest;
mod wgpu;
use wgpu::{
    CityVertex, OverlayVertex, WgpuContext, init_wgpu_from_canvas_id, render_mesh, resize_wgpu,
    set_cities_points, set_corridors_points, set_regions_points,
};

#[derive(Debug, serde::Deserialize)]
struct GeoJsonFeatureCollection {
    features: Vec<GeoJsonFeature>,
}

#[derive(Debug, serde::Deserialize)]
struct GeoJsonFeature {
    geometry: GeoJsonGeometry,
}

#[derive(Debug, serde::Deserialize)]
struct GeoJsonGeometry {
    #[serde(rename = "type")]
    ty: String,
    coordinates: serde_json::Value,
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
            distance: 3.0,
            target: [0.0, 0.0, 0.0],
        }
    }
}

#[derive(Debug)]
pub struct ViewerState {
    pub dataset: String,
    pub canvas_width: f64,
    pub canvas_height: f64,
    pub wgpu: Option<WgpuContext>,
    pub show_graticule: bool,
    pub sun_follow_real_time: bool,
    pub city_marker_size: f32,
    pub cities_centers: Option<Vec<[f32; 3]>>,
    pub pending_cities: Option<Vec<CityVertex>>,
    pub pending_corridors: Option<Vec<OverlayVertex>>,
    pub pending_regions: Option<Vec<OverlayVertex>>,
    pub frame_index: u64,
    pub dt_s: f64,
    pub time_s: f64,
    pub time_end_s: f64,
    pub camera: CameraState,
}

thread_local! {
    static STATE: RefCell<ViewerState> = RefCell::new(ViewerState {
        dataset: "demo-city".to_string(),
        canvas_width: 1280.0,
        canvas_height: 720.0,
        wgpu: None,
        show_graticule: false,
        sun_follow_real_time: true,
        city_marker_size: 0.02,
        cities_centers: None,
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

fn lon_lat_deg_to_world(lon_deg: f64, lat_deg: f64, lift_k: f32) -> [f32; 3] {
    let lon = (lon_deg as f32).to_radians();
    let lat = (lat_deg as f32).to_radians();

    // Coordinate system:
    // - Right-handed
    // - Origin at globe center
    // - +Y north
    // - +X lon=0°,lat=0° (equator / Greenwich)
    // - +Z lon=90°E,lat=0°
    let cos_lat = lat.cos();
    let sin_lat = lat.sin();
    let cos_lon = lon.cos();
    let sin_lon = lon.sin();
    let mut p = [cos_lat * cos_lon, sin_lat, cos_lat * sin_lon];

    p[0] *= lift_k;
    p[1] *= lift_k;
    p[2] *= lift_k;
    p
}

fn current_sun_direction_world() -> Option<[f32; 3]> {
    // Compute an approximate sun direction in the same coordinate system as the globe.
    // This is a standard low-cost solar position approximation (good enough for lighting).
    //
    // Returns a unit vector pointing from Earth center toward the Sun.

    fn deg_to_rad(d: f64) -> f64 {
        d.to_radians()
    }

    fn rad_to_deg(r: f64) -> f64 {
        r.to_degrees()
    }

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
    let t = n / 36525.0;

    // Mean longitude and anomaly (degrees)
    let l = wrap_360(280.46 + 0.9856474 * n);
    let g = wrap_360(357.528 + 0.9856003 * n);

    // Ecliptic longitude (degrees)
    let lambda = wrap_360(l + 1.915 * deg_to_rad(g).sin() + 0.020 * deg_to_rad(2.0 * g).sin());
    // Obliquity of the ecliptic (degrees)
    let eps = 23.439 - 0.0000004 * n;

    // Right ascension & declination
    let lambda_r = deg_to_rad(lambda);
    let eps_r = deg_to_rad(eps);
    let alpha_r = (eps_r.cos() * lambda_r.sin()).atan2(lambda_r.cos());
    let delta_r = (eps_r.sin() * lambda_r.sin()).asin();

    let alpha_deg = wrap_360(rad_to_deg(alpha_r));
    let delta_deg = rad_to_deg(delta_r);

    // Greenwich Mean Sidereal Time (degrees)
    let gmst = wrap_360(
        280.46061837 + 360.98564736629 * (jd - 2451545.0) + 0.000387933 * t * t
            - (t * t * t) / 38_710_000.0,
    );

    // Subsolar longitude: alpha - GMST (degrees east).
    let subsolar_lon = wrap_180(alpha_deg - gmst);
    let subsolar_lat = delta_deg;

    Some(lon_lat_deg_to_world(subsolar_lon, subsolar_lat, 1.0))
}

fn build_corridor_line_vertices(points: &[[f32; 3]]) -> Vec<OverlayVertex> {
    if points.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity((points.len().saturating_sub(1)) * 2);
    for seg in points.windows(2) {
        out.push(OverlayVertex {
            position: seg[0],
            _pad: 0.0,
        });
        out.push(OverlayVertex {
            position: seg[1],
            _pad: 0.0,
        });
    }
    out
}

fn slerp_unit(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    fn norm(a: [f32; 3]) -> f32 {
        dot(a, a).sqrt()
    }
    fn normalize(a: [f32; 3]) -> [f32; 3] {
        let n = norm(a);
        if n <= 0.0 {
            [0.0, 0.0, 0.0]
        } else {
            [a[0] / n, a[1] / n, a[2] / n]
        }
    }

    let a = normalize(a);
    let b = normalize(b);
    let mut d = dot(a, b);
    d = d.clamp(-1.0, 1.0);

    // If the points are almost identical, fall back to linear interpolation.
    if d > 0.9995 {
        let v = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ];
        return normalize(v);
    }

    let omega = d.acos();
    let sin_omega = omega.sin();
    let s0 = ((1.0 - t) * omega).sin() / sin_omega;
    let s1 = (t * omega).sin() / sin_omega;
    normalize([
        a[0] * s0 + b[0] * s1,
        a[1] * s0 + b[1] * s1,
        a[2] * s0 + b[2] * s1,
    ])
}

fn build_city_vertices(centers: &[[f32; 3]], size: f32) -> Vec<CityVertex> {
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn norm(a: [f32; 3]) -> f32 {
        dot(a, a).sqrt()
    }

    fn normalize(a: [f32; 3]) -> [f32; 3] {
        let n = norm(a);
        if n <= 0.0 {
            [0.0, 0.0, 0.0]
        } else {
            [a[0] / n, a[1] / n, a[2] / n]
        }
    }

    fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
        [a[0] * s, a[1] * s, a[2] * s]
    }

    // Two triangles (6 vertices) per city.
    let mut out = Vec::with_capacity(centers.len() * 6);
    for &p in centers {
        let n = normalize(p);
        let up = if n[1].abs() > 0.95 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let tangent = normalize(cross(up, n));
        let bitangent = normalize(cross(n, tangent));

        let t = mul(tangent, size);
        let b = mul(bitangent, size);

        // True quad corners in the tangent plane.
        let p0 = add(add(p, t), b);
        let p1 = add(sub(p, t), b);
        let p2 = sub(sub(p, t), b);
        let p3 = sub(add(p, t), b);

        // Two triangles: p0-p1-p2 and p0-p2-p3
        for v in [p0, p1, p2, p0, p2, p3] {
            out.push(CityVertex {
                position: v,
                _pad: 0.0,
            });
        }

        // The geometry is symmetric about p, so resizing should never move the center.
        // (In debug builds, assert the centroid equals p.)
        debug_assert!({
            let base = out.len() - 6;
            let mut c = [0.0f32; 3];
            for i in 0..6 {
                let q = out[base + i].position;
                c[0] += q[0];
                c[1] += q[1];
                c[2] += q[2];
            }
            c[0] /= 6.0;
            c[1] /= 6.0;
            c[2] /= 6.0;
            let eps = 1e-4;
            (c[0] - p[0]).abs() < eps && (c[1] - p[1]).abs() < eps && (c[2] - p[2]).abs() < eps
        });
    }
    out
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
    // This is the column-major form of:
    // [ m00,  0,   0,   0 ]
    // [  0,  m11,  0,   0 ]
    // [  0,   0,  m22, m23 ]
    // [  0,   0,  -1,   0 ]
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
    let proj = mat4_perspective_rh_z0(45f64.to_radians(), aspect, 0.05, 10_000.0);
    mat4_mul(proj, view)
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
            let show_cities = state.dataset == "cities";
            let show_corridors = state.dataset == "air_corridors";
            let show_regions = state.dataset == "regions";
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
        let zoom = (wheel_delta_y * 0.0015).exp();
        s.camera.distance = clamp(s.camera.distance * zoom, 0.25, 5000.0);
    });
    render_scene()
}

#[wasm_bindgen]
pub fn set_dataset(dataset: &str) -> Result<(), JsValue> {
    let dataset = dataset.to_string();
    let should_load_cities = dataset == "cities";
    let should_load_corridors = dataset == "air_corridors";
    let should_load_regions = dataset == "regions";

    STATE.with(|state| {
        state.borrow_mut().dataset = dataset;
    });

    if should_load_cities {
        spawn_local(async move {
            let centers = match fetch_cities_centers().await {
                Ok(c) => c,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch cities: {:?}",
                        err
                    )));
                    return;
                }
            };

            let points = STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.cities_centers = Some(centers);
                build_city_vertices(
                    s.cities_centers.as_deref().unwrap_or_default(),
                    s.city_marker_size,
                )
            });

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                if let Some(ctx) = &mut s.wgpu {
                    set_cities_points(ctx, &points);
                    s.pending_cities = None;
                } else {
                    s.pending_cities = Some(points);
                }
            });

            let _ = render_scene();
        });
    }

    if should_load_corridors {
        spawn_local(async move {
            let verts = match fetch_air_corridors_vertices().await {
                Ok(v) => v,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch air corridors: {:?}",
                        err
                    )));
                    return;
                }
            };

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                if let Some(ctx) = &mut s.wgpu {
                    set_corridors_points(ctx, &verts);
                    s.pending_corridors = None;
                } else {
                    s.pending_corridors = Some(verts);
                }
            });

            let _ = render_scene();
        });
    }

    if should_load_regions {
        spawn_local(async move {
            let verts = match fetch_regions_vertices().await {
                Ok(v) => v,
                Err(err) => {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "Failed to fetch regions: {:?}",
                        err
                    )));
                    return;
                }
            };

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                if let Some(ctx) = &mut s.wgpu {
                    set_regions_points(ctx, &verts);
                    s.pending_regions = None;
                } else {
                    s.pending_regions = Some(verts);
                }
            });

            let _ = render_scene();
        });
    }

    // Render immediately so selection changes are responsive.
    render_scene()?;
    Ok(())
}

#[wasm_bindgen]
pub fn set_city_marker_size(size: f64) -> Result<(), JsValue> {
    // Size is in world-space units on the unit sphere.
    let size = (size as f32).clamp(0.001, 0.25);

    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.city_marker_size = size;

        if let Some(centers) = s.cities_centers.as_deref() {
            let verts = build_city_vertices(centers, s.city_marker_size);
            if let Some(ctx) = &mut s.wgpu {
                set_cities_points(ctx, &verts);
                s.pending_cities = None;
            } else {
                s.pending_cities = Some(verts);
            }
        }
    });

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

async fn fetch_cities_centers() -> Result<Vec<[f32; 3]>, JsValue> {
    // Use a relative URL so it works under Trunk dev server and GitHub Pages.
    let resp = Request::get("assets/chunks/cities.json")
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let fc: GeoJsonFeatureCollection =
        serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut out = Vec::with_capacity(fc.features.len());
    for f in fc.features {
        if f.geometry.ty != "Point" {
            continue;
        }
        let coords = f
            .geometry
            .coordinates
            .as_array()
            .ok_or_else(|| JsValue::from_str("Invalid Point coordinates"))?;
        if coords.len() < 2 {
            continue;
        }
        let lon_deg = coords[0].as_f64().unwrap_or(0.0);
        let lat_deg = coords[1].as_f64().unwrap_or(0.0);

        // Lift slightly off the surface to reduce z-fighting.
        out.push(lon_lat_deg_to_world(lon_deg, lat_deg, 1.01));
    }

    Ok(out)
}

async fn fetch_air_corridors_vertices() -> Result<Vec<OverlayVertex>, JsValue> {
    let resp = Request::get("assets/chunks/air_corridors.json")
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let fc: GeoJsonFeatureCollection =
        serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut out: Vec<OverlayVertex> = Vec::new();

    for f in fc.features {
        if f.geometry.ty != "LineString" {
            continue;
        }
        let coords = f
            .geometry
            .coordinates
            .as_array()
            .ok_or_else(|| JsValue::from_str("Invalid LineString coordinates"))?;
        if coords.len() < 2 {
            continue;
        }

        // Convert to unit vectors first so we can draw a great-circle arc.
        let mut pts_unit: Vec<[f32; 3]> = Vec::with_capacity(coords.len());
        for c in coords {
            let arr = c
                .as_array()
                .ok_or_else(|| JsValue::from_str("Invalid coordinate pair"))?;
            if arr.len() < 2 {
                continue;
            }
            let lon_deg = arr[0].as_f64().unwrap_or(0.0);
            let lat_deg = arr[1].as_f64().unwrap_or(0.0);
            let p = lon_lat_deg_to_world(lon_deg, lat_deg, 1.0);
            pts_unit.push(p);
        }
        if pts_unit.len() < 2 {
            continue;
        }

        // Sample each segment as a great-circle to avoid chords through the globe.
        for seg in pts_unit.windows(2) {
            let a = seg[0];
            let b = seg[1];

            let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
            let omega = d.acos();
            let steps = ((omega / (5f32.to_radians())).ceil() as usize).clamp(4, 128);

            let mut arc: Vec<[f32; 3]> = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let u = slerp_unit(a, b, t);
                arc.push([u[0] * 1.012, u[1] * 1.012, u[2] * 1.012]);
            }

            out.extend(build_corridor_line_vertices(&arc));
        }
    }

    Ok(out)
}

async fn fetch_regions_vertices() -> Result<Vec<OverlayVertex>, JsValue> {
    let resp = Request::get("assets/chunks/regions.json")
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let fc: GeoJsonFeatureCollection =
        serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut out: Vec<OverlayVertex> = Vec::new();

    for f in fc.features {
        if f.geometry.ty != "Polygon" {
            continue;
        }
        let rings = f
            .geometry
            .coordinates
            .as_array()
            .ok_or_else(|| JsValue::from_str("Invalid Polygon coordinates"))?;
        if rings.is_empty() {
            continue;
        }

        let outer = rings[0]
            .as_array()
            .ok_or_else(|| JsValue::from_str("Invalid Polygon ring"))?;
        if outer.len() < 4 {
            continue;
        }

        // Drop the duplicated closing vertex if present.
        let mut verts_ll: Vec<(f64, f64)> = Vec::with_capacity(outer.len());
        for c in outer {
            let arr = c
                .as_array()
                .ok_or_else(|| JsValue::from_str("Invalid coordinate pair"))?;
            if arr.len() < 2 {
                continue;
            }
            let lon_deg = arr[0].as_f64().unwrap_or(0.0);
            let lat_deg = arr[1].as_f64().unwrap_or(0.0);
            verts_ll.push((lon_deg, lat_deg));
        }
        if verts_ll.len() >= 2 {
            let first = verts_ll[0];
            let last = *verts_ll.last().unwrap();
            if (first.0 - last.0).abs() < 1e-9 && (first.1 - last.1).abs() < 1e-9 {
                verts_ll.pop();
            }
        }
        if verts_ll.len() < 3 {
            continue;
        }

        // Simple fan triangulation (works for the demo rectangles).
        let (lon0, lat0) = verts_ll[0];
        let p0 = lon_lat_deg_to_world(lon0, lat0, 1.006);
        for i in 1..(verts_ll.len() - 1) {
            let (lona, lata) = verts_ll[i];
            let (lonb, latb) = verts_ll[i + 1];
            let pa = lon_lat_deg_to_world(lona, lata, 1.006);
            let pb = lon_lat_deg_to_world(lonb, latb, 1.006);

            for p in [p0, pa, pb] {
                out.push(OverlayVertex {
                    position: p,
                    _pad: 0.0,
                });
            }
        }
    }

    Ok(out)
}
