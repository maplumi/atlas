use console_error_panic_hook::set_once;
use gloo_net::http::Request;
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use formats::SceneManifest;
mod wgpu;
use wgpu::{WgpuContext, init_wgpu_from_canvas_id, render_mesh, resize_wgpu};

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
            let _ = render_mesh(ctx, view_proj);
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
    STATE.with(|state| {
        state.borrow_mut().dataset = dataset.to_string();
    });
    Ok(())
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
        s.wgpu = Some(ctx);
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
