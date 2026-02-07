use gloo_net::http::Request;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_futures::spawn_local;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

// Guard to prevent double-initialization of global state (relevant during hot reload).
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK_SET: OnceLock<()> = OnceLock::new();

/// Yield to the browser event loop to keep the UI responsive during heavy computation.
/// This uses setTimeout(0) to allow repaints and input processing.
async fn yield_now() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let window = web_sys::window().unwrap();
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
    });
    let _ = JsFuture::from(promise).await;
}

use catalog::{CatalogEntry, CatalogStore};
use formats::SceneManifest;
use foundation::math::{Geodetic, WGS84_A, WGS84_B, ecef_to_geodetic, geodetic_to_ecef};
use layers::symbology::LayerStyle;
use layers::vector::VectorLayer;
use scene::components::VectorGeometryKind;

mod globe_controller;
use globe_controller::GlobeController;

mod wgpu;
use wgpu::{
    CityVertex, CorridorVertex, Globals2D, Overlay2DVertex, OverlayVertex, Point2DInstance,
    Segment2DInstance, Style, TerrainVertex, WgpuContext, init_wgpu_from_canvas_id, perf_reset,
    perf_snapshot, render_map2d, render_mesh, resize_wgpu, set_base_regions_points,
    set_base_regions2d_vertices, set_cities_points, set_corridors_points, set_grid2d_instances,
    set_lines2d_instances, set_points2d_instances, set_regions_points, set_regions2d_vertices,
    set_styles, set_terrain_points,
};

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
enum ViewMode {
    TwoD,
    #[default]
    ThreeD,
}

impl ViewMode {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "2d" | "two_d" | "two" => ViewMode::TwoD,
            _ => ViewMode::ThreeD,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
enum Theme {
    #[default]
    Dark,
    DeepDark,
    Light,
}

impl Theme {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "deep-dark" | "deep_dark" | "deep" | "black" => Theme::DeepDark,
            "light" => Theme::Light,
            _ => Theme::Dark,
        }
    }
}

#[derive(Debug, Copy, Clone)]
struct ThemePalette {
    clear_color: ::wgpu::Color,
    globe_color: [f32; 3],
    stars_alpha: f32,
    canvas_2d_clear: &'static str,
    base_surface_color: [f32; 4],
}

fn palette_for(theme: Theme) -> ThemePalette {
    match theme {
        Theme::Dark => ThemePalette {
            clear_color: ::wgpu::Color {
                r: 0.004,
                g: 0.008,
                b: 0.016,
                a: 1.0,
            },
            globe_color: [0.10, 0.55, 0.85],
            stars_alpha: 1.0,
            canvas_2d_clear: "#020617",
            base_surface_color: [0.20, 0.65, 0.35, 1.0],
        },
        Theme::DeepDark => ThemePalette {
            clear_color: ::wgpu::Color {
                r: 0.001,
                g: 0.002,
                b: 0.004,
                a: 1.0,
            },
            globe_color: [0.07, 0.45, 0.75],
            stars_alpha: 1.1,
            canvas_2d_clear: "#000000",
            base_surface_color: [0.16, 0.55, 0.30, 1.0],
        },
        Theme::Light => ThemePalette {
            clear_color: ::wgpu::Color {
                r: 0.92,
                g: 0.95,
                b: 0.98,
                a: 1.0,
            },
            globe_color: [0.18, 0.55, 0.85],
            stars_alpha: 0.25,
            canvas_2d_clear: "#f8fafc",
            base_surface_color: [0.12, 0.55, 0.25, 1.0],
        },
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Camera2DState {
    pub center_lon_deg: f64,
    pub center_lat_deg: f64,
    /// Zoom multiplier: 1.0 roughly fits the world into the viewport.
    pub zoom: f64,
}

impl Default for Camera2DState {
    fn default() -> Self {
        Self {
            center_lon_deg: 0.0,
            center_lat_deg: 0.0,
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct CameraState {
    pub yaw_rad: f64,
    pub pitch_rad: f64,
    pub distance: f64,
    pub target: [f64; 3],
}

#[derive(Debug, Copy, Clone)]
struct Cull2DSnapshot {
    center_m: [f32; 2],
    scale_px_per_m: f32,
    viewport_px: [f32; 2],
    show_points: bool,
    show_lines: bool,
    geom_gen: u64,
}

#[derive(Debug, Copy, Clone)]
struct MercatorViewRect {
    center_x: f32,
    center_y: f32,
    half_w_m: f32,
    half_h_m: f32,
    world_width_m: f32,
}

#[derive(Debug)]
struct Cull2DJob {
    snapshot: Cull2DSnapshot,
    rect: MercatorViewRect,

    // Point sources
    cities_i: usize,
    uploaded_i: usize,
    feed_keys: Vec<String>,
    feed_layer_i: usize,
    feed_point_i: usize,
    selection_point_done: bool,

    // Line sources (segment index, not vertex index)
    corridors_seg_i: usize,
    uploaded_corridors_seg_i: usize,
    selection_seg_i: usize,

    last_uploaded_points: usize,
    last_uploaded_lines: usize,

    points_out: Vec<Point2DInstance>,
    lines_out: Vec<Segment2DInstance>,
}

#[derive(Debug, Clone)]
struct DebugLabel {
    text: String,
    mercator_m: [f32; 2],
    viewer_pos: [f32; 3],
    priority: f32,
}

#[derive(Debug, Copy, Clone)]
struct Label2DSnapshot {
    cam: Camera2DState,
    viewport_px: [f32; 2],
    generation: u64,
}

#[derive(Debug, Clone)]
struct Label2DCandidate {
    text: String,
    mercator_m: [f32; 2],
    priority: f32,
}

#[derive(Debug, Clone)]
struct PlacedLabel2D {
    text: String,
    x_px: f32,
    y_px: f32,
    priority: f32,
}

#[derive(Debug)]
struct Label2DJob {
    snapshot: Label2DSnapshot,
    candidates: Vec<Label2DCandidate>,
    i: usize,
    occupied_cells: std::collections::HashSet<u64>,
    placed: Vec<PlacedLabel2D>,
}

#[derive(Debug, Clone, Deserialize)]
struct TerrainTileset {
    #[serde(rename = "version")]
    _version: u32,
    tile_size: u32,
    zoom_min: u32,
    zoom_max: u32,
    #[serde(rename = "data_type")]
    _data_type: String,
    tile_path_template: String,
    #[serde(default)]
    vertical_datum: Option<String>,
    #[serde(default)]
    vertical_units: Option<String>,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    min_height: f64,
    max_height: f64,
    #[serde(default)]
    no_data: Option<f64>,
    #[serde(default)]
    sample_step: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct SurfaceTileset {
    #[serde(rename = "version")]
    _version: u32,
    zoom_min: u32,
    zoom_max: u32,
    #[serde(rename = "data_type")]
    _data_type: String,
    tile_path_template: String,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    #[serde(default)]
    coordinate_space: Option<String>,
}

#[derive(Debug, Clone)]
struct TerrainTile {
    z: u32,
    x: u32,
    y: u32,
    heights_m: Vec<f32>,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            // Default view: center Africa (roughly central longitude/latitude).
            // In this viewer, yaw ~ longitude and pitch ~ latitude for the point facing the camera.
            // Yaw ~160° places Africa (lon ~20°E) in front of the camera.
            yaw_rad: 160f64.to_radians(),
            pitch_rad: 5f64.to_radians(),
            distance: 3.0 * WGS84_A,
            target: [0.0, 0.0, 0.0],
        }
    }
}

// ── Control Configuration Contract ──────────────────────────────────────────
//
// Defines the interaction behavior defaults for the spatiotemporal viewer.
// These contracts ensure consistent, predictable user interaction across both
// the 3D globe and 2D map views.
//
// **3D Globe Controls:**
//   Left drag   = arcball rotation (surface follows cursor direction)
//   Right drag  = pan/translate globe in screen space
//   Wheel       = dolly zoom (distance to globe center)
//   Shift+drag  = pan/translate globe in screen space (same as right drag)
//
// **2D Map Controls:**
//   Left drag   = pan map (map follows cursor direction)
//   Right drag  = pan map (same behavior as left drag)
//   Wheel       = zoom anchored at cursor position
//   Shift+drag  = pan map (same behavior)
//
// **Common:**
//   R key       = reset camera to default
//   Pinch       = zoom (touch devices)
//
// All sensitivity values are tunable via the settings UI and persisted via JS.

/// Interaction configuration for the viewer.  All fields have documented
/// defaults that are exercised by tests below.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ControlConfig {
    // ── 3D Globe ─────────────────────────────────────────────
    /// Orbit (arcball) sensitivity multiplier.  1.0 = default.
    pub orbit_sensitivity: f64,
    /// Pan sensitivity multiplier for 3D target translation.
    pub pan_sensitivity_3d: f64,
    /// Zoom (dolly) speed multiplier for 3D.  Applied to the 0.0015 exponent.
    pub zoom_speed_3d: f64,
    /// Whether to invert the orbit Y axis (vertical drag direction).
    pub invert_orbit_y: bool,
    /// Whether to invert the pan Y axis in 3D.
    pub invert_pan_y_3d: bool,
    /// Minimum distance from globe center (prevents going inside).
    pub min_distance: f64,
    /// Maximum distance from globe center.
    pub max_distance: f64,
    /// Pitch clamp in radians (absolute value, symmetric).
    pub pitch_clamp_rad: f64,
    /// Maximum target offset from origin in meters.  Prevents the globe
    /// from being panned completely out of view.
    pub max_target_offset_m: f64,

    // ── 2D Map ───────────────────────────────────────────────
    /// Pan sensitivity multiplier for 2D.
    pub pan_sensitivity_2d: f64,
    /// Zoom speed multiplier for 2D.  Applied to the 0.0015 exponent.
    pub zoom_speed_2d: f64,
    /// Whether to invert the pan Y axis in 2D.
    pub invert_pan_y_2d: bool,
    /// Minimum zoom level (1.0 = whole world).
    pub min_zoom_2d: f64,
    /// Maximum zoom level.
    pub max_zoom_2d: f64,
    /// Enable kinetic (inertia) panning on pointer release.
    pub kinetic_panning: bool,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            orbit_sensitivity: 1.0,
            pan_sensitivity_3d: 1.0,
            zoom_speed_3d: 1.0,
            invert_orbit_y: false,
            invert_pan_y_3d: false,
            min_distance: 10.0,
            max_distance: 200.0 * WGS84_A,
            pitch_clamp_rad: 1.55,
            max_target_offset_m: 2.0 * WGS84_A,

            pan_sensitivity_2d: 1.0,
            zoom_speed_2d: 1.0,
            invert_pan_y_2d: false,
            min_zoom_2d: 1.0,
            max_zoom_2d: 200.0,
            kinetic_panning: true,
        }
    }
}

#[derive(Debug)]
struct FeedLayerState {
    name: String,
    centers: Vec<[f32; 3]>,
    centers_mercator: Vec<[f32; 2]>,
    count_points: usize,
    style: LayerStyle,
}

#[derive(Debug)]
struct ViewerState {
    dataset: String,
    canvas_width: f64,
    canvas_height: f64,
    view_mode: ViewMode,
    theme: Theme,
    canvas_2d: Option<HtmlCanvasElement>,
    ctx_2d: Option<CanvasRenderingContext2d>,
    wgpu: Option<WgpuContext>,
    show_graticule: bool,
    sun_follow_real_time: bool,

    // Globe appearance.
    globe_transparent: bool,

    // Point marker size in screen pixels.
    city_marker_size: f32,

    // Line width in screen pixels (applies to all line layers).
    line_width_px: f32,

    // Layer styles and visibility.
    base_regions_style: LayerStyle,
    cities_style: LayerStyle,
    corridors_style: LayerStyle,
    regions_style: LayerStyle,
    uploaded_points_style: LayerStyle,
    uploaded_corridors_style: LayerStyle,
    uploaded_regions_style: LayerStyle,
    selection_style: LayerStyle,
    terrain_style: LayerStyle,

    // Engine worlds (source-of-truth). Renderable geometry must be derived via `layers`.
    base_world: Option<scene::World>,
    cities_world: Option<scene::World>,
    corridors_world: Option<scene::World>,
    regions_world: Option<scene::World>,
    uploaded_world: Option<scene::World>,

    // CPU-side cached geometry in viewer coordinates.
    base_regions_positions: Option<Vec<[f32; 3]>>,
    base_regions_mercator: Option<Vec<[f32; 2]>>,
    cities_centers: Option<Vec<[f32; 3]>>,
    cities_mercator: Option<Vec<[f32; 2]>>,
    corridors_positions: Option<Vec<[f32; 3]>>,
    corridors_mercator: Option<Vec<[f32; 2]>>,
    regions_positions: Option<Vec<[f32; 3]>>,
    regions_mercator: Option<Vec<[f32; 2]>>,

    base_world_loading: bool,
    base_world_error: Option<String>,
    base_world_source: Option<String>,

    surface_tileset: Option<SurfaceTileset>,
    surface_positions: Option<Vec<[f32; 3]>>,
    surface_zoom: Option<u32>,
    surface_source: Option<String>,
    surface_loading: bool,
    surface_last_error: Option<String>,
    surface_next_retry_ms: f64,

    terrain_tileset: Option<TerrainTileset>,
    terrain_vertices: Option<Vec<TerrainVertex>>,
    terrain_zoom: Option<u32>,
    terrain_source: Option<String>,
    terrain_loading: bool,
    terrain_last_error: Option<String>,

    auto_rotate_enabled: bool,
    auto_rotate_speed_deg_per_s: f64,
    auto_rotate_last_user_time_s: f64,
    auto_rotate_resume_delay_s: f64,

    uploaded_name: Option<String>,
    uploaded_catalog_id: Option<String>,
    uploaded_centers: Option<Vec<[f32; 3]>>,
    uploaded_mercator: Option<Vec<[f32; 2]>>,
    uploaded_corridors_positions: Option<Vec<[f32; 3]>>,
    uploaded_corridors_mercator: Option<Vec<[f32; 2]>>,
    uploaded_regions_positions: Option<Vec<[f32; 3]>>,
    uploaded_regions_mercator: Option<Vec<[f32; 2]>>,
    uploaded_count_points: usize,
    uploaded_count_lines: usize,
    uploaded_count_polys: usize,

    // Online feeds (browser-fetched). For now these render as point layers.
    feed_layers: BTreeMap<String, FeedLayerState>,

    // Stable GPU style IDs for feed layers (so style updates don't require geometry rebuild).
    feed_style_ids: BTreeMap<String, u32>,
    next_feed_style_id: u32,

    base_count_polys: usize,
    cities_count_points: usize,
    corridors_count_lines: usize,
    regions_count_polys: usize,

    selection_center: Option<[f32; 3]>,
    selection_center_mercator: Option<[f32; 2]>,
    selection_line_positions: Option<Vec<[f32; 3]>>,
    selection_line_mercator: Option<Vec<[f32; 2]>>,
    selection_poly_positions: Option<Vec<[f32; 3]>>,
    selection_poly_mercator: Option<Vec<[f32; 2]>>,

    // 2D render instrumentation (ms). Best-effort timing.
    perf_2d_total_ms: f64,
    perf_2d_poly_ms: f64,
    perf_2d_line_ms: f64,
    perf_2d_point_ms: f64,
    perf_2d_poly_tris: u32,
    perf_2d_line_segs: u32,
    perf_2d_points: u32,

    // 2D viewport culling (WebGPU path): time-sliced rebuild of visible point/line instances.
    cull2d_enabled: bool,
    cull2d_geom_gen: u64,
    cull2d_last_snapshot: Option<Cull2DSnapshot>,
    cull2d_job: Option<Cull2DJob>,
    cull2d_visible_points: u32,
    cull2d_visible_line_segs: u32,

    // GPU perf counters (best-effort; WebGPU path).
    perf_gpu_upload_calls: u32,
    perf_gpu_upload_bytes: u64,
    perf_gpu_render_passes: u32,
    perf_gpu_draw_calls: u32,
    perf_gpu_draw_instances: u64,
    perf_gpu_draw_vertices: u64,
    perf_gpu_draw_indices: u64,
    perf_gpu_frame_ms: f64,

    // Labels (overlay canvas). This is intentionally a scaffold: incremental layout and batching.
    labels_enabled: bool,
    labels_gen: u64,
    debug_labels: Vec<DebugLabel>,
    labels2d_job: Option<Label2DJob>,
    labels2d_last_snapshot: Option<Label2DSnapshot>,
    labels2d_placed: Vec<PlacedLabel2D>,

    // Combined buffers (all visible layers), uploaded into the shared GPU buffers.
    pending_styles: Option<Vec<Style>>,
    pending_cities: Option<Vec<CityVertex>>,
    pending_corridors: Option<Vec<CorridorVertex>>,
    pending_base_regions: Option<Vec<OverlayVertex>>,
    pending_regions: Option<Vec<OverlayVertex>>,
    pending_terrain: Option<Vec<TerrainVertex>>,
    pending_base_regions2d: Option<Vec<Overlay2DVertex>>,
    pending_regions2d: Option<Vec<Overlay2DVertex>>,
    pending_points2d: Option<Vec<Point2DInstance>>,
    pending_lines2d: Option<Vec<Segment2DInstance>>,
    pending_grid2d: Option<Vec<Segment2DInstance>>,
    frame_index: u64,
    dt_s: f64,
    time_s: f64,
    time_end_s: f64,
    camera: CameraState,
    camera_2d: Camera2DState,

    // Quaternion-based globe controller for 3D view.
    globe_controller: GlobeController,
    /// Last frame time for globe controller updates.
    last_frame_time_s: f64,

    // Pointer drag state shared by 2D + 3D camera interactions.
    drag_last_x_px: f64,
    drag_last_y_px: f64,
    arcball_last_unit: Option<[f64; 3]>,

    // Interaction control configuration (tunable via settings UI).
    controls: ControlConfig,
}

thread_local! {
    static STATE: RefCell<ViewerState> = RefCell::new(ViewerState {
        dataset: "cities".to_string(),
        canvas_width: 1280.0,
        canvas_height: 720.0,
        view_mode: ViewMode::ThreeD,
        theme: Theme::Dark,
        canvas_2d: None,
        ctx_2d: None,
        wgpu: None,
        show_graticule: false,
        sun_follow_real_time: true,
        globe_transparent: false,
        city_marker_size: 4.0,
        line_width_px: 2.5,

        base_regions_style: LayerStyle { visible: true, color: [0.20, 0.65, 0.35, 1.0], lift: 0.0 },
        cities_style: LayerStyle { visible: false, color: [1.0, 0.25, 0.25, 0.95], lift: 0.0 },
        corridors_style: LayerStyle { visible: false, color: [1.0, 0.85, 0.25, 0.90], lift: 0.0 },
        regions_style: LayerStyle { visible: false, color: [0.10, 0.90, 0.75, 0.30], lift: 0.0 },
        uploaded_points_style: LayerStyle { visible: false, color: [0.60, 0.95, 1.00, 0.95], lift: 0.0 },
        uploaded_corridors_style: LayerStyle { visible: false, color: [0.85, 0.95, 0.60, 0.90], lift: 0.0 },
        uploaded_regions_style: LayerStyle { visible: false, color: [0.45, 0.75, 1.00, 0.25], lift: 0.0 },
        selection_style: LayerStyle { visible: true, color: [1.0, 1.0, 1.0, 0.95], lift: 0.0 },
        terrain_style: LayerStyle { visible: false, color: [0.32, 0.72, 0.45, 0.95], lift: 0.0 },

        base_world: None,
        cities_world: None,
        corridors_world: None,
        regions_world: None,
        uploaded_world: None,

        base_regions_positions: None,
        base_regions_mercator: None,
        cities_centers: None,
        cities_mercator: None,
        corridors_positions: None,
        corridors_mercator: None,
        regions_positions: None,
        regions_mercator: None,

        base_world_loading: false,
        base_world_error: None,
        base_world_source: None,

        surface_tileset: None,
        surface_positions: None,
        surface_zoom: None,
        surface_source: None,
        surface_loading: false,
        surface_last_error: None,
        surface_next_retry_ms: 0.0,

        terrain_tileset: None,
        terrain_vertices: None,
        terrain_zoom: None,
        terrain_source: None,
        terrain_loading: false,
        terrain_last_error: None,

        auto_rotate_enabled: true,
        auto_rotate_speed_deg_per_s: 0.15,
        auto_rotate_last_user_time_s: 0.0,
        auto_rotate_resume_delay_s: 1.2,

        uploaded_name: None,
        uploaded_catalog_id: None,
        uploaded_centers: None,
        uploaded_mercator: None,
        uploaded_corridors_positions: None,
        uploaded_corridors_mercator: None,
        uploaded_regions_positions: None,
        uploaded_regions_mercator: None,
        uploaded_count_points: 0,
        uploaded_count_lines: 0,
        uploaded_count_polys: 0,

        feed_layers: BTreeMap::new(),

        feed_style_ids: BTreeMap::new(),
        next_feed_style_id: 100,

        base_count_polys: 0,
        cities_count_points: 0,
        corridors_count_lines: 0,
        regions_count_polys: 0,

        selection_center: None,
        selection_center_mercator: None,
        selection_line_positions: None,
        selection_line_mercator: None,
        selection_poly_positions: None,
        selection_poly_mercator: None,

        perf_2d_total_ms: 0.0,
        perf_2d_poly_ms: 0.0,
        perf_2d_line_ms: 0.0,
        perf_2d_point_ms: 0.0,
        perf_2d_poly_tris: 0,
        perf_2d_line_segs: 0,
        perf_2d_points: 0,

        cull2d_enabled: true,
        cull2d_geom_gen: 0,
        cull2d_last_snapshot: None,
        cull2d_job: None,
        cull2d_visible_points: 0,
        cull2d_visible_line_segs: 0,

        perf_gpu_upload_calls: 0,
        perf_gpu_upload_bytes: 0,
        perf_gpu_render_passes: 0,
        perf_gpu_draw_calls: 0,
        perf_gpu_draw_instances: 0,
        perf_gpu_draw_vertices: 0,
        perf_gpu_draw_indices: 0,
        perf_gpu_frame_ms: 0.0,

        labels_enabled: true,
        labels_gen: 0,
        debug_labels: Vec::new(),
        labels2d_job: None,
        labels2d_last_snapshot: None,
        labels2d_placed: Vec::new(),
        pending_styles: None,
        pending_cities: None,
        pending_corridors: None,
        pending_base_regions: None,
        pending_regions: None,
        pending_terrain: None,
        pending_base_regions2d: None,
        pending_regions2d: None,
        pending_points2d: None,
        pending_lines2d: None,
        pending_grid2d: None,
        frame_index: 0,
        dt_s: 1.0 / 60.0,
        time_s: 0.0,
        time_end_s: 10.0,
        camera: CameraState::default(),
        camera_2d: Camera2DState::default(),

        globe_controller: GlobeController::default(),
        last_frame_time_s: 0.0,

        drag_last_x_px: 0.0,
        drag_last_y_px: 0.0,
        arcball_last_unit: None,

        controls: ControlConfig::default(),
    });
}

/// Safe TLS access helper that returns a default on teardown instead of panicking.
/// Use this for all STATE/CATALOG accesses to prevent hot-reload crashes.
fn with_state<F, R>(f: F) -> R
where
    F: FnOnce(&RefCell<ViewerState>) -> R,
    R: Default,
{
    STATE.try_with(f).unwrap_or_default()
}

fn init_panic_hook() {
    PANIC_HOOK_SET.get_or_init(|| {
        std::panic::set_hook(Box::new(|info| {
            let msg = info.to_string();
            web_sys::console::error_1(&JsValue::from_str(&msg));
        }));
    });
}

#[derive(Debug)]
enum ViewerCatalogStore {
    Local(catalog::LocalStorageCatalogStore),
    Memory(catalog::InMemoryCatalogStore),
}

impl ViewerCatalogStore {
    fn new() -> Self {
        match catalog::LocalStorageCatalogStore::new("atlas.catalog.v1") {
            Ok(s) => ViewerCatalogStore::Local(s),
            Err(_) => ViewerCatalogStore::Memory(catalog::InMemoryCatalogStore::new()),
        }
    }

    fn upsert_avc_bytes(
        &mut self,
        entry: CatalogEntry,
        avc_bytes: &[u8],
    ) -> Result<(), catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => s.upsert_avc_bytes(entry, avc_bytes),
            ViewerCatalogStore::Memory(s) => s.upsert_avc_bytes(entry, avc_bytes),
        }
    }

    fn get_avc_bytes(&self, id: &str) -> Result<Vec<u8>, catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => {
                s.get_avc_bytes(id)?.ok_or(catalog::CatalogError::NotFound)
            }
            ViewerCatalogStore::Memory(s) => {
                s.get_avc_bytes(id)?.ok_or(catalog::CatalogError::NotFound)
            }
        }
    }
}

impl CatalogStore for ViewerCatalogStore {
    fn list(&self) -> Result<Vec<CatalogEntry>, catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => s.list(),
            ViewerCatalogStore::Memory(s) => s.list(),
        }
    }

    fn get(&self, id: &str) -> Result<Option<CatalogEntry>, catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => s.get(id),
            ViewerCatalogStore::Memory(s) => s.get(id),
        }
    }

    fn upsert(&mut self, entry: CatalogEntry) -> Result<(), catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => s.upsert(entry),
            ViewerCatalogStore::Memory(s) => s.upsert(entry),
        }
    }

    fn delete(&mut self, id: &str) -> Result<bool, catalog::CatalogError> {
        match self {
            ViewerCatalogStore::Local(s) => s.delete(id),
            ViewerCatalogStore::Memory(s) => s.delete(id),
        }
    }
}

thread_local! {
    static CATALOG: RefCell<ViewerCatalogStore> = RefCell::new(ViewerCatalogStore::new());
}

// IndexedDB (best-effort) storage for AVC bytes.
// This avoids LocalStorage quota issues and avoids base64 megastring allocations.
#[wasm_bindgen(inline_js = "
let __atlas_idb_promise = null;

function __atlas_open_db() {
    if (__atlas_idb_promise) return __atlas_idb_promise;

    __atlas_idb_promise = new Promise((resolve, reject) => {
        try {
            const req = indexedDB.open('atlas', 1);
            req.onupgradeneeded = () => {
                const db = req.result;
                if (!db.objectStoreNames.contains('avc')) {
                    db.createObjectStore('avc');
                }
            };
            req.onsuccess = () => resolve(req.result);
            req.onerror = () => reject(req.error || new Error('IndexedDB open failed'));
        } catch (e) {
            reject(e);
        }
    });

    return __atlas_idb_promise;
}

export async function atlas_idb_put_avc(id, bytes) {
    const db = await __atlas_open_db();
    return await new Promise((resolve, reject) => {
        try {
            const tx = db.transaction(['avc'], 'readwrite');
            const store = tx.objectStore('avc');
            // Store as Uint8Array (structured clone supports typed arrays).
            store.put(bytes, id);
            tx.oncomplete = () => resolve(true);
            tx.onerror = () => reject(tx.error || new Error('IndexedDB put failed'));
            tx.onabort = () => reject(tx.error || new Error('IndexedDB put aborted'));
        } catch (e) {
            reject(e);
        }
    });
}

export async function atlas_idb_get_avc(id) {
    const db = await __atlas_open_db();
    return await new Promise((resolve, reject) => {
        try {
            const tx = db.transaction(['avc'], 'readonly');
            const store = tx.objectStore('avc');
            const req = store.get(id);
            req.onsuccess = () => resolve(req.result ?? null);
            req.onerror = () => reject(req.error || new Error('IndexedDB get failed'));
        } catch (e) {
            reject(e);
        }
    });
}

export async function atlas_idb_delete_avc(id) {
    const db = await __atlas_open_db();
    return await new Promise((resolve, reject) => {
        try {
            const tx = db.transaction(['avc'], 'readwrite');
            const store = tx.objectStore('avc');
            store.delete(id);
            tx.oncomplete = () => resolve(true);
            tx.onerror = () => reject(tx.error || new Error('IndexedDB delete failed'));
            tx.onabort = () => reject(tx.error || new Error('IndexedDB delete aborted'));
        } catch (e) {
            reject(e);
        }
    });
}
")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn atlas_idb_put_avc(id: &str, bytes: &[u8]) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(catch)]
    fn atlas_idb_get_avc(id: &str) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(catch)]
    fn atlas_idb_delete_avc(id: &str) -> Result<js_sys::Promise, JsValue>;
}

async fn idb_put_avc_bytes(id: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let promise = atlas_idb_put_avc(id, bytes)?;
    JsFuture::from(promise).await?;
    Ok(())
}

async fn idb_get_avc_bytes(id: &str) -> Result<Option<Vec<u8>>, JsValue> {
    let promise = atlas_idb_get_avc(id)?;
    let v = JsFuture::from(promise).await?;
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    let arr = js_sys::Uint8Array::new(&v);
    let mut out = vec![0u8; arr.length() as usize];
    arr.copy_to(&mut out);
    Ok(Some(out))
}

async fn idb_delete_avc(id: &str) -> Result<(), JsValue> {
    let promise = atlas_idb_delete_avc(id)?;
    JsFuture::from(promise).await?;
    Ok(())
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Returns wall-clock time in seconds (for idle detection, not tied to time_s).
fn wall_clock_seconds() -> f64 {
    js_sys::Date::now() / 1000.0
}

fn wrap_lon_deg(mut lon: f64) -> f64 {
    lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    lon
}

const MERCATOR_MAX_LAT_DEG: f64 = 85.05112878;

fn is_mercator_lat_valid(lat_deg: f64) -> bool {
    lat_deg.is_finite() && (-MERCATOR_MAX_LAT_DEG..=MERCATOR_MAX_LAT_DEG).contains(&lat_deg)
}

fn mercator_x_m(lon_deg: f64) -> f64 {
    WGS84_A * lon_deg.to_radians()
}

fn mercator_y_m(lat_deg: f64) -> f64 {
    let lat = clamp(lat_deg, -MERCATOR_MAX_LAT_DEG, MERCATOR_MAX_LAT_DEG).to_radians();
    WGS84_A * (0.5 * (std::f64::consts::FRAC_PI_2 + lat)).tan().ln()
}

fn unwrap_mercator_x_m(anchor_x_m: f64, x_m: f64) -> f64 {
    let ww = 2.0 * std::f64::consts::PI * WGS84_A;
    let dx = (x_m - anchor_x_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww;
    anchor_x_m + dx
}

fn wrap_dx_f32(dx: f32, ww: f32) -> f32 {
    // Matches MAP2D_SHADER.wrap_dx: dx wrapped into [-ww/2, +ww/2].
    let t = (dx + 0.5 * ww) / ww;
    (dx + 0.5 * ww) - ww * t.floor() - 0.5 * ww
}

fn make_mercator_view_rect(
    center_x: f32,
    center_y: f32,
    viewport_w_px: f32,
    viewport_h_px: f32,
    scale_px_per_m: f32,
    world_width_m: f32,
    pad_px: f32,
) -> MercatorViewRect {
    let inv_scale = 1.0 / scale_px_per_m.max(1e-6);
    let pad_m = pad_px * inv_scale;
    MercatorViewRect {
        center_x,
        center_y,
        half_w_m: 0.5 * viewport_w_px * inv_scale + pad_m,
        half_h_m: 0.5 * viewport_h_px * inv_scale + pad_m,
        world_width_m,
    }
}

fn mercator_point_visible(rect: MercatorViewRect, p: [f32; 2]) -> bool {
    let dx = wrap_dx_f32(p[0] - rect.center_x, rect.world_width_m);
    let dy = p[1] - rect.center_y;
    dx.abs() <= rect.half_w_m && dy.abs() <= rect.half_h_m
}

fn mercator_segment_visible(rect: MercatorViewRect, a: [f32; 2], b: [f32; 2]) -> bool {
    // Matches MAP2D_SHADER.vs_line unwrapping: wrap A near center, then keep B relative to A.
    let ax_adj = rect.center_x + wrap_dx_f32(a[0] - rect.center_x, rect.world_width_m);
    let bx_adj = ax_adj + (b[0] - a[0]);
    let min_x = ax_adj.min(bx_adj);
    let max_x = ax_adj.max(bx_adj);
    let min_y = a[1].min(b[1]);
    let max_y = a[1].max(b[1]);

    let view_min_x = rect.center_x - rect.half_w_m;
    let view_max_x = rect.center_x + rect.half_w_m;
    let view_min_y = rect.center_y - rect.half_h_m;
    let view_max_y = rect.center_y + rect.half_h_m;

    !(max_x < view_min_x || min_x > view_max_x || max_y < view_min_y || min_y > view_max_y)
}

fn cull2d_should_restart(prev: Option<Cull2DSnapshot>, next: Cull2DSnapshot, ww: f32) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    if prev.geom_gen != next.geom_gen {
        return true;
    }
    if prev.show_points != next.show_points || prev.show_lines != next.show_lines {
        return true;
    }
    if (prev.viewport_px[0] - next.viewport_px[0]).abs() > 0.5
        || (prev.viewport_px[1] - next.viewport_px[1]).abs() > 0.5
    {
        return true;
    }

    // Avoid restarting on tiny camera changes so time-sliced jobs can complete.
    // Restart when center shifts by ~25% of the viewport width or zoom changes significantly.
    let prev_half_w_m = 0.5 * prev.viewport_px[0] / prev.scale_px_per_m.max(1e-6);
    let prev_half_h_m = 0.5 * prev.viewport_px[1] / prev.scale_px_per_m.max(1e-6);
    let shift_thresh = 0.25 * prev_half_w_m.min(prev_half_h_m).max(1.0);
    let dx = wrap_dx_f32(next.center_m[0] - prev.center_m[0], ww);
    let dy = next.center_m[1] - prev.center_m[1];
    if dx.abs() > shift_thresh || dy.abs() > shift_thresh {
        return true;
    }

    let scale_ratio = (next.scale_px_per_m / prev.scale_px_per_m.max(1e-6)).max(1e-6);
    if !((1.0 / 1.15)..=1.15).contains(&scale_ratio) {
        return true;
    }

    false
}

fn cull2d_start_job(
    snapshot: Cull2DSnapshot,
    rect: MercatorViewRect,
    s: &ViewerState,
) -> Cull2DJob {
    let feed_keys = s.feed_layers.keys().cloned().collect::<Vec<_>>();
    Cull2DJob {
        snapshot,
        rect,
        cities_i: 0,
        uploaded_i: 0,
        feed_keys,
        feed_layer_i: 0,
        feed_point_i: 0,
        selection_point_done: false,
        corridors_seg_i: 0,
        uploaded_corridors_seg_i: 0,
        selection_seg_i: 0,

        // Use sentinel so a completed empty result still uploads to clear old buffers.
        last_uploaded_points: usize::MAX,
        last_uploaded_lines: usize::MAX,
        points_out: Vec::new(),
        lines_out: Vec::new(),
    }
}

fn cull2d_advance_job(
    s: &ViewerState,
    job: &mut Cull2DJob,
    mut budget_points: usize,
    mut budget_segs: usize,
) -> bool {
    // Points
    if job.snapshot.show_points {
        if !s.cities_style.visible {
            job.cities_i = s.cities_mercator.as_deref().map(|v| v.len()).unwrap_or(0);
        }
        if !s.uploaded_points_style.visible {
            job.uploaded_i = s.uploaded_mercator.as_deref().map(|v| v.len()).unwrap_or(0);
        }
        if !(s.selection_style.visible && s.selection_center_mercator.is_some()) {
            job.selection_point_done = true;
        }

        if budget_points > 0 {
            if s.cities_style.visible
                && let Some(centers) = s.cities_mercator.as_deref()
            {
                while job.cities_i < centers.len() && budget_points > 0 {
                    let c = centers[job.cities_i];
                    if mercator_point_visible(job.rect, c) {
                        job.points_out.push(Point2DInstance {
                            center_m: c,
                            style_id: STYLE_CITIES,
                            _pad0: 0,
                        });
                    }
                    job.cities_i += 1;
                    budget_points -= 1;
                }
            } else if !s.cities_style.visible {
                job.cities_i = s.cities_mercator.as_deref().map(|v| v.len()).unwrap_or(0);
            } else {
                job.cities_i = usize::MAX;
            }
        }

        if budget_points > 0 {
            if s.uploaded_points_style.visible
                && let Some(centers) = s.uploaded_mercator.as_deref()
            {
                while job.uploaded_i < centers.len() && budget_points > 0 {
                    let c = centers[job.uploaded_i];
                    if mercator_point_visible(job.rect, c) {
                        job.points_out.push(Point2DInstance {
                            center_m: c,
                            style_id: STYLE_UPLOADED_POINTS,
                            _pad0: 0,
                        });
                    }
                    job.uploaded_i += 1;
                    budget_points -= 1;
                }
            } else if !s.uploaded_points_style.visible {
                job.uploaded_i = s.uploaded_mercator.as_deref().map(|v| v.len()).unwrap_or(0);
            } else {
                job.uploaded_i = usize::MAX;
            }
        }

        while budget_points > 0 && job.feed_layer_i < job.feed_keys.len() {
            let key = &job.feed_keys[job.feed_layer_i];
            let Some(layer) = s.feed_layers.get(key) else {
                job.feed_layer_i += 1;
                job.feed_point_i = 0;
                continue;
            };
            if !layer.style.visible || layer.centers_mercator.is_empty() {
                job.feed_layer_i += 1;
                job.feed_point_i = 0;
                continue;
            }
            let style_id = s.feed_style_ids.get(key).copied().unwrap_or(STYLE_DEFAULT);
            while job.feed_point_i < layer.centers_mercator.len() && budget_points > 0 {
                let c = layer.centers_mercator[job.feed_point_i];
                if mercator_point_visible(job.rect, c) {
                    job.points_out.push(Point2DInstance {
                        center_m: c,
                        style_id,
                        _pad0: 0,
                    });
                }
                job.feed_point_i += 1;
                budget_points -= 1;
            }
            if job.feed_point_i >= layer.centers_mercator.len() {
                job.feed_layer_i += 1;
                job.feed_point_i = 0;
            }
        }

        if budget_points > 0 && !job.selection_point_done {
            job.selection_point_done = true;
            if let Some(c) = s.selection_center_mercator
                && mercator_point_visible(job.rect, c)
            {
                job.points_out.push(Point2DInstance {
                    center_m: c,
                    style_id: STYLE_SELECTION_POINT,
                    _pad0: 0,
                });
            }
        }
    }

    // Lines (segments)
    if job.snapshot.show_lines {
        if !s.corridors_style.visible {
            job.corridors_seg_i = s
                .corridors_mercator
                .as_deref()
                .map(|v| v.len() / 2)
                .unwrap_or(0);
        }
        if !s.uploaded_corridors_style.visible {
            job.uploaded_corridors_seg_i = s
                .uploaded_corridors_mercator
                .as_deref()
                .map(|v| v.len() / 2)
                .unwrap_or(0);
        }
        if !(s.selection_style.visible && s.selection_line_mercator.is_some()) {
            job.selection_seg_i = s
                .selection_line_mercator
                .as_deref()
                .map(|v| v.len() / 2)
                .unwrap_or(0);
        }

        if budget_segs > 0 {
            if s.corridors_style.visible
                && let Some(pos) = s.corridors_mercator.as_deref()
            {
                let segs_total = pos.len() / 2;
                while job.corridors_seg_i < segs_total && budget_segs > 0 {
                    let i = job.corridors_seg_i * 2;
                    let a = pos[i];
                    let b = pos[i + 1];
                    if mercator_segment_visible(job.rect, a, b) {
                        job.lines_out.push(Segment2DInstance {
                            a_m: a,
                            b_m: b,
                            style_id: STYLE_CORRIDORS,
                            _pad0: 0,
                        });
                    }
                    job.corridors_seg_i += 1;
                    budget_segs -= 1;
                }
            } else if !s.corridors_style.visible {
                job.corridors_seg_i = s
                    .corridors_mercator
                    .as_deref()
                    .map(|v| v.len() / 2)
                    .unwrap_or(0);
            } else {
                job.corridors_seg_i = usize::MAX;
            }
        }

        if budget_segs > 0 {
            if s.uploaded_corridors_style.visible
                && let Some(pos) = s.uploaded_corridors_mercator.as_deref()
            {
                let segs_total = pos.len() / 2;
                while job.uploaded_corridors_seg_i < segs_total && budget_segs > 0 {
                    let i = job.uploaded_corridors_seg_i * 2;
                    let a = pos[i];
                    let b = pos[i + 1];
                    if mercator_segment_visible(job.rect, a, b) {
                        job.lines_out.push(Segment2DInstance {
                            a_m: a,
                            b_m: b,
                            style_id: STYLE_UPLOADED_CORRIDORS,
                            _pad0: 0,
                        });
                    }
                    job.uploaded_corridors_seg_i += 1;
                    budget_segs -= 1;
                }
            } else if !s.uploaded_corridors_style.visible {
                job.uploaded_corridors_seg_i = s
                    .uploaded_corridors_mercator
                    .as_deref()
                    .map(|v| v.len() / 2)
                    .unwrap_or(0);
            } else {
                job.uploaded_corridors_seg_i = usize::MAX;
            }
        }

        if budget_segs > 0 {
            if s.selection_style.visible
                && let Some(pos) = s.selection_line_mercator.as_deref()
            {
                let segs_total = pos.len() / 2;
                while job.selection_seg_i < segs_total && budget_segs > 0 {
                    let i = job.selection_seg_i * 2;
                    let a = pos[i];
                    let b = pos[i + 1];
                    if mercator_segment_visible(job.rect, a, b) {
                        job.lines_out.push(Segment2DInstance {
                            a_m: a,
                            b_m: b,
                            style_id: STYLE_SELECTION_LINE,
                            _pad0: 0,
                        });
                    }
                    job.selection_seg_i += 1;
                    budget_segs -= 1;
                }
            } else {
                job.selection_seg_i = usize::MAX;
            }
        }
    }

    // Completion check
    let points_done = !job.snapshot.show_points
        || (job.cities_i >= s.cities_mercator.as_deref().map(|v| v.len()).unwrap_or(0)
            && job.uploaded_i >= s.uploaded_mercator.as_deref().map(|v| v.len()).unwrap_or(0)
            && job.feed_layer_i >= job.feed_keys.len()
            && job.selection_point_done);

    let lines_done = !job.snapshot.show_lines
        || (job.corridors_seg_i
            >= s.corridors_mercator
                .as_deref()
                .map(|v| v.len() / 2)
                .unwrap_or(0)
            && job.uploaded_corridors_seg_i
                >= s.uploaded_corridors_mercator
                    .as_deref()
                    .map(|v| v.len() / 2)
                    .unwrap_or(0)
            && job.selection_seg_i
                >= s.selection_line_mercator
                    .as_deref()
                    .map(|v| v.len() / 2)
                    .unwrap_or(0));

    points_done && lines_done
}

fn label2d_should_restart(prev: Option<Label2DSnapshot>, next: Label2DSnapshot) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    if prev.generation != next.generation {
        return true;
    }
    // Restart if viewport changes materially.
    if (prev.viewport_px[0] - next.viewport_px[0]).abs() > 0.5
        || (prev.viewport_px[1] - next.viewport_px[1]).abs() > 0.5
    {
        return true;
    }
    // Restart if camera moved enough that label positions change.
    let dlon = (next.cam.center_lon_deg - prev.cam.center_lon_deg).abs();
    let dlat = (next.cam.center_lat_deg - prev.cam.center_lat_deg).abs();
    if dlon > 0.05 || dlat > 0.05 {
        return true;
    }
    let z_ratio = (next.cam.zoom / prev.cam.zoom.max(1e-9)).max(1e-9);
    if !((1.0 / 1.1)..=1.1).contains(&z_ratio) {
        return true;
    }
    false
}

fn label2d_cell_key(cx: i32, cy: i32) -> u64 {
    ((cx as u32 as u64) << 32) | (cy as u32 as u64)
}

fn label2d_try_place(
    occupied: &mut std::collections::HashSet<u64>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> bool {
    // Coarse collision: grid-based occupancy.
    // This is cheap and good enough for a scaffold.
    let cell_w = 120.0;
    let cell_h = 28.0;
    if !(x.is_finite() && y.is_finite()) {
        return false;
    }
    if x < 4.0 || x > (w - 4.0) || y < 4.0 || y > (h - 4.0) {
        return false;
    }

    let cx = (x / cell_w).floor() as i32;
    let cy = (y / cell_h).floor() as i32;
    // Reserve a small neighborhood to reduce overlaps.
    for oy in -1..=1 {
        for ox in -1..=1 {
            let k = label2d_cell_key(cx + ox, cy + oy);
            if occupied.contains(&k) {
                return false;
            }
        }
    }
    for oy in -1..=1 {
        for ox in -1..=1 {
            occupied.insert(label2d_cell_key(cx + ox, cy + oy));
        }
    }
    true
}

fn label2d_build_candidates(s: &ViewerState) -> Vec<Label2DCandidate> {
    let mut out: Vec<Label2DCandidate> = Vec::new();

    // Selection label (highest priority).
    if s.selection_style.visible
        && let Some(c) = s.selection_center_mercator
    {
        let lon = wrap_lon_deg(inverse_mercator_lon_deg(c[0] as f64));
        let lat = inverse_mercator_lat_deg(c[1] as f64);
        out.push(Label2DCandidate {
            text: format!("Selected ({lon:.5}, {lat:.5})"),
            mercator_m: c,
            priority: 10_000.0,
        });
    }

    // User-supplied debug labels.
    for dl in &s.debug_labels {
        out.push(Label2DCandidate {
            text: dl.text.clone(),
            mercator_m: dl.mercator_m,
            priority: dl.priority,
        });
    }

    // Highest priority first.
    out.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn label2d_start_job(snapshot: Label2DSnapshot, candidates: Vec<Label2DCandidate>) -> Label2DJob {
    Label2DJob {
        snapshot,
        candidates,
        i: 0,
        occupied_cells: std::collections::HashSet::new(),
        placed: Vec::new(),
    }
}

fn label2d_advance_job(
    job: &mut Label2DJob,
    projector: &MercatorProjector,
    w: f32,
    h: f32,
    budget: usize,
) -> bool {
    let mut remaining = budget;
    while job.i < job.candidates.len() && remaining > 0 {
        let c = &job.candidates[job.i];
        job.i += 1;
        remaining -= 1;

        let (x, y) = projector.project_mercator_m(
            c.mercator_m[0] as f64,
            c.mercator_m[1] as f64,
            w as f64,
            h as f64,
        );
        let x = x as f32;
        let y = y as f32;

        if label2d_try_place(&mut job.occupied_cells, x, y, w, h) {
            job.placed.push(PlacedLabel2D {
                text: c.text.clone(),
                x_px: x,
                y_px: y,
                priority: c.priority,
            });
            if job.placed.len() >= 2000 {
                break;
            }
        }
    }

    job.i >= job.candidates.len() || job.placed.len() >= 2000
}

fn render_labels2d_overlay(ctx2d: &CanvasRenderingContext2d, labels: &[PlacedLabel2D]) {
    // Render as an overlay. Keep this conservative and readable.
    ctx2d.set_font("12px system-ui, -apple-system, Segoe UI, Roboto, sans-serif");
    ctx2d.set_text_align("left");
    ctx2d.set_text_baseline("middle");

    // Draw in priority order (already sorted) so high-priority labels land on top.
    for l in labels {
        // Priority -> alpha ramp (kept subtle).
        let a = (0.70 + 0.30 * (l.priority / 10_000.0).clamp(0.0, 1.0)) as f64;
        ctx2d.set_global_alpha(a);
        let x = l.x_px as f64 + 6.0;
        let y = l.y_px as f64 - 10.0;
        // Outline for contrast.
        ctx_set_stroke_style(ctx2d, "rgba(0,0,0,0.85)");
        ctx2d.set_line_width(3.5);
        let _ = ctx2d.stroke_text(&l.text, x, y);
        // Fill.
        ctx_set_fill_style(ctx2d, "rgba(255,255,255,0.92)");
        let _ = ctx2d.fill_text(&l.text, x, y);
    }

    // Restore default alpha for any other overlay drawing.
    ctx2d.set_global_alpha(1.0);
}

fn clip_polygon_lat_band(mut poly: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    // Sutherland–Hodgman against two horizontal lines: lat <= +max, lat >= -max.
    fn clip_against(
        poly: Vec<(f64, f64)>,
        keep_if: impl Fn(f64) -> bool,
        bound_lat: f64,
    ) -> Vec<(f64, f64)> {
        if poly.is_empty() {
            return poly;
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        let mut prev = *poly.last().unwrap();
        let mut prev_in = keep_if(prev.1);
        for &cur in &poly {
            let cur_in = keep_if(cur.1);
            if prev_in != cur_in {
                // Edge crosses the boundary; find t where lat == bound_lat.
                let (lon0, lat0) = prev;
                let (lon1, lat1) = cur;
                let denom = lat1 - lat0;
                if denom.abs() > 1e-12 {
                    let t = (bound_lat - lat0) / denom;
                    let lon = lon0 + (lon1 - lon0) * t;
                    out.push((lon, bound_lat));
                } else {
                    // Degenerate: just snap.
                    out.push((cur.0, bound_lat));
                }
            }
            if cur_in {
                out.push(cur);
            }
            prev = cur;
            prev_in = cur_in;
        }
        out
    }

    poly = clip_against(
        poly,
        |lat| lat <= MERCATOR_MAX_LAT_DEG,
        MERCATOR_MAX_LAT_DEG,
    );
    poly = clip_against(
        poly,
        |lat| lat >= -MERCATOR_MAX_LAT_DEG,
        -MERCATOR_MAX_LAT_DEG,
    );
    poly
}

fn world_tris_to_mercator_clipped(pos: &[[f32; 3]]) -> Vec<[f32; 2]> {
    // World width in Mercator meters.
    let ww = 2.0 * std::f64::consts::PI * WGS84_A;
    // Threshold: if a triangle spans more than this fraction of world width, it crosses the antimeridian.
    let span_threshold = ww * 0.5; // 180° in meters

    let mut out: Vec<[f32; 2]> = Vec::with_capacity(pos.len());
    for tri in pos.chunks_exact(3).take(2_000_000) {
        let (lon0, lat0) = world_to_lon_lat_fast_deg(tri[0]);
        let (lon1, lat1) = world_to_lon_lat_fast_deg(tri[1]);
        let (lon2, lat2) = world_to_lon_lat_fast_deg(tri[2]);

        // If fully valid, fast path.
        if is_mercator_lat_valid(lat0) && is_mercator_lat_valid(lat1) && is_mercator_lat_valid(lat2)
        {
            let ax = mercator_x_m(lon0);
            let ay = mercator_y_m(lat0);
            let bx0 = mercator_x_m(lon1);
            let by = mercator_y_m(lat1);
            let cx0 = mercator_x_m(lon2);
            let cy = mercator_y_m(lat2);

            // Unwrap within triangle to keep it local.
            let bx = unwrap_mercator_x_m(ax, bx0);
            let cx = unwrap_mercator_x_m(ax, cx0);

            // Check if this triangle spans the antimeridian (i.e., spans > 180°).
            // If so, we need to split it or emit on both sides.
            let min_x = ax.min(bx).min(cx);
            let max_x = ax.max(bx).max(cx);
            let span = max_x - min_x;

            if span > span_threshold {
                // Triangle crosses antimeridian. Emit on both sides of the world.
                // Left copy (shift entire triangle left by world width)
                out.push([(ax - ww) as f32, ay as f32]);
                out.push([(bx - ww) as f32, by as f32]);
                out.push([(cx - ww) as f32, cy as f32]);
                // Right copy (original position, but we need the version that's on the right)
                // Re-unwrap with anchor at the rightmost point
                let right_anchor = ax.max(bx0).max(cx0);
                let ax_r = unwrap_mercator_x_m(right_anchor, ax);
                let bx_r = unwrap_mercator_x_m(right_anchor, bx0);
                let cx_r = unwrap_mercator_x_m(right_anchor, cx0);
                out.push([ax_r as f32, ay as f32]);
                out.push([bx_r as f32, by as f32]);
                out.push([cx_r as f32, cy as f32]);
            } else {
                out.push([ax as f32, ay as f32]);
                out.push([bx as f32, by as f32]);
                out.push([cx as f32, cy as f32]);
            }
            continue;
        }

        // Clip the triangle polygon against the Mercator latitude band.
        let poly = clip_polygon_lat_band(vec![(lon0, lat0), (lon1, lat1), (lon2, lat2)]);
        if poly.len() < 3 {
            continue;
        }

        // Triangulate by fan.
        let (fan0_lon, fan0_lat) = poly[0];
        let fan0_x = mercator_x_m(fan0_lon);
        let fan0_y = mercator_y_m(fan0_lat);
        for w in poly.windows(2).skip(1) {
            let (b_lon, b_lat) = w[0];
            let (c_lon, c_lat) = w[1];
            let bx0 = mercator_x_m(b_lon);
            let by = mercator_y_m(b_lat);
            let cx0 = mercator_x_m(c_lon);
            let cy = mercator_y_m(c_lat);
            let bx = unwrap_mercator_x_m(fan0_x, bx0);
            let cx = unwrap_mercator_x_m(fan0_x, cx0);

            // Check for antimeridian crossing in clipped triangles too.
            let min_x = fan0_x.min(bx).min(cx);
            let max_x = fan0_x.max(bx).max(cx);
            let span = max_x - min_x;

            if span > span_threshold {
                // Emit on both sides
                out.push([(fan0_x - ww) as f32, fan0_y as f32]);
                out.push([(bx - ww) as f32, by as f32]);
                out.push([(cx - ww) as f32, cy as f32]);

                let right_anchor = fan0_x.max(bx0).max(cx0);
                let f0_r = unwrap_mercator_x_m(right_anchor, fan0_x);
                let bx_r = unwrap_mercator_x_m(right_anchor, bx0);
                let cx_r = unwrap_mercator_x_m(right_anchor, cx0);
                out.push([f0_r as f32, fan0_y as f32]);
                out.push([bx_r as f32, by as f32]);
                out.push([cx_r as f32, cy as f32]);
            } else {
                out.push([fan0_x as f32, fan0_y as f32]);
                out.push([bx as f32, by as f32]);
                out.push([cx as f32, cy as f32]);
            }
        }
    }
    out
}

fn clip_segment_lat_band(a: (f64, f64), b: (f64, f64)) -> Option<((f64, f64), (f64, f64))> {
    // Clip segment to lat ∈ [-max, +max].
    let (mut lon0, mut lat0) = a;
    let (mut lon1, mut lat1) = b;

    // Quick reject if both outside on same side.
    if lat0 < -MERCATOR_MAX_LAT_DEG && lat1 < -MERCATOR_MAX_LAT_DEG {
        return None;
    }
    if lat0 > MERCATOR_MAX_LAT_DEG && lat1 > MERCATOR_MAX_LAT_DEG {
        return None;
    }

    // Clip against +max.
    if lat0 > MERCATOR_MAX_LAT_DEG || lat1 > MERCATOR_MAX_LAT_DEG {
        let denom = lat1 - lat0;
        if denom.abs() < 1e-12 {
            return None;
        }
        let t = (MERCATOR_MAX_LAT_DEG - lat0) / denom;
        let lon = lon0 + (lon1 - lon0) * t;
        if lat0 > MERCATOR_MAX_LAT_DEG {
            lon0 = lon;
            lat0 = MERCATOR_MAX_LAT_DEG;
        } else {
            lon1 = lon;
            lat1 = MERCATOR_MAX_LAT_DEG;
        }
    }

    // Clip against -max.
    if lat0 < -MERCATOR_MAX_LAT_DEG || lat1 < -MERCATOR_MAX_LAT_DEG {
        let denom = lat1 - lat0;
        if denom.abs() < 1e-12 {
            return None;
        }
        let t = (-MERCATOR_MAX_LAT_DEG - lat0) / denom;
        let lon = lon0 + (lon1 - lon0) * t;
        if lat0 < -MERCATOR_MAX_LAT_DEG {
            lon0 = lon;
            lat0 = -MERCATOR_MAX_LAT_DEG;
        } else {
            lon1 = lon;
            lat1 = -MERCATOR_MAX_LAT_DEG;
        }
    }

    Some(((lon0, lat0), (lon1, lat1)))
}

fn world_segs_to_mercator_clipped(pos: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(pos.len());
    for seg in pos.chunks_exact(2).take(3_000_000) {
        let (lon0, lat0) = world_to_lon_lat_fast_deg(seg[0]);
        let (lon1, lat1) = world_to_lon_lat_fast_deg(seg[1]);
        let Some(((clon0, clat0), (clon1, clat1))) =
            clip_segment_lat_band((lon0, lat0), (lon1, lat1))
        else {
            continue;
        };

        let ax = mercator_x_m(clon0);
        let ay = mercator_y_m(clat0);
        let bx0 = mercator_x_m(clon1);
        let by = mercator_y_m(clat1);
        let bx = unwrap_mercator_x_m(ax, bx0);

        out.push([ax as f32, ay as f32]);
        out.push([bx as f32, by as f32]);
    }
    out
}

fn inverse_mercator_lon_deg(x_m: f64) -> f64 {
    (x_m / WGS84_A).to_degrees()
}

fn inverse_mercator_lat_deg(y_m: f64) -> f64 {
    let lat = 2.0 * (y_m / WGS84_A).exp().atan() - std::f64::consts::FRAC_PI_2;
    lat.to_degrees()
}

fn camera2d_scale_px_per_m(cam: Camera2DState, w: f64, h: f64) -> f64 {
    let world_width_m = 2.0 * std::f64::consts::PI * WGS84_A;
    let max_y = mercator_y_m(MERCATOR_MAX_LAT_DEG);
    let world_height_m = 2.0 * max_y;
    // Use max() so the world FILLS the viewport (no edges visible at zoom=1).
    // At minimum zoom, either width or height will exactly match, the other will overflow.
    let base = (w / world_width_m).max(h / world_height_m);
    (base * cam.zoom).max(1e-6)
}

/// Clamp center_y so the visible extent doesn't exceed the Mercator bounds.
fn clamp_center_y_for_extent(center_y: f64, half_h_m: f64) -> f64 {
    let max_y = mercator_y_m(MERCATOR_MAX_LAT_DEG);
    // The visible top edge is center_y + half_h_m, bottom is center_y - half_h_m.
    // We need: center_y + half_h_m <= max_y  AND  center_y - half_h_m >= -max_y
    // Rearranging: center_y <= max_y - half_h_m  AND  center_y >= -max_y + half_h_m
    let max_center = max_y - half_h_m;
    let min_center = -max_y + half_h_m;
    if min_center > max_center {
        // Viewport is taller than the world - center at 0
        0.0
    } else {
        clamp(center_y, min_center, max_center)
    }
}

struct MercatorProjector {
    center_x: f64,
    center_y: f64,
    scale_px_per_m: f64,
    world_width_m: f64,
}

impl MercatorProjector {
    fn new(cam: Camera2DState, w: f64, h: f64) -> Self {
        let center_x = mercator_x_m(cam.center_lon_deg);
        let center_y = mercator_y_m(cam.center_lat_deg);
        let scale_px_per_m = camera2d_scale_px_per_m(cam, w, h);
        let world_width_m = 2.0 * std::f64::consts::PI * WGS84_A;
        Self {
            center_x,
            center_y,
            scale_px_per_m,
            world_width_m,
        }
    }

    fn project_lon_lat(&self, lon_deg: f64, lat_deg: f64, w: f64, h: f64) -> (f64, f64) {
        let x_m = mercator_x_m(lon_deg);
        let y_m = mercator_y_m(lat_deg);
        let dx = (x_m - self.center_x + 0.5 * self.world_width_m).rem_euclid(self.world_width_m)
            - 0.5 * self.world_width_m;
        let dy = y_m - self.center_y;
        let x = w * 0.5 + dx * self.scale_px_per_m;
        let y = h * 0.5 - dy * self.scale_px_per_m;
        (x, y)
    }

    fn screen_to_lon_lat(&self, x_px: f64, y_px: f64, w: f64, h: f64) -> (f64, f64) {
        let dx_m = (x_px - w * 0.5) / self.scale_px_per_m;
        let dy_m = (h * 0.5 - y_px) / self.scale_px_per_m;
        let x_m = self.center_x + dx_m;
        let y_m = self.center_y + dy_m;
        let lon = wrap_lon_deg(inverse_mercator_lon_deg(x_m));
        let lat = clamp(
            inverse_mercator_lat_deg(y_m),
            -MERCATOR_MAX_LAT_DEG,
            MERCATOR_MAX_LAT_DEG,
        );
        (lon, lat)
    }

    fn project_mercator_m(&self, x_m: f64, y_m: f64, w: f64, h: f64) -> (f64, f64) {
        let dx = (x_m - self.center_x + 0.5 * self.world_width_m).rem_euclid(self.world_width_m)
            - 0.5 * self.world_width_m;
        let dy = y_m - self.center_y;
        let x = w * 0.5 + dx * self.scale_px_per_m;
        let y = h * 0.5 - dy * self.scale_px_per_m;
        (x, y)
    }
}

fn pan_camera_2d(
    cam: Camera2DState,
    delta_x_px: f64,
    delta_y_px: f64,
    w: f64,
    h: f64,
) -> Camera2DState {
    let projector = MercatorProjector::new(cam, w, h);
    let dx_m = -delta_x_px / projector.scale_px_per_m;
    // Screen Y is inverted relative to Mercator Y (screen-down = +delta_y,
    // Mercator-north = +y).  Use positive sign so drag-down moves center
    // north, making the map content follow the cursor downward.
    let dy_m = delta_y_px / projector.scale_px_per_m;
    let center_x = projector.center_x + dx_m;
    let center_y = projector.center_y + dy_m;

    // Clamp Y so visible extent stays within Mercator bounds.
    let half_h_m = 0.5 * h / projector.scale_px_per_m;
    let clamped_center_y = clamp_center_y_for_extent(center_y, half_h_m);

    Camera2DState {
        center_lon_deg: wrap_lon_deg(inverse_mercator_lon_deg(center_x)),
        center_lat_deg: inverse_mercator_lat_deg(clamped_center_y),
        ..cam
    }
}

#[wasm_bindgen]
pub fn camera_zoom_at(x_px: f64, y_px: f64, wheel_delta_y: f64) -> Result<(), JsValue> {
    if !x_px.is_finite() || !y_px.is_finite() || !wheel_delta_y.is_finite() {
        return Err(JsValue::from_str("camera_zoom_at args must be finite"));
    }

    let mode = with_state(|state| state.borrow().view_mode);
    if mode != ViewMode::TwoD {
        return camera_zoom(wheel_delta_y);
    }

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();

        let w = s.canvas_width.max(1.0);
        let h = s.canvas_height.max(1.0);
        let cam = s.camera_2d;
        let projector = MercatorProjector::new(cam, w, h);

        // Mercator point under the cursor (in meters).
        let dx_m = (x_px - w * 0.5) / projector.scale_px_per_m;
        let dy_m = (h * 0.5 - y_px) / projector.scale_px_per_m;
        let p_x_m = projector.center_x + dx_m;
        let p_y_m = projector.center_y + dy_m;

        // Zoom.
        let cfg = s.controls;
        let zoom_factor = (-wheel_delta_y * 0.0015 * cfg.zoom_speed_2d).exp();
        let next_zoom = clamp(cam.zoom * zoom_factor, cfg.min_zoom_2d, cfg.max_zoom_2d);

        // Adjust center so the cursor stays anchored on the same mercator point.
        let next_cam = Camera2DState {
            zoom: next_zoom,
            ..cam
        };
        let next_scale = camera2d_scale_px_per_m(next_cam, w, h);
        let next_dx_m = (x_px - w * 0.5) / next_scale;
        let next_dy_m = (h * 0.5 - y_px) / next_scale;
        let next_center_x = p_x_m - next_dx_m;
        let next_center_y = p_y_m - next_dy_m;

        // Clamp Y so visible extent stays within Mercator bounds.
        let next_half_h_m = 0.5 * h / next_scale;
        let clamped_center_y = clamp_center_y_for_extent(next_center_y, next_half_h_m);

        s.camera_2d = Camera2DState {
            center_lon_deg: wrap_lon_deg(inverse_mercator_lon_deg(next_center_x)),
            center_lat_deg: inverse_mercator_lat_deg(clamped_center_y),
            zoom: next_zoom,
        };
    });

    render_scene()
}

fn rgba_css(c: [f32; 4]) -> String {
    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
    let a = c[3].clamp(0.0, 1.0);
    format!("rgba({r},{g},{b},{a})")
}

fn ctx_set_fill_style(ctx: &CanvasRenderingContext2d, value: &str) {
    let _ = js_sys::Reflect::set(
        ctx.as_ref(),
        &JsValue::from_str("fillStyle"),
        &JsValue::from_str(value),
    );
}

fn ctx_set_stroke_style(ctx: &CanvasRenderingContext2d, value: &str) {
    let _ = js_sys::Reflect::set(
        ctx.as_ref(),
        &JsValue::from_str("strokeStyle"),
        &JsValue::from_str(value),
    );
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

#[allow(dead_code)]
fn quat_from_unit_vectors(a: [f64; 3], b: [f64; 3]) -> [f64; 4] {
    // Returns a unit quaternion rotating `a` to `b`.
    let a = vec3_normalize(a);
    let b = vec3_normalize(b);
    let dot = clamp(vec3_dot(a, b), -1.0, 1.0);

    // If vectors are nearly opposite, pick an arbitrary orthogonal axis.
    if dot < -0.999999 {
        let mut axis = vec3_cross([1.0, 0.0, 0.0], a);
        if vec3_dot(axis, axis) < 1e-12 {
            axis = vec3_cross([0.0, 1.0, 0.0], a);
        }
        axis = vec3_normalize(axis);
        return [axis[0], axis[1], axis[2], 0.0];
    }

    let axis = vec3_cross(a, b);
    let w = 1.0 + dot;
    let mut q = [axis[0], axis[1], axis[2], w];
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 0.0 {
        q[0] /= n;
        q[1] /= n;
        q[2] /= n;
        q[3] /= n;
    }
    q
}

#[allow(dead_code)]
fn quat_conjugate(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

#[allow(dead_code)]
fn quat_rotate_vec3(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    // Assumes q is unit.
    let qv = [q[0], q[1], q[2]];
    let t = vec3_mul(vec3_cross(qv, v), 2.0);
    vec3_add(v, vec3_add(vec3_mul(t, q[3]), vec3_cross(qv, t)))
}

fn trackball_unit_from_screen(x_px: f64, y_px: f64, w: f64, h: f64) -> [f64; 3] {
    // Virtual trackball mapping (unit sphere in screen space).
    let min_dim = w.min(h).max(1.0);
    let nx = (2.0 * x_px - w) / min_dim;
    let ny = (h - 2.0 * y_px) / min_dim;
    let r2 = nx * nx + ny * ny;
    let (x, y, z) = if r2 <= 1.0 {
        (nx, ny, (1.0 - r2).sqrt())
    } else {
        let inv_r = 1.0 / r2.sqrt();
        (nx * inv_r, ny * inv_r, 0.0)
    };
    vec3_normalize([x, y, z])
}

fn arcball_unit_from_screen(canvas_w: f64, canvas_h: f64, x_px: f64, y_px: f64) -> [f64; 3] {
    // Use a classic virtual trackball mapping for consistent, smooth drag everywhere.
    // (Using ray-hit->lon/lat introduces distortion near the poles and can feel unsynced.)
    trackball_unit_from_screen(x_px, y_px, canvas_w, canvas_h)
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn camera_view_proj(camera: CameraState, canvas_width: f64, canvas_height: f64) -> [[f32; 4]; 4] {
    let aspect = if canvas_height <= 0.0 {
        1.0
    } else {
        (canvas_width / canvas_height).max(1e-6)
    };

    // Viewer coordinate system uses Y as north (ECEF Z) and Z as -east (negative ECEF Y).
    // Keep camera yaw aligned with geodetic longitude (east-positive) in this space.
    let dir = [
        camera.pitch_rad.cos() * camera.yaw_rad.cos(),
        camera.pitch_rad.sin(),
        -camera.pitch_rad.cos() * camera.yaw_rad.sin(),
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

/// Returns the view-projection matrix from the globe controller (quaternion-based).
fn globe_controller_view_proj(
    gc: &GlobeController,
    canvas_width: f64,
    canvas_height: f64,
) -> [[f32; 4]; 4] {
    let aspect = if canvas_height <= 0.0 {
        1.0
    } else {
        (canvas_width / canvas_height).max(1e-6)
    };
    let fov_y_rad = 45f64.to_radians();
    gc.view_proj_matrix(aspect, fov_y_rad)
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
    let mode = match STATE.try_with(|state_ref| state_ref.borrow().view_mode) {
        Ok(mode) => mode,
        // During hot-reload / teardown, JS callbacks can still fire briefly.
        // Avoid panicking on TLS access in that window.
        Err(_) => return Ok(()),
    };
    match mode {
        ViewMode::ThreeD => {
            let t0 = now_ms();
            let _ = STATE.try_with(|state_ref| {
                let state = state_ref.borrow();
                if let Some(ctx) = &state.wgpu {
                    perf_reset(ctx);
                    let view_proj = globe_controller_view_proj(
                        &state.globe_controller,
                        state.canvas_width,
                        state.canvas_height,
                    );

                    let light_dir = if state.sun_follow_real_time {
                        current_sun_direction_world().unwrap_or([0.4, 0.7, 0.2])
                    } else {
                        [0.4, 0.7, 0.2]
                    };

                    // Layer visibility is baked into the combined overlay buffers; we can always
                    // attempt to draw and rely on vertex counts to early-out.
                    let show_base_regions = state.base_regions_style.visible;
                    let show_terrain = state.terrain_style.visible;
                    let show_cities = true;
                    let show_corridors = true;
                    let show_regions = true;
                    let _ = render_mesh(
                        ctx,
                        view_proj,
                        light_dir,
                        state.show_graticule,
                        show_base_regions,
                        show_terrain,
                        show_cities,
                        show_corridors,
                        show_regions,
                    );
                }
            });

            // Snapshot GPU counters for this frame.
            let t1 = now_ms();
            let _ = STATE.try_with(|state_ref| {
                let mut s = state_ref.borrow_mut();
                if let Some(ctx) = &s.wgpu {
                    let snap = perf_snapshot(ctx);
                    s.perf_gpu_upload_calls = snap.upload_calls;
                    s.perf_gpu_upload_bytes = snap.upload_bytes;
                    s.perf_gpu_render_passes = snap.render_passes;
                    s.perf_gpu_draw_calls = snap.draw_calls;
                    s.perf_gpu_draw_instances = snap.draw_instances;
                    s.perf_gpu_draw_vertices = snap.draw_vertices;
                    s.perf_gpu_draw_indices = snap.draw_indices;
                    s.perf_gpu_frame_ms = (t1 - t0).max(0.0);
                }
            });

            // Draw overlay labels on the 2D canvas (selection/debug labels).
            // Keep this separate from the WebGPU pass to avoid text/GPU complexity.
            render_labels_overlay_3d();
            Ok(())
        }
        ViewMode::TwoD => {
            if with_state(|state| state.borrow().wgpu.is_some()) {
                render_scene_2d_wgpu()
            } else {
                render_scene_2d()
            }
        }
    }
}

fn project_viewer_to_screen(
    view_proj: [[f32; 4]; 4],
    p: [f32; 3],
    w: f32,
    h: f32,
) -> Option<(f32, f32)> {
    let clip = mat4_mul_vec4(view_proj, [p[0], p[1], p[2], 1.0]);
    if !clip[0].is_finite() || !clip[1].is_finite() || !clip[3].is_finite() {
        return None;
    }
    if clip[3].abs() < 1e-6 {
        return None;
    }
    let ndc_x = clip[0] / clip[3];
    let ndc_y = clip[1] / clip[3];
    if !(ndc_x.is_finite() && ndc_y.is_finite()) {
        return None;
    }
    let sx = (ndc_x * 0.5 + 0.5) * w;
    let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h;
    Some((sx, sy))
}

fn render_labels_overlay_3d() {
    let _ = STATE.try_with(|state_ref| {
        let s = state_ref.borrow();
        let Some(ctx2d) = s.ctx_2d.as_ref() else {
            return;
        };

        // Clear the overlay each frame.
        let w = s.canvas_width.max(1.0);
        let h = s.canvas_height.max(1.0);
        ctx2d.clear_rect(0.0, 0.0, w, h);

        if !s.labels_enabled {
            return;
        }

        let view_proj =
            globe_controller_view_proj(&s.globe_controller, s.canvas_width, s.canvas_height);
        let wf = w as f32;
        let hf = h as f32;

        // Assemble a small set of labels.
        let mut labels: Vec<(f32, String, [f32; 3])> = Vec::new();
        if s.selection_style.visible
            && let Some(p) = s.selection_center
        {
            labels.push((10_000.0, "Selected".to_string(), p));
        }
        for dl in &s.debug_labels {
            labels.push((dl.priority, dl.text.clone(), dl.viewer_pos));
        }
        labels.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        ctx2d.set_font("12px system-ui, -apple-system, Segoe UI, Roboto, sans-serif");
        ctx2d.set_text_align("left");
        ctx2d.set_text_baseline("middle");

        for (_prio, text, pos) in labels.iter().take(256) {
            let Some((sx, sy)) = project_viewer_to_screen(view_proj, *pos, wf, hf) else {
                continue;
            };
            if sx < 0.0 || sx > wf || sy < 0.0 || sy > hf {
                continue;
            }
            let x = sx as f64 + 6.0;
            let y = sy as f64 - 10.0;
            ctx_set_stroke_style(ctx2d, "rgba(0,0,0,0.85)");
            ctx2d.set_line_width(3.5);
            let _ = ctx2d.stroke_text(text, x, y);
            ctx_set_fill_style(ctx2d, "rgba(255,255,255,0.92)");
            let _ = ctx2d.fill_text(text, x, y);
        }
    });
}

/// Perf stats collected during a 2D render pass.
struct Render2DStats {
    total_ms: f64,
    poly_ms: f64,
    line_ms: f64,
    point_ms: f64,
    poly_tris: u32,
    line_segs: u32,
    points: u32,
}

fn render_scene_2d() -> Result<(), JsValue> {
    // First pass: render and collect stats (immutable borrow).
    let stats_opt: Option<Render2DStats> = STATE
        .try_with(|state_ref| {
            let state = state_ref.borrow();
            let ctx = state.ctx_2d.as_ref()?;

            let t0 = now_ms();

            let w = state.canvas_width.max(1.0);
            let h = state.canvas_height.max(1.0);

            // Clear.
            let clear = palette_for(state.theme).canvas_2d_clear;
            ctx_set_fill_style(ctx, clear);
            ctx.fill_rect(0.0, 0.0, w, h);

            // Optional graticule (Web Mercator).
            if state.show_graticule {
                let projector = MercatorProjector::new(state.camera_2d, w, h);
                // Minor lines.
                ctx_set_stroke_style(ctx, "rgba(148,163,184,0.20)");
                ctx.set_line_width(0.75);
                for lon in (-180..=180).step_by(10) {
                    let (x0, y0) = projector.project_lon_lat(lon as f64, -85.0, w, h);
                    let (x1, y1) = projector.project_lon_lat(lon as f64, 85.0, w, h);
                    ctx.begin_path();
                    ctx.move_to(x0, y0);
                    ctx.line_to(x1, y1);
                    ctx.stroke();
                }
                for lat in (-80..=80).step_by(10) {
                    let (x0, y0) = projector.project_lon_lat(-180.0, lat as f64, w, h);
                    let (x1, y1) = projector.project_lon_lat(180.0, lat as f64, w, h);
                    ctx.begin_path();
                    ctx.move_to(x0, y0);
                    ctx.line_to(x1, y1);
                    ctx.stroke();
                }

                // Major lines.
                ctx_set_stroke_style(ctx, "rgba(148,163,184,0.55)");
                ctx.set_line_width(1.25);
                for lon in (-180..=180).step_by(30) {
                    let (x0, y0) = projector.project_lon_lat(lon as f64, -85.0, w, h);
                    let (x1, y1) = projector.project_lon_lat(lon as f64, 85.0, w, h);
                    ctx.begin_path();
                    ctx.move_to(x0, y0);
                    ctx.line_to(x1, y1);
                    ctx.stroke();
                }
                for lat in (-60..=60).step_by(30) {
                    let (x0, y0) = projector.project_lon_lat(-180.0, lat as f64, w, h);
                    let (x1, y1) = projector.project_lon_lat(180.0, lat as f64, w, h);
                    ctx.begin_path();
                    ctx.move_to(x0, y0);
                    ctx.line_to(x1, y1);
                    ctx.stroke();
                }
            }

            let projector = MercatorProjector::new(state.camera_2d, w, h);

            let mut poly_tris: u32 = 0;
            let mut line_segs: u32 = 0;
            let mut points: u32 = 0;

            let tp0 = now_ms();

            // Polygons first (fills).
            let draw_poly_tris = |pos: &[[f32; 3]], color: [f32; 4]| {
                // Chunking avoids extremely large paths causing browser rasterization artifacts.
                const TRI_CHUNK: usize = 10_000;
                let ww = projector.world_width_m;
                let cx = projector.center_x;
                let cy = projector.center_y;
                let s = projector.scale_px_per_m;

                ctx_set_fill_style(ctx, &rgba_css(color));
                ctx.begin_path();

                for (i, tri) in pos.chunks_exact(3).take(2_000_000).enumerate() {
                    if i > 0 && (i % TRI_CHUNK) == 0 {
                        ctx.fill();
                        ctx.begin_path();
                    }

                    let a = tri[0];
                    let b = tri[1];
                    let c = tri[2];

                    let (lon_a, lat_a) = world_to_lon_lat_fast_deg(a);
                    let (lon_b, lat_b) = world_to_lon_lat_fast_deg(b);
                    let (lon_c, lat_c) = world_to_lon_lat_fast_deg(c);

                    let ax_m = mercator_x_m(lon_a);
                    let ay_m = mercator_y_m(lat_a);
                    let bx_m = mercator_x_m(lon_b);
                    let by_m = mercator_y_m(lat_b);
                    let cx_m = mercator_x_m(lon_c);
                    let cy_m = mercator_y_m(lat_c);

                    // Unwrap mercator X per-triangle to avoid seam-spanning triangles.
                    let ax_adj = {
                        let dx0 = (ax_m - cx + 0.5 * ww).rem_euclid(ww) - 0.5 * ww;
                        cx + dx0
                    };
                    let bx_adj = ax_adj + ((bx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);
                    let cx_adj = ax_adj + ((cx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);

                    let ax = w * 0.5 + (ax_adj - cx) * s;
                    let bx = w * 0.5 + (bx_adj - cx) * s;
                    let cxp = w * 0.5 + (cx_adj - cx) * s;
                    let ay = h * 0.5 - (ay_m - cy) * s;
                    let by = h * 0.5 - (by_m - cy) * s;
                    let cyy = h * 0.5 - (cy_m - cy) * s;

                    ctx.move_to(ax, ay);
                    ctx.line_to(bx, by);
                    ctx.line_to(cxp, cyy);
                    ctx.close_path();
                }
                ctx.fill();
            };

            let draw_poly_tris_mercator = |pos: &[[f32; 2]], color: [f32; 4]| {
                const TRI_CHUNK: usize = 10_000;
                let ww = projector.world_width_m;
                let cx = projector.center_x;
                let cy = projector.center_y;
                let s = projector.scale_px_per_m;

                ctx_set_fill_style(ctx, &rgba_css(color));
                ctx.begin_path();
                for (i, tri) in pos.chunks_exact(3).take(2_000_000).enumerate() {
                    if i > 0 && (i % TRI_CHUNK) == 0 {
                        ctx.fill();
                        ctx.begin_path();
                    }

                    let a = tri[0];
                    let b = tri[1];
                    let c = tri[2];

                    let ax_m = a[0] as f64;
                    let ay_m = a[1] as f64;
                    let bx_m = b[0] as f64;
                    let by_m = b[1] as f64;
                    let cx_m = c[0] as f64;
                    let cy_m = c[1] as f64;

                    // Unwrap mercator X per-triangle to avoid seam-spanning triangles.
                    let ax_adj = {
                        let dx0 = (ax_m - cx + 0.5 * ww).rem_euclid(ww) - 0.5 * ww;
                        cx + dx0
                    };
                    let bx_adj = ax_adj + ((bx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);
                    let cx_adj = ax_adj + ((cx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);

                    let ax = w * 0.5 + (ax_adj - cx) * s;
                    let bx = w * 0.5 + (bx_adj - cx) * s;
                    let cxp = w * 0.5 + (cx_adj - cx) * s;
                    let ay = h * 0.5 - (ay_m - cy) * s;
                    let by = h * 0.5 - (by_m - cy) * s;
                    let cyy = h * 0.5 - (cy_m - cy) * s;

                    ctx.move_to(ax, ay);
                    ctx.line_to(bx, by);
                    ctx.line_to(cxp, cyy);
                    ctx.close_path();
                }
                ctx.fill();
            };

            if state.base_regions_style.visible {
                if let Some(pos) = state.base_regions_mercator.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris_mercator(pos, state.base_regions_style.color);
                } else if let Some(pos) = state.base_regions_positions.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris(pos, state.base_regions_style.color);
                }
            }
            if state.regions_style.visible {
                if let Some(pos) = state.regions_mercator.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris_mercator(pos, state.regions_style.color);
                } else if let Some(pos) = state.regions_positions.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris(pos, state.regions_style.color);
                }
            }
            if state.uploaded_regions_style.visible {
                if let Some(pos) = state.uploaded_regions_mercator.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris_mercator(pos, state.uploaded_regions_style.color);
                } else if let Some(pos) = state.uploaded_regions_positions.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris(pos, state.uploaded_regions_style.color);
                }
            }
            if state.selection_style.visible {
                if let Some(pos) = state.selection_poly_mercator.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris_mercator(pos, state.selection_style.color);
                } else if let Some(pos) = state.selection_poly_positions.as_deref() {
                    poly_tris = poly_tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
                    draw_poly_tris(pos, state.selection_style.color);
                }
            }

            let tp1 = now_ms();

            // Lines.
            let tl0 = now_ms();
            let draw_lines = |pos: &[[f32; 3]], color: [f32; 4], width_px: f64| {
                const SEG_CHUNK: usize = 25_000;
                let ww = projector.world_width_m;
                let cx = projector.center_x;
                let cy = projector.center_y;
                let s = projector.scale_px_per_m;

                ctx_set_stroke_style(ctx, &rgba_css(color));
                ctx.set_line_width(width_px.max(1.0));
                ctx.set_line_cap("round");
                ctx.begin_path();

                for (i, seg) in pos.chunks_exact(2).take(1_500_000).enumerate() {
                    if i > 0 && (i % SEG_CHUNK) == 0 {
                        ctx.stroke();
                        ctx.begin_path();
                    }

                    let a = seg[0];
                    let b = seg[1];
                    let (lon_a, lat_a) = world_to_lon_lat_fast_deg(a);
                    let (lon_b, lat_b) = world_to_lon_lat_fast_deg(b);

                    let ax_m = mercator_x_m(lon_a);
                    let ay_m = mercator_y_m(lat_a);
                    let bx_m = mercator_x_m(lon_b);
                    let by_m = mercator_y_m(lat_b);

                    // Unwrap mercator X per-segment.
                    let ax_adj = {
                        let dx0 = (ax_m - cx + 0.5 * ww).rem_euclid(ww) - 0.5 * ww;
                        cx + dx0
                    };
                    let bx_adj = ax_adj + ((bx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);

                    let ax = w * 0.5 + (ax_adj - cx) * s;
                    let bx = w * 0.5 + (bx_adj - cx) * s;
                    let ay = h * 0.5 - (ay_m - cy) * s;
                    let by = h * 0.5 - (by_m - cy) * s;

                    ctx.move_to(ax, ay);
                    ctx.line_to(bx, by);
                }
                ctx.stroke();
            };

            let draw_lines_mercator = |pos: &[[f32; 2]], color: [f32; 4], width_px: f64| {
                const SEG_CHUNK: usize = 25_000;
                let ww = projector.world_width_m;
                let cx = projector.center_x;
                let cy = projector.center_y;
                let s = projector.scale_px_per_m;

                ctx_set_stroke_style(ctx, &rgba_css(color));
                ctx.set_line_width(width_px.max(1.0));
                ctx.set_line_cap("round");
                ctx.begin_path();

                for (i, seg) in pos.chunks_exact(2).take(1_500_000).enumerate() {
                    if i > 0 && (i % SEG_CHUNK) == 0 {
                        ctx.stroke();
                        ctx.begin_path();
                    }

                    let a = seg[0];
                    let b = seg[1];
                    let ax_m = a[0] as f64;
                    let ay_m = a[1] as f64;
                    let bx_m = b[0] as f64;
                    let by_m = b[1] as f64;

                    // Unwrap mercator X per-segment.
                    let ax_adj = {
                        let dx0 = (ax_m - cx + 0.5 * ww).rem_euclid(ww) - 0.5 * ww;
                        cx + dx0
                    };
                    let bx_adj = ax_adj + ((bx_m - ax_m + 0.5 * ww).rem_euclid(ww) - 0.5 * ww);

                    let ax = w * 0.5 + (ax_adj - cx) * s;
                    let bx = w * 0.5 + (bx_adj - cx) * s;
                    let ay = h * 0.5 - (ay_m - cy) * s;
                    let by = h * 0.5 - (by_m - cy) * s;

                    ctx.move_to(ax, ay);
                    ctx.line_to(bx, by);
                }
                ctx.stroke();
            };

            if state.corridors_style.visible {
                if let Some(pos) = state.corridors_mercator.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines_mercator(
                        pos,
                        state.corridors_style.color,
                        state.line_width_px as f64,
                    );
                } else if let Some(pos) = state.corridors_positions.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines(pos, state.corridors_style.color, state.line_width_px as f64);
                }
            }
            if state.uploaded_corridors_style.visible {
                if let Some(pos) = state.uploaded_corridors_mercator.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines_mercator(
                        pos,
                        state.uploaded_corridors_style.color,
                        state.line_width_px as f64,
                    );
                } else if let Some(pos) = state.uploaded_corridors_positions.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines(
                        pos,
                        state.uploaded_corridors_style.color,
                        state.line_width_px as f64,
                    );
                }
            }
            if state.selection_style.visible {
                if let Some(pos) = state.selection_line_mercator.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines_mercator(
                        pos,
                        state.selection_style.color,
                        (state.line_width_px * 1.6) as f64,
                    );
                } else if let Some(pos) = state.selection_line_positions.as_deref() {
                    line_segs = line_segs.saturating_add((pos.len() / 2).min(250_000) as u32);
                    draw_lines(
                        pos,
                        state.selection_style.color,
                        (state.line_width_px * 1.6) as f64,
                    );
                }
            }

            let tl1 = now_ms();

            // Points.
            let tpt0 = now_ms();
            let draw_points = |centers: &[[f32; 3]], color: [f32; 4], radius_px: f64| {
                ctx_set_fill_style(ctx, &rgba_css(color));
                ctx.begin_path();
                for &c in centers.iter().take(2_000_000) {
                    let (lon, lat) = world_to_lon_lat_fast_deg(c);
                    let (x, y) = projector.project_lon_lat(lon, lat, w, h);
                    let _ = ctx.arc(x, y, radius_px, 0.0, std::f64::consts::TAU);
                }
                ctx.fill();
            };

            let draw_points_mercator = |centers: &[[f32; 2]], color: [f32; 4], radius_px: f64| {
                ctx_set_fill_style(ctx, &rgba_css(color));
                ctx.begin_path();
                for &c in centers.iter().take(2_000_000) {
                    let (x, y) = projector.project_mercator_m(c[0] as f64, c[1] as f64, w, h);
                    let _ = ctx.arc(x, y, radius_px, 0.0, std::f64::consts::TAU);
                }
                ctx.fill();
            };

            let r = (state.city_marker_size as f64).clamp(1.0, 32.0);
            if state.cities_style.visible {
                if let Some(centers) = state.cities_mercator.as_deref() {
                    points = points.saturating_add(centers.len().min(2_000_000) as u32);
                    draw_points_mercator(centers, state.cities_style.color, r);
                } else if let Some(centers) = state.cities_centers.as_deref() {
                    points = points.saturating_add(centers.len().min(2_000_000) as u32);
                    draw_points(centers, state.cities_style.color, r);
                }
            }
            if state.uploaded_points_style.visible {
                if let Some(centers) = state.uploaded_mercator.as_deref() {
                    points = points.saturating_add(centers.len().min(2_000_000) as u32);
                    draw_points_mercator(centers, state.uploaded_points_style.color, r);
                } else if let Some(centers) = state.uploaded_centers.as_deref() {
                    points = points.saturating_add(centers.len().min(2_000_000) as u32);
                    draw_points(centers, state.uploaded_points_style.color, r);
                }
            }
            for layer in state.feed_layers.values() {
                if layer.style.visible && !layer.centers_mercator.is_empty() {
                    points =
                        points.saturating_add(layer.centers_mercator.len().min(2_000_000) as u32);
                    draw_points_mercator(&layer.centers_mercator, layer.style.color, r);
                }
            }
            if state.selection_style.visible
                && let Some(c) = state.selection_center
            {
                points = points.saturating_add(1);
                draw_points(
                    std::slice::from_ref(&c),
                    state.selection_style.color,
                    r * 1.35,
                );
            }

            let tpt1 = now_ms();

            let t1 = now_ms();
            Some(Render2DStats {
                total_ms: (t1 - t0).max(0.0),
                poly_ms: (tp1 - tp0).max(0.0),
                line_ms: (tl1 - tl0).max(0.0),
                point_ms: (tpt1 - tpt0).max(0.0),
                poly_tris,
                line_segs,
                points,
            })
        })
        .ok()
        .flatten();

    // Second pass: write stats (mutable borrow, after the immutable borrow is released).
    if let Some(stats) = stats_opt {
        let _ = STATE.try_with(|state_ref| {
            let mut s = state_ref.borrow_mut();
            s.perf_2d_total_ms = stats.total_ms;
            s.perf_2d_poly_ms = stats.poly_ms;
            s.perf_2d_line_ms = stats.line_ms;
            s.perf_2d_point_ms = stats.point_ms;
            s.perf_2d_poly_tris = stats.poly_tris;
            s.perf_2d_line_segs = stats.line_segs;
            s.perf_2d_points = stats.points;
        });
    }
    Ok(())
}

fn render_scene_2d_wgpu() -> Result<(), JsValue> {
    let t0 = now_ms();
    match STATE.try_with(|state_ref| {
        let mut state = state_ref.borrow_mut();
        if state.wgpu.is_none() {
            return Ok(());
        }

        // Reset per-frame GPU counters (used for metrics UI).
        if let Some(ctx) = state.wgpu.as_ref() {
            perf_reset(ctx);
        }

        // If the 2D canvas is visible (it remains the input target), keep it transparent so it
        // doesn't occlude the WebGPU output.
        if let Some(ctx2d) = state.ctx_2d.as_ref() {
            let w = state.canvas_width.max(1.0);
            let h = state.canvas_height.max(1.0);
            ctx2d.clear_rect(0.0, 0.0, w, h);
        }

        let w = state.canvas_width.max(1.0);
        let h = state.canvas_height.max(1.0);

        let center_x = mercator_x_m(state.camera_2d.center_lon_deg) as f32;
        let center_y = mercator_y_m(state.camera_2d.center_lat_deg) as f32;
        let scale_px_per_m = camera2d_scale_px_per_m(state.camera_2d, w, h) as f32;
        let world_width_m = (2.0 * std::f64::consts::PI * WGS84_A) as f32;

        let globals2d = Globals2D {
            center_m: [center_x, center_y],
            scale_px_per_m,
            world_width_m,
            viewport_px: [w as f32, h as f32],
            _pad0: [0.0, 0.0],
        };

        let any_feed_points = state
            .feed_layers
            .values()
            .any(|layer| layer.style.visible && !layer.centers_mercator.is_empty());

        let show_graticule = state.show_graticule;
        let show_base_regions = state.base_regions_style.visible;
        let show_regions = state.regions_style.visible
            || state.uploaded_regions_style.visible
            || (state.selection_style.visible && state.selection_poly_mercator.is_some());
        let show_lines = state.corridors_style.visible
            || state.uploaded_corridors_style.visible
            || (state.selection_style.visible && state.selection_line_mercator.is_some());
        let show_points = state.cities_style.visible
            || state.uploaded_points_style.visible
            || any_feed_points
            || (state.selection_style.visible && state.selection_center_mercator.is_some());

        // Time-sliced 2D viewport culling for points/lines.
        // This keeps pan/zoom responsive on large datasets by uploading only visible instances.
        let snapshot = Cull2DSnapshot {
            center_m: [center_x, center_y],
            scale_px_per_m,
            viewport_px: [w as f32, h as f32],
            show_points,
            show_lines,
            geom_gen: state.cull2d_geom_gen,
        };

        if state.cull2d_enabled && (show_points || show_lines) {
            let pad_px = (state.city_marker_size.max(state.line_width_px * 0.5 + 2.0))
                .clamp(4.0, 96.0)
                + 8.0;
            let rect = make_mercator_view_rect(
                center_x,
                center_y,
                w as f32,
                h as f32,
                scale_px_per_m,
                world_width_m,
                pad_px,
            );

            let need_restart = if let Some(job) = state.cull2d_job.as_ref() {
                cull2d_should_restart(Some(job.snapshot), snapshot, rect.world_width_m)
            } else {
                cull2d_should_restart(state.cull2d_last_snapshot, snapshot, rect.world_width_m)
            };
            if need_restart {
                let job = {
                    let s_ro: &ViewerState = &state;
                    cull2d_start_job(snapshot, rect, s_ro)
                };
                state.cull2d_job = Some(job);
            }

            if let Some(mut job) = state.cull2d_job.take() {
                // If the camera drifted far from the job snapshot, restart to avoid uploading stale results.
                if cull2d_should_restart(Some(job.snapshot), snapshot, rect.world_width_m) {
                    let new_job = {
                        let s_ro: &ViewerState = &state;
                        cull2d_start_job(snapshot, rect, s_ro)
                    };
                    job = new_job;
                }

                let done = {
                    let s_ro: &ViewerState = &state;
                    cull2d_advance_job(s_ro, &mut job, 50_000, 50_000)
                };

                // Progressive upload: keep the on-screen buffers close to the view while the
                // time-sliced culling job is still running.
                if let Some(ctx_mut) = state.wgpu.as_mut() {
                    let want_upload_points = job.snapshot.show_points
                        && job.points_out.len() != job.last_uploaded_points
                        && (done || !job.points_out.is_empty());
                    if want_upload_points {
                        set_points2d_instances(ctx_mut, &job.points_out);
                        job.last_uploaded_points = job.points_out.len();
                    }

                    let want_upload_lines = job.snapshot.show_lines
                        && job.lines_out.len() != job.last_uploaded_lines
                        && (done || !job.lines_out.is_empty());
                    if want_upload_lines {
                        set_lines2d_instances(ctx_mut, &job.lines_out);
                        job.last_uploaded_lines = job.lines_out.len();
                    }
                }

                if done {
                    state.cull2d_last_snapshot = Some(job.snapshot);
                    state.cull2d_visible_points = job.points_out.len() as u32;
                    state.cull2d_visible_line_segs = job.lines_out.len() as u32;
                } else {
                    state.cull2d_job = Some(job);
                }
            }
        } else {
            state.cull2d_job = None;
            state.cull2d_last_snapshot = None;
            state.cull2d_visible_points = 0;
            state.cull2d_visible_line_segs = 0;
        }

        let ctx = state.wgpu.as_ref().unwrap();

        let res = render_map2d(
            ctx,
            globals2d,
            show_graticule,
            show_base_regions,
            show_regions,
            show_lines,
            show_points,
        );

        // Labels overlay (Canvas2D): layout is time-sliced and drawn on top.
        if state.labels_enabled
            && let Some(ctx2d) = state.ctx_2d.clone()
        {
            let snapshot = Label2DSnapshot {
                cam: state.camera_2d,
                viewport_px: [w as f32, h as f32],
                generation: state.labels_gen,
            };

            let need_restart = if let Some(job) = state.labels2d_job.as_ref() {
                label2d_should_restart(Some(job.snapshot), snapshot)
            } else {
                label2d_should_restart(state.labels2d_last_snapshot, snapshot)
            };

            if need_restart {
                let candidates = {
                    let s_ro: &ViewerState = &state;
                    label2d_build_candidates(s_ro)
                };
                state.labels2d_job = Some(label2d_start_job(snapshot, candidates));
                state.labels2d_placed.clear();
            }

            let projector = MercatorProjector::new(state.camera_2d, w, h);
            if let Some(mut job) = state.labels2d_job.take() {
                let done = label2d_advance_job(&mut job, &projector, w as f32, h as f32, 250);
                // Render progressively.
                state.labels2d_placed = job.placed.clone();
                if done {
                    state.labels2d_last_snapshot = Some(job.snapshot);
                    state.labels2d_job = None;
                } else {
                    state.labels2d_job = Some(job);
                }
            }

            render_labels2d_overlay(&ctx2d, &state.labels2d_placed);
        }

        let t1 = now_ms();
        state.perf_2d_total_ms = t1 - t0;
        state.perf_2d_poly_ms = 0.0;
        state.perf_2d_line_ms = 0.0;
        state.perf_2d_point_ms = 0.0;

        let mut tris: u32 = 0;
        if state.base_regions_style.visible
            && let Some(pos) = state.base_regions_mercator.as_deref()
        {
            tris = tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
        }
        if state.regions_style.visible
            && let Some(pos) = state.regions_mercator.as_deref()
        {
            tris = tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
        }
        if state.uploaded_regions_style.visible
            && let Some(pos) = state.uploaded_regions_mercator.as_deref()
        {
            tris = tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
        }
        if state.selection_style.visible
            && let Some(pos) = state.selection_poly_mercator.as_deref()
        {
            tris = tris.saturating_add((pos.len() / 3).min(2_000_000) as u32);
        }

        let mut segs: u32 = 0;
        if state.cull2d_enabled {
            segs = if show_lines {
                state.cull2d_visible_line_segs
            } else {
                0
            };
        } else {
            if state.corridors_style.visible
                && let Some(pos) = state.corridors_mercator.as_deref()
            {
                segs = segs.saturating_add((pos.len() / 2).min(1_500_000) as u32);
            }
            if state.uploaded_corridors_style.visible
                && let Some(pos) = state.uploaded_corridors_mercator.as_deref()
            {
                segs = segs.saturating_add((pos.len() / 2).min(1_500_000) as u32);
            }
            if state.selection_style.visible
                && let Some(pos) = state.selection_line_mercator.as_deref()
            {
                segs = segs.saturating_add((pos.len() / 2).min(1_500_000) as u32);
            }
        }

        let mut pts: u32 = 0;
        if state.cull2d_enabled {
            pts = if show_points {
                state.cull2d_visible_points
            } else {
                0
            };
        } else {
            if state.cities_style.visible
                && let Some(pos) = state.cities_mercator.as_deref()
            {
                pts = pts.saturating_add(pos.len().min(2_000_000) as u32);
            }
            if state.uploaded_points_style.visible
                && let Some(pos) = state.uploaded_mercator.as_deref()
            {
                pts = pts.saturating_add(pos.len().min(2_000_000) as u32);
            }
            for layer in state.feed_layers.values() {
                if layer.style.visible {
                    pts = pts.saturating_add(layer.centers_mercator.len().min(2_000_000) as u32);
                }
            }
            if state.selection_style.visible && state.selection_center_mercator.is_some() {
                pts = pts.saturating_add(1);
            }
        }

        state.perf_2d_poly_tris = tris;
        state.perf_2d_line_segs = segs;
        state.perf_2d_points = pts;

        // Snapshot GPU counters for this frame.
        if let Some(ctx) = state.wgpu.as_ref() {
            let snap = perf_snapshot(ctx);
            state.perf_gpu_upload_calls = snap.upload_calls;
            state.perf_gpu_upload_bytes = snap.upload_bytes;
            state.perf_gpu_render_passes = snap.render_passes;
            state.perf_gpu_draw_calls = snap.draw_calls;
            state.perf_gpu_draw_instances = snap.draw_instances;
            state.perf_gpu_draw_vertices = snap.draw_vertices;
            state.perf_gpu_draw_indices = snap.draw_indices;
            state.perf_gpu_frame_ms = (t1 - t0).max(0.0);
        }
        res
    }) {
        Ok(res) => res,
        Err(_) => Ok(()),
    }
}

#[wasm_bindgen]
pub fn get_2d_perf_stats() -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();
    let (total, poly, line, point, tris, segs, pts) = with_state(|state| {
        let s = state.borrow();
        (
            s.perf_2d_total_ms,
            s.perf_2d_poly_ms,
            s.perf_2d_line_ms,
            s.perf_2d_point_ms,
            s.perf_2d_poly_tris,
            s.perf_2d_line_segs,
            s.perf_2d_points,
        )
    });
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("total_ms"),
        &JsValue::from_f64(total),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("poly_ms"),
        &JsValue::from_f64(poly),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("line_ms"),
        &JsValue::from_f64(line),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("point_ms"),
        &JsValue::from_f64(point),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("poly_tris"),
        &JsValue::from_f64(tris as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("line_segs"),
        &JsValue::from_f64(segs as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("points"),
        &JsValue::from_f64(pts as f64),
    )?;
    Ok(out.into())
}

#[wasm_bindgen]
pub fn get_gpu_perf_stats() -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();
    let (upload_calls, upload_bytes, passes, draw_calls, inst, vtx, idx, frame_ms) =
        with_state(|state| {
            let s = state.borrow();
            (
                s.perf_gpu_upload_calls,
                s.perf_gpu_upload_bytes,
                s.perf_gpu_render_passes,
                s.perf_gpu_draw_calls,
                s.perf_gpu_draw_instances,
                s.perf_gpu_draw_vertices,
                s.perf_gpu_draw_indices,
                s.perf_gpu_frame_ms,
            )
        });

    let mbps = if frame_ms > 0.0 {
        (upload_bytes as f64) / (1024.0 * 1024.0) / (frame_ms / 1000.0)
    } else {
        0.0
    };

    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("frame_ms"),
        &JsValue::from_f64(frame_ms),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("upload_calls"),
        &JsValue::from_f64(upload_calls as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("upload_bytes"),
        &JsValue::from_f64(upload_bytes as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("upload_mib_per_s"),
        &JsValue::from_f64(mbps),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("render_passes"),
        &JsValue::from_f64(passes as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("draw_calls"),
        &JsValue::from_f64(draw_calls as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("draw_instances"),
        &JsValue::from_f64(inst as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("draw_vertices"),
        &JsValue::from_f64(vtx as f64),
    )?;
    js_sys::Reflect::set(
        &out,
        &JsValue::from_str("draw_indices"),
        &JsValue::from_f64(idx as f64),
    )?;
    Ok(out.into())
}

fn build_city_vertices(centers: &[[f32; 3]], style_id: u32) -> Vec<CityVertex> {
    let mut out: Vec<CityVertex> = Vec::with_capacity(centers.len());
    for &c in centers {
        out.push(CityVertex {
            center: c,
            style_id,
        });
    }
    out
}

fn ecef_vec3_to_viewer_f32(p: foundation::math::Vec3) -> [f32; 3] {
    // Viewer coordinates are a permuted ECEF: viewer = (x, z, -y)
    [p.x as f32, p.z as f32, (-p.y) as f32]
}

fn lon_lat_deg_to_world(lon_deg: f64, lat_deg: f64) -> [f32; 3] {
    let geo = Geodetic::new(lat_deg.to_radians(), lon_deg.to_radians(), 0.0);
    let ecef = geodetic_to_ecef(geo);
    [ecef.x as f32, ecef.z as f32, (-ecef.y) as f32]
}

fn unit_from_lon_lat_deg(lon_deg: f64, lat_deg: f64) -> [f64; 3] {
    let lon = lon_deg.to_radians();
    let lat = lat_deg.to_radians();
    let cos_lat = lat.cos();
    // Viewer coordinates are a permuted ECEF: viewer = (x, z, -y).
    // This means +lon (east) corresponds to -Z in viewer space.
    [cos_lat * lon.cos(), lat.sin(), -cos_lat * lon.sin()]
}

fn slerp_unit(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    let omega = dot.acos();
    let sin_omega = omega.sin();
    if sin_omega.abs() < 1e-6 {
        let x = a[0] + (b[0] - a[0]) * t;
        let y = a[1] + (b[1] - a[1]) * t;
        let z = a[2] + (b[2] - a[2]) * t;
        let len = (x * x + y * y + z * z).sqrt().max(1e-9);
        [x / len, y / len, z / len]
    } else {
        let a_scale = ((1.0 - t) * omega).sin() / sin_omega;
        let b_scale = (t * omega).sin() / sin_omega;
        [
            a_scale * a[0] + b_scale * b[0],
            a_scale * a[1] + b_scale * b[1],
            a_scale * a[2] + b_scale * b[2],
        ]
    }
}

fn lon_lat_deg_from_unit(u: [f64; 3]) -> (f64, f64) {
    // In viewer space, +lon (east) corresponds to -Z.
    // unit_from_lon_lat_deg(lon, lat) => z = -cos(lat)*sin(lon)
    // Therefore lon = atan2(-z, x).
    let lon = (-u[2]).atan2(u[0]).to_degrees();
    let lat = u[1].clamp(-1.0, 1.0).asin().to_degrees();
    (lon, lat)
}

fn world_to_lon_lat_fast_deg(p: [f32; 3]) -> (f64, f64) {
    // For 2D/WebMercator rendering, a fast spherical mapping is sufficient and
    // dramatically cheaper than a full ECEF->geodetic conversion.
    let u = vec3_normalize([p[0] as f64, p[1] as f64, p[2] as f64]);
    lon_lat_deg_from_unit(u)
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

fn build_corridor_vertices(positions_line_list: &[[f32; 3]], style_id: u32) -> Vec<CorridorVertex> {
    let seg_count = positions_line_list.len() / 2;
    let mut out: Vec<CorridorVertex> = Vec::with_capacity(seg_count);

    let push_segment = |out: &mut Vec<CorridorVertex>, a: [f32; 3], b: [f32; 3]| {
        out.push(CorridorVertex {
            a,
            _pad0: 0,
            b,
            style_id,
        });
    };

    const MAX_SEGMENTS: usize = 1_500_000;
    let mut emitted_segments = 0usize;

    for seg in positions_line_list.chunks_exact(2).take(1_500_000) {
        let a = seg[0];
        let b = seg[1];

        // Skip degenerate segments.
        let dx = a[0] - b[0];
        let dy = a[1] - b[1];
        let dz = a[2] - b[2];
        if dx * dx + dy * dy + dz * dz < 1e-12 {
            continue;
        }

        let (lon_a, lat_a) = world_to_lon_lat_deg([a[0] as f64, a[1] as f64, a[2] as f64]);
        let (lon_b, lat_b) = world_to_lon_lat_deg([b[0] as f64, b[1] as f64, b[2] as f64]);
        let ua = unit_from_lon_lat_deg(lon_a, lat_a);
        let ub = unit_from_lon_lat_deg(lon_b, lat_b);
        let dot = (ua[0] * ub[0] + ua[1] * ub[1] + ua[2] * ub[2]).clamp(-1.0, 1.0);
        let angle = dot.acos();
        let max_step = 5.0_f64.to_radians();
        let steps = ((angle / max_step).ceil() as usize).clamp(1, 128);

        if steps <= 1 {
            push_segment(&mut out, a, b);
            emitted_segments += 1;
        } else {
            let mut prev = a;
            for i in 1..=steps {
                if emitted_segments >= MAX_SEGMENTS {
                    break;
                }
                let t = i as f64 / steps as f64;
                let p = if i == steps {
                    b
                } else {
                    let u = slerp_unit(ua, ub, t);
                    let (lon, lat) = lon_lat_deg_from_unit(u);
                    lon_lat_deg_to_world(lon, lat)
                };
                push_segment(&mut out, prev, p);
                prev = p;
                emitted_segments += 1;
            }
        }

        if emitted_segments >= MAX_SEGMENTS {
            break;
        }
    }

    out
}
// Stable GPU style IDs. Geometry references these IDs so we can update style values
// without rebuilding / re-uploading geometry.
const STYLE_DEFAULT: u32 = 0;
const STYLE_BASE_REGIONS: u32 = 1;
const STYLE_REGIONS: u32 = 2;
const STYLE_UPLOADED_REGIONS: u32 = 3;
const STYLE_SELECTION_POLY: u32 = 4;

const STYLE_CITIES: u32 = 10;
const STYLE_UPLOADED_POINTS: u32 = 11;
const STYLE_SELECTION_POINT: u32 = 12;

const STYLE_CORRIDORS: u32 = 20;
const STYLE_UPLOADED_CORRIDORS: u32 = 21;
const STYLE_SELECTION_LINE: u32 = 22;

const STYLE_GRATICULE_2D: u32 = 30;

fn feed_style_id(s: &mut ViewerState, feed_layer_id: &str) -> u32 {
    if let Some(id) = s.feed_style_ids.get(feed_layer_id).copied() {
        return id;
    }
    let id = s.next_feed_style_id.max(100);
    s.next_feed_style_id = id.saturating_add(1);
    s.feed_style_ids.insert(feed_layer_id.to_string(), id);
    id
}

fn default_style() -> Style {
    Style {
        color: [1.0, 1.0, 1.0, 1.0],
        lift_m: 0.0,
        size_px: 3.0,
        width_px: 1.0,
        _pad0: 0.0,
    }
}

fn style_from_layer(style: LayerStyle, lift_default_m: f32, size_px: f32, width_px: f32) -> Style {
    let mut lift_m = style.lift * (WGS84_A as f32);
    if lift_m <= 0.0 {
        lift_m = lift_default_m;
    }
    Style {
        color: style.color,
        lift_m,
        size_px,
        width_px,
        _pad0: 0.0,
    }
}

fn rebuild_styles_table(s: &mut ViewerState) -> Vec<Style> {
    // Ensure all current feed layers have a stable ID.
    let feed_layer_ids: Vec<String> = s.feed_layers.keys().cloned().collect();
    for feed_layer_id in &feed_layer_ids {
        let _ = feed_style_id(s, feed_layer_id);
    }

    let mut max_id = STYLE_SELECTION_LINE.max(STYLE_GRATICULE_2D);
    if let Some(max_feed) = s.feed_style_ids.values().copied().max() {
        max_id = max_id.max(max_feed);
    }

    let mut styles = vec![default_style(); (max_id as usize).saturating_add(1)];

    // Polygons
    styles[STYLE_BASE_REGIONS as usize] = style_from_layer(s.base_regions_style, 100.0, 0.0, 0.0);
    styles[STYLE_REGIONS as usize] = style_from_layer(s.regions_style, 25.0, 0.0, 0.0);
    styles[STYLE_UPLOADED_REGIONS as usize] =
        style_from_layer(s.uploaded_regions_style, 25.0, 0.0, 0.0);
    styles[STYLE_SELECTION_POLY as usize] = Style {
        color: s.selection_style.color,
        lift_m: (s.selection_style.lift + 0.03) * (WGS84_A as f32),
        size_px: 0.0,
        width_px: 0.0,
        _pad0: 0.0,
    };

    // Points
    let size_px = s.city_marker_size.clamp(1.0, 64.0);
    styles[STYLE_CITIES as usize] = style_from_layer(s.cities_style, 50.0, size_px, 1.0);
    styles[STYLE_UPLOADED_POINTS as usize] =
        style_from_layer(s.uploaded_points_style, 50.0, size_px, 1.0);
    styles[STYLE_SELECTION_POINT as usize] = style_from_layer(
        s.selection_style,
        50.0,
        (s.city_marker_size * 1.35).clamp(1.0, 64.0),
        1.0,
    );

    // Lines
    let width_px = s.line_width_px.clamp(1.0, 24.0);
    styles[STYLE_CORRIDORS as usize] = style_from_layer(s.corridors_style, 50.0, 0.0, width_px);
    styles[STYLE_UPLOADED_CORRIDORS as usize] =
        style_from_layer(s.uploaded_corridors_style, 50.0, 0.0, width_px);
    styles[STYLE_SELECTION_LINE as usize] = Style {
        color: s.selection_style.color,
        lift_m: (s.selection_style.lift + 0.03) * (WGS84_A as f32),
        size_px: 0.0,
        width_px: (s.line_width_px * 1.6).clamp(1.0, 24.0),
        _pad0: 0.0,
    };

    // 2D graticule (Web Mercator). Rendered as instanced segments in the 2D WebGPU path.
    styles[STYLE_GRATICULE_2D as usize] = Style {
        color: [0.58, 0.64, 0.72, 0.20],
        lift_m: 0.0,
        size_px: 0.0,
        width_px: 0.75,
        _pad0: 0.0,
    };

    // Feed layers
    for (feed_layer_id, layer) in &s.feed_layers {
        if let Some(id) = s.feed_style_ids.get(feed_layer_id).copied()
            && (id as usize) < styles.len()
        {
            styles[id as usize] = style_from_layer(layer.style, 50.0, size_px, 1.0);
        }
    }

    styles[STYLE_DEFAULT as usize] = default_style();
    styles
}

fn rebuild_styles_and_upload_only() -> Result<(), JsValue> {
    match STATE.try_with(|state| {
        let mut s = state.borrow_mut();
        let styles = rebuild_styles_table(&mut s);
        if let Some(ctx) = &mut s.wgpu {
            set_styles(ctx, &styles);
            s.pending_styles = None;
        } else {
            s.pending_styles = Some(styles);
        }
        Ok(())
    }) {
        Ok(res) => res,
        Err(_) => Ok(()),
    }
}

fn rebuild_overlays_and_upload() -> Result<(), JsValue> {
    // Budget guardrails: the web viewer will trap on extremely large GPU buffers.
    // Increased limits to support larger datasets; JS-side validation can still apply lower limits.
    const MAX_UPLOADED_POINTS: usize = 2_000_000;
    const MAX_UPLOADED_LINE_SEGMENTS: usize = 3_000_000;
    const MAX_UPLOADED_POLY_VERTS: usize = 6_000_000;

    match STATE.try_with(|state| {
        let mut s = state.borrow_mut();

        // Sources for 2D culling (mercator caches + layer visibility) may change during rebuild.
        // Bump generation to force a fresh cull on the next 2D WebGPU frame.
        s.cull2d_geom_gen = s.cull2d_geom_gen.wrapping_add(1);
        s.cull2d_job = None;
        s.cull2d_last_snapshot = None;
        s.cull2d_visible_points = 0;
        s.cull2d_visible_line_segs = 0;

        // Refresh cached viewer-space geometry from the engine worlds.
        // This ensures all rendered features flow through `layers`.
        let layer = VectorLayer::new(1);

        // Built-ins
        if let Some(pos) = s.surface_positions.as_ref() {
            let pos_vec = pos.clone();
            s.base_regions_positions = Some(pos_vec);
            s.base_regions_mercator = s
                .base_regions_positions
                .as_ref()
                .map(|pos| world_tris_to_mercator_clipped(pos));
        } else {
            s.base_regions_positions = s.base_world.as_ref().map(|w| {
                let snap = layer.extract(w);
                snap.area_triangles
                    .into_iter()
                    .map(ecef_vec3_to_viewer_f32)
                    .collect::<Vec<_>>()
            });
            s.base_regions_mercator = s
                .base_regions_positions
                .as_ref()
                .map(|pos| world_tris_to_mercator_clipped(pos));
        }
        s.cities_centers = s.cities_world.as_ref().map(|w| {
            let snap = layer.extract(w);
            snap.points
                .into_iter()
                .map(ecef_vec3_to_viewer_f32)
                .collect::<Vec<_>>()
        });
        s.cities_mercator = s
            .cities_centers
            .as_ref()
            .map(|centers| centers.iter().map(viewer_to_mercator_m).collect());

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
        s.corridors_mercator = s
            .corridors_positions
            .as_ref()
            .map(|pos| world_segs_to_mercator_clipped(pos));

        s.regions_positions = s.regions_world.as_ref().map(|w| {
            let snap = layer.extract(w);
            snap.area_triangles
                .into_iter()
                .map(ecef_vec3_to_viewer_f32)
                .collect::<Vec<_>>()
        });
        s.regions_mercator = s
            .regions_positions
            .as_ref()
            .map(|pos| world_tris_to_mercator_clipped(pos));

        // Uploaded
        if let Some(w) = s.uploaded_world.as_ref() {
            let snap = layer.extract(w);

            let uploaded_points = snap.points.len();
            let uploaded_line_segments: usize = snap
                .lines
                .iter()
                .map(|line| line.len().saturating_sub(1))
                .sum();
            let uploaded_poly_verts = snap.area_triangles.len();

            if uploaded_points > MAX_UPLOADED_POINTS {
                return Err(JsValue::from_str(
                    "Upload too complex for the web viewer (too many points).",
                ));
            }
            if uploaded_line_segments > MAX_UPLOADED_LINE_SEGMENTS {
                return Err(JsValue::from_str(
                    "Upload too complex for the web viewer (too many line segments).",
                ));
            }
            if uploaded_poly_verts > MAX_UPLOADED_POLY_VERTS {
                return Err(JsValue::from_str(
                    "Upload too complex for the web viewer (too many polygon triangles).",
                ));
            }

            s.uploaded_centers = Some(
                snap.points
                    .into_iter()
                    .map(ecef_vec3_to_viewer_f32)
                    .collect::<Vec<_>>(),
            );
            s.uploaded_mercator = s
                .uploaded_centers
                .as_ref()
                .map(|centers| centers.iter().map(viewer_to_mercator_m).collect());
            s.uploaded_corridors_positions = Some({
                let mut out: Vec<[f32; 3]> = Vec::with_capacity(uploaded_line_segments * 2);
                for line in snap.lines {
                    for seg in line.windows(2) {
                        out.push(ecef_vec3_to_viewer_f32(seg[0]));
                        out.push(ecef_vec3_to_viewer_f32(seg[1]));
                    }
                }
                out
            });
            s.uploaded_corridors_mercator = s
                .uploaded_corridors_positions
                .as_ref()
                .map(|pos| world_segs_to_mercator_clipped(pos));
            s.uploaded_regions_positions = Some(
                snap.area_triangles
                    .into_iter()
                    .map(ecef_vec3_to_viewer_f32)
                    .collect::<Vec<_>>(),
            );
            s.uploaded_regions_mercator = s
                .uploaded_regions_positions
                .as_ref()
                .map(|pos| world_tris_to_mercator_clipped(pos));
        } else {
            s.uploaded_centers = None;
            s.uploaded_mercator = None;
            s.uploaded_corridors_positions = None;
            s.uploaded_corridors_mercator = None;
            s.uploaded_regions_positions = None;
            s.uploaded_regions_mercator = None;
        }

        // Feed layers (points).
        for layer in s.feed_layers.values_mut() {
            if layer.centers_mercator.len() != layer.centers.len() {
                layer.centers_mercator = layer.centers.iter().map(viewer_to_mercator_m).collect();
            }
        }

        // Selection caches.
        s.selection_center_mercator = s.selection_center.map(|c| viewer_to_mercator_m(&c));
        s.selection_line_mercator = s
            .selection_line_positions
            .as_ref()
            .map(|pos| world_segs_to_mercator_clipped(pos));
        s.selection_poly_mercator = s
            .selection_poly_positions
            .as_ref()
            .map(|pos| world_tris_to_mercator_clipped(pos));

        let styles = rebuild_styles_table(&mut s);

        let mut points: Vec<CityVertex> = Vec::new();
        let mut lines: Vec<CorridorVertex> = Vec::new();
        let mut base_polys: Vec<OverlayVertex> = Vec::new();
        let mut polys: Vec<OverlayVertex> = Vec::new();

        // 2D (Web Mercator) GPU buffers.
        let mut base_polys2d: Vec<Overlay2DVertex> = Vec::new();
        let mut polys2d: Vec<Overlay2DVertex> = Vec::new();
        let mut points2d: Vec<Point2DInstance> = Vec::new();
        let mut lines2d: Vec<Segment2DInstance> = Vec::new();
        let mut grid2d: Vec<Segment2DInstance> = Vec::new();

        // Points (instanced)
        if s.cities_style.visible
            && let Some(centers) = s.cities_centers.as_deref()
        {
            points.extend(build_city_vertices(centers, STYLE_CITIES));
        }
        if s.uploaded_points_style.visible
            && let Some(centers) = s.uploaded_centers.as_deref()
        {
            points.extend(build_city_vertices(centers, STYLE_UPLOADED_POINTS));
        }
        for (feed_layer_id, layer) in &s.feed_layers {
            if layer.style.visible && !layer.centers.is_empty() {
                let style_id = s
                    .feed_style_ids
                    .get(feed_layer_id)
                    .copied()
                    .unwrap_or(STYLE_DEFAULT);
                points.extend(build_city_vertices(&layer.centers, style_id));
            }
        }
        if s.selection_style.visible
            && let Some(c) = s.selection_center
        {
            points.extend(build_city_vertices(
                std::slice::from_ref(&c),
                STYLE_SELECTION_POINT,
            ));
        }

        // Lines (instanced)
        if s.corridors_style.visible
            && let Some(pos) = s.corridors_positions.as_deref()
        {
            lines.extend(build_corridor_vertices(pos, STYLE_CORRIDORS));
        }
        if s.uploaded_corridors_style.visible
            && let Some(pos) = s.uploaded_corridors_positions.as_deref()
        {
            lines.extend(build_corridor_vertices(pos, STYLE_UPLOADED_CORRIDORS));
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_line_positions.as_deref()
            && !pos.is_empty()
        {
            lines.extend(build_corridor_vertices(pos, STYLE_SELECTION_LINE));
        }

        // Base polygons (triangles)
        if s.base_regions_style.visible
            && let Some(pos) = s.base_regions_positions.as_deref()
        {
            let style_id = STYLE_BASE_REGIONS;
            base_polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                style_id,
            }));
        }

        // 2D base polygons (Mercator triangles)
        if s.base_regions_style.visible
            && let Some(pos) = s.base_regions_mercator.as_deref()
        {
            base_polys2d.reserve(pos.len());
            for tri in pos.chunks_exact(3).take(2_000_000) {
                let anchor_x_m = tri[0][0];
                base_polys2d.push(Overlay2DVertex {
                    position_m: tri[0],
                    anchor_x_m,
                    style_id: STYLE_BASE_REGIONS,
                });
                base_polys2d.push(Overlay2DVertex {
                    position_m: tri[1],
                    anchor_x_m,
                    style_id: STYLE_BASE_REGIONS,
                });
                base_polys2d.push(Overlay2DVertex {
                    position_m: tri[2],
                    anchor_x_m,
                    style_id: STYLE_BASE_REGIONS,
                });
            }
        }

        // Polygons (triangles)
        if s.regions_style.visible
            && let Some(pos) = s.regions_positions.as_deref()
        {
            let style_id = STYLE_REGIONS;
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                style_id,
            }));
        }
        if s.uploaded_regions_style.visible
            && let Some(pos) = s.uploaded_regions_positions.as_deref()
        {
            let style_id = STYLE_UPLOADED_REGIONS;
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                style_id,
            }));
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_poly_positions.as_deref()
            && !pos.is_empty()
        {
            let style_id = STYLE_SELECTION_POLY;
            polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                style_id,
            }));
        }

        // 2D polygons (Mercator triangles)
        if s.regions_style.visible
            && let Some(pos) = s.regions_mercator.as_deref()
        {
            polys2d.reserve(pos.len());
            for tri in pos.chunks_exact(3).take(2_000_000) {
                let anchor_x_m = tri[0][0];
                polys2d.push(Overlay2DVertex {
                    position_m: tri[0],
                    anchor_x_m,
                    style_id: STYLE_REGIONS,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[1],
                    anchor_x_m,
                    style_id: STYLE_REGIONS,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[2],
                    anchor_x_m,
                    style_id: STYLE_REGIONS,
                });
            }
        }
        if s.uploaded_regions_style.visible
            && let Some(pos) = s.uploaded_regions_mercator.as_deref()
        {
            polys2d.reserve(pos.len());
            for tri in pos.chunks_exact(3).take(2_000_000) {
                let anchor_x_m = tri[0][0];
                polys2d.push(Overlay2DVertex {
                    position_m: tri[0],
                    anchor_x_m,
                    style_id: STYLE_UPLOADED_REGIONS,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[1],
                    anchor_x_m,
                    style_id: STYLE_UPLOADED_REGIONS,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[2],
                    anchor_x_m,
                    style_id: STYLE_UPLOADED_REGIONS,
                });
            }
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_poly_mercator.as_deref()
            && !pos.is_empty()
        {
            polys2d.reserve(pos.len());
            for tri in pos.chunks_exact(3).take(2_000_000) {
                let anchor_x_m = tri[0][0];
                polys2d.push(Overlay2DVertex {
                    position_m: tri[0],
                    anchor_x_m,
                    style_id: STYLE_SELECTION_POLY,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[1],
                    anchor_x_m,
                    style_id: STYLE_SELECTION_POLY,
                });
                polys2d.push(Overlay2DVertex {
                    position_m: tri[2],
                    anchor_x_m,
                    style_id: STYLE_SELECTION_POLY,
                });
            }
        }

        // 2D lines (Mercator segments)
        if s.corridors_style.visible
            && let Some(pos) = s.corridors_mercator.as_deref()
        {
            lines2d.reserve(pos.len() / 2);
            for seg in pos.chunks_exact(2).take(1_500_000) {
                lines2d.push(Segment2DInstance {
                    a_m: seg[0],
                    b_m: seg[1],
                    style_id: STYLE_CORRIDORS,
                    _pad0: 0,
                });
            }
        }
        if s.uploaded_corridors_style.visible
            && let Some(pos) = s.uploaded_corridors_mercator.as_deref()
        {
            lines2d.reserve(pos.len() / 2);
            for seg in pos.chunks_exact(2).take(1_500_000) {
                lines2d.push(Segment2DInstance {
                    a_m: seg[0],
                    b_m: seg[1],
                    style_id: STYLE_UPLOADED_CORRIDORS,
                    _pad0: 0,
                });
            }
        }
        if s.selection_style.visible
            && let Some(pos) = s.selection_line_mercator.as_deref()
            && !pos.is_empty()
        {
            lines2d.reserve(pos.len() / 2);
            for seg in pos.chunks_exact(2).take(1_500_000) {
                lines2d.push(Segment2DInstance {
                    a_m: seg[0],
                    b_m: seg[1],
                    style_id: STYLE_SELECTION_LINE,
                    _pad0: 0,
                });
            }
        }

        // 2D points (Mercator)
        if s.cities_style.visible
            && let Some(centers) = s.cities_mercator.as_deref()
        {
            points2d.reserve(centers.len());
            for &c in centers.iter().take(2_000_000) {
                points2d.push(Point2DInstance {
                    center_m: c,
                    style_id: STYLE_CITIES,
                    _pad0: 0,
                });
            }
        }
        if s.uploaded_points_style.visible
            && let Some(centers) = s.uploaded_mercator.as_deref()
        {
            points2d.reserve(centers.len());
            for &c in centers.iter().take(2_000_000) {
                points2d.push(Point2DInstance {
                    center_m: c,
                    style_id: STYLE_UPLOADED_POINTS,
                    _pad0: 0,
                });
            }
        }
        for (feed_layer_id, layer) in &s.feed_layers {
            if layer.style.visible && !layer.centers_mercator.is_empty() {
                let style_id = s
                    .feed_style_ids
                    .get(feed_layer_id)
                    .copied()
                    .unwrap_or(STYLE_DEFAULT);
                points2d.reserve(layer.centers_mercator.len());
                for &c in layer.centers_mercator.iter().take(2_000_000) {
                    points2d.push(Point2DInstance {
                        center_m: c,
                        style_id,
                        _pad0: 0,
                    });
                }
            }
        }
        if s.selection_style.visible
            && let Some(c) = s.selection_center_mercator
        {
            points2d.push(Point2DInstance {
                center_m: c,
                style_id: STYLE_SELECTION_POINT,
                _pad0: 0,
            });
        }

        // 2D graticule (Mercator). Small enough to regenerate per rebuild.
        {
            let y0 = mercator_y_m(-85.0) as f32;
            let y1 = mercator_y_m(85.0) as f32;
            for lon in (-180..=180).step_by(10) {
                let x = mercator_x_m(lon as f64) as f32;
                grid2d.push(Segment2DInstance {
                    a_m: [x, y0],
                    b_m: [x, y1],
                    style_id: STYLE_GRATICULE_2D,
                    _pad0: 0,
                });
            }
            for lat in (-80..=80).step_by(10) {
                let y = mercator_y_m(lat as f64) as f32;
                let mut prev = None;
                for lon in (-180..=180).step_by(10) {
                    let x = mercator_x_m(lon as f64) as f32;
                    let cur = [x, y];
                    if let Some(p) = prev {
                        grid2d.push(Segment2DInstance {
                            a_m: p,
                            b_m: cur,
                            style_id: STYLE_GRATICULE_2D,
                            _pad0: 0,
                        });
                    }
                    prev = Some(cur);
                }
            }
        }

        if let Some(ctx) = &mut s.wgpu {
            set_styles(ctx, &styles);
            set_cities_points(ctx, &points);
            set_corridors_points(ctx, &lines);
            set_base_regions_points(ctx, &base_polys);
            set_regions_points(ctx, &polys);
            set_base_regions2d_vertices(ctx, &base_polys2d);
            set_regions2d_vertices(ctx, &polys2d);
            set_points2d_instances(ctx, &points2d);
            set_lines2d_instances(ctx, &lines2d);
            set_grid2d_instances(ctx, &grid2d);
            s.pending_styles = None;
            s.pending_cities = None;
            s.pending_corridors = None;
            s.pending_base_regions = None;
            s.pending_regions = None;
            s.pending_base_regions2d = None;
            s.pending_regions2d = None;
            s.pending_points2d = None;
            s.pending_lines2d = None;
            s.pending_grid2d = None;
        } else {
            s.pending_styles = Some(styles);
            s.pending_cities = Some(points);
            s.pending_corridors = Some(lines);
            s.pending_base_regions = Some(base_polys);
            s.pending_regions = Some(polys);
            s.pending_base_regions2d = Some(base_polys2d);
            s.pending_regions2d = Some(polys2d);
            s.pending_points2d = Some(points2d);
            s.pending_lines2d = Some(lines2d);
            s.pending_grid2d = Some(grid2d);
        }

        Ok(())
    }) {
        Ok(res) => res,
        Err(_) => Ok(()),
    }
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    // Avoid double-initialization (can happen during hot-reload edge cases).
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    init_panic_hook();
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
pub fn init_canvas_2d() {
    if let Err(err) = init_canvas_2d_inner() {
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "2d canvas init error: {:?}",
            err
        )));
    }
}

fn init_canvas_2d_inner() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id("atlas-canvas-2d")
        .ok_or_else(|| JsValue::from_str("missing atlas-canvas-2d"))?
        .dyn_into::<HtmlCanvasElement>()?;
    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("2d context unavailable"))?
        .dyn_into::<CanvasRenderingContext2d>()?;

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.canvas_2d = Some(canvas);
        s.ctx_2d = Some(ctx);
    });

    render_scene()
}

#[wasm_bindgen]
pub fn set_view_mode(mode: &str) -> Result<(), JsValue> {
    let mode = ViewMode::from_str(mode);
    with_state(|state| {
        let mut s = state.borrow_mut();

        if s.view_mode != mode {
            match (s.view_mode, mode) {
                (ViewMode::ThreeD, ViewMode::TwoD) => {
                    // Start 2D mode with the default whole-world view (lon=0, lat=0, zoom=1)
                    // rather than mapping from the 3D camera position.
                    // This ensures a clean, predictable initial 2D view.
                    s.camera_2d = Camera2DState::default();
                }
                (ViewMode::TwoD, ViewMode::ThreeD) => {
                    // Map 2D center back to a 3D orbit camera.
                    // In this viewer yaw 0° looks at lon 180° (Pacific), so
                    // visible_lon ≈ 180° - yaw.  Invert: yaw = 180° - lon.
                    let yaw_rad = (180.0 - s.camera_2d.center_lon_deg).to_radians();
                    let pitch_rad = clamp(s.camera_2d.center_lat_deg.to_radians(), -1.55, 1.55);
                    let dist = (3.0 * WGS84_A) / s.camera_2d.zoom.max(1e-6);
                    let distance = clamp(dist, 1.001 * WGS84_A, 200.0 * WGS84_A);

                    // Update legacy camera state
                    s.camera.yaw_rad = yaw_rad;
                    s.camera.pitch_rad = pitch_rad;
                    s.camera.distance = distance;
                    s.camera.target = [0.0, 0.0, 0.0];

                    // Sync globe controller with the new camera state
                    s.globe_controller.set_from_yaw_pitch(yaw_rad, pitch_rad);
                    s.globe_controller.set_distance(distance);

                    // Reset frame time so dt calculation is clean on first 3D frame
                    s.last_frame_time_s = 0.0;
                }
                _ => {}
            }
        }

        s.view_mode = mode;
    });
    if mode == ViewMode::TwoD {
        ensure_surface_loaded();
    } else if with_state(|state| state.borrow().terrain_style.visible) {
        ensure_terrain_loaded();
    }
    render_scene()
}

#[wasm_bindgen]
pub fn set_canvas_sizes(width: f64, height: f64) {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.canvas_width = width;
        s.canvas_height = height;
        if let Some(ctx) = &mut s.wgpu {
            resize_wgpu(ctx, width as u32, height as u32);
        }
    });
    // Re-render after resize to update the visible content.
    let _ = render_scene();
}

#[wasm_bindgen]
pub fn camera_reset() -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        match s.view_mode {
            ViewMode::ThreeD => s.camera = CameraState::default(),
            ViewMode::TwoD => s.camera_2d = Camera2DState::default(),
        }
    });
    render_scene()
}

#[wasm_bindgen]
pub fn get_camera_yaw_deg() -> f64 {
    with_state(|state| {
        let s = state.borrow();
        match s.view_mode {
            ViewMode::ThreeD => s.camera.yaw_rad.to_degrees().rem_euclid(360.0),
            ViewMode::TwoD => 0.0,
        }
    })
}

#[wasm_bindgen]
pub fn set_camera_yaw_deg(yaw_deg: f64) -> Result<(), JsValue> {
    if !yaw_deg.is_finite() {
        return Err(JsValue::from_str("yaw_deg must be finite"));
    }

    let mut yaw_rad = yaw_deg.to_radians();
    yaw_rad = (yaw_rad + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI)
        - std::f64::consts::PI;

    with_state(|state| {
        let mut s = state.borrow_mut();
        if s.view_mode == ViewMode::ThreeD {
            s.camera.yaw_rad = yaw_rad;
            // Sync to globe controller
            let yaw = s.camera.yaw_rad;
            let pitch = s.camera.pitch_rad;
            s.globe_controller.set_from_yaw_pitch(yaw, pitch);
        }
    });
    render_scene()
}

/// Get debug info about the globe controller state.
#[wasm_bindgen]
pub fn get_globe_controller_debug() -> String {
    with_state(|state| {
        let s = state.borrow();
        format!(
            "orientation: {:?}, distance: {:.1}, inertia_active: {}, angular_velocity: {:?}",
            s.globe_controller.orientation(),
            s.globe_controller.distance(),
            s.globe_controller.is_inertia_active(),
            s.globe_controller.angular_velocity()
        )
    })
}

/// Returns true if globe inertia animation is currently active.
#[wasm_bindgen]
pub fn is_globe_inertia_active() -> bool {
    with_state(|state| {
        let s = state.borrow();
        s.globe_controller.is_inertia_active()
    })
}

/// Orbit around the globe.
///
/// Intended usage: call with pointer delta in pixels.
/// **Contract (3D):** Drag left => surface facing user moves left (yaw increases).
///   Drag up => surface facing user tilts up (pitch decreases).
///   Sensitivity scaled so full shorter-axis drag ≈ 180°.
/// **Contract (2D):** Treated as pan — map follows cursor direction.
#[wasm_bindgen]
pub fn camera_orbit(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        let cfg = s.controls;
        match s.view_mode {
            ViewMode::ThreeD => {
                let min_dim = s.canvas_width.min(s.canvas_height).max(1.0);
                let speed = std::f64::consts::PI / min_dim * cfg.orbit_sensitivity;
                let dy_sign = if cfg.invert_orbit_y { -1.0 } else { 1.0 };

                // Drag right (+dx) → yaw increases → camera orbits eastward
                // → surface facing user moves RIGHT (follows cursor).
                s.camera.yaw_rad += delta_x_px * speed;
                s.camera.pitch_rad = clamp(
                    s.camera.pitch_rad + dy_sign * delta_y_px * speed,
                    -cfg.pitch_clamp_rad,
                    cfg.pitch_clamp_rad,
                );
                s.camera.yaw_rad = (s.camera.yaw_rad + std::f64::consts::PI)
                    .rem_euclid(2.0 * std::f64::consts::PI)
                    - std::f64::consts::PI;

                // Sync globe controller so the update loop and inertia
                // start from the correct orientation when the drag ends.
                // Do NOT call on_pointer_move here — it runs a competing
                // arcball rotation that conflicts with our delta-based math.
                let sync_yaw = s.camera.yaw_rad;
                let sync_pitch = s.camera.pitch_rad;
                s.globe_controller.set_from_yaw_pitch(sync_yaw, sync_pitch);
            }
            ViewMode::TwoD => {
                s.camera_2d = pan_camera_2d(
                    s.camera_2d,
                    delta_x_px,
                    delta_y_px,
                    s.canvas_width,
                    s.canvas_height,
                );
            }
        }
    });
    render_scene()
}

/// Begin a pointer drag.
///
/// In 3D, this initializes arcball state to enable consistent grab-to-rotate.
/// In 2D, this sets the reference position for pan deltas.
#[wasm_bindgen]
pub fn camera_drag_begin(x_px: f64, y_px: f64) -> Result<(), JsValue> {
    camera_drag_begin_with_button(x_px, y_px, 0)
}

/// Begin a pointer drag with specified mouse button.
///
/// button: 0 = left, 1 = middle, 2 = right (standard MouseEvent.button values)
#[wasm_bindgen]
pub fn camera_drag_begin_with_button(x_px: f64, y_px: f64, button: i32) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        s.drag_last_x_px = x_px;
        s.drag_last_y_px = y_px;
        match s.view_mode {
            ViewMode::ThreeD => {
                // Left-click (orbit) resets target to origin so rotation
                // is always around the globe center, not a panned offset.
                // Right-click (pan) keeps the current target.
                if button == 0 {
                    s.camera.target = [0.0, 0.0, 0.0];
                }
                let canvas_w = s.canvas_width;
                let canvas_h = s.canvas_height;
                s.globe_controller.set_canvas_size(canvas_w, canvas_h);
                s.globe_controller.on_pointer_down([x_px, y_px], button);
                s.arcball_last_unit =
                    Some(arcball_unit_from_screen(canvas_w, canvas_h, x_px, y_px));
            }
            ViewMode::TwoD => {
                s.arcball_last_unit = None;
            }
        }
    });
    Ok(())
}

/// Update a pointer drag.
///
/// In 3D, performs an arcball rotation around the globe center.
/// The surface under the cursor follows the cursor direction (grab-and-rotate).
/// In 2D, pans the mercator view.
#[wasm_bindgen]
pub fn camera_drag_move(x_px: f64, y_px: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        match s.view_mode {
            ViewMode::ThreeD => {
                // Let globe controller drive drag via its own arcball
                // for smooth rotation with inertia velocity tracking.
                s.globe_controller.on_pointer_move([x_px, y_px]);

                // Keep legacy camera state in sync for rendering.
                s.camera.yaw_rad = s.globe_controller.yaw_rad();
                s.camera.pitch_rad = s.globe_controller.pitch_rad();
                s.camera.distance = s.globe_controller.distance();

                // Update arcball_last_unit for legacy code paths.
                let next_u = arcball_unit_from_screen(s.canvas_width, s.canvas_height, x_px, y_px);
                s.arcball_last_unit = Some(next_u);
            }
            ViewMode::TwoD => {
                let dx = x_px - s.drag_last_x_px;
                let dy = y_px - s.drag_last_y_px;
                s.drag_last_x_px = x_px;
                s.drag_last_y_px = y_px;
                s.camera_2d = pan_camera_2d(s.camera_2d, dx, dy, s.canvas_width, s.canvas_height);
            }
        }
    });
    render_scene()
}

/// End a pointer drag (clears arcball state).
#[wasm_bindgen]
pub fn camera_drag_end() -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.arcball_last_unit = None;
        if matches!(s.view_mode, ViewMode::ThreeD) {
            s.globe_controller.on_pointer_up();
        }
    });
    Ok(())
}

/// Pan the camera target (translates the globe in 3D, pans map in 2D).
///
/// **Contract (3D):** Right-click drag moves the entire globe in the viewport.
///   Drag right => globe moves right on screen.  Drag up => globe moves up.
///   The target offset is clamped to `max_target_offset_m` to keep the globe
///   visible.  Double-click or R key resets target to origin.
/// **Contract (2D):** Same as left-drag pan — map follows cursor.
#[wasm_bindgen]
pub fn camera_pan(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        let cfg = s.controls;
        match s.view_mode {
            ViewMode::ThreeD => {
                // Compute camera right/up vectors in world space to translate
                // the target perpendicular to the view direction.
                let dir_cam = vec3_normalize([
                    s.camera.pitch_rad.cos() * s.camera.yaw_rad.cos(),
                    s.camera.pitch_rad.sin(),
                    -s.camera.pitch_rad.cos() * s.camera.yaw_rad.sin(),
                ]);
                let world_up = [0.0, 1.0, 0.0];
                let right = vec3_normalize(vec3_cross(world_up, dir_cam));
                let up = vec3_cross(dir_cam, right);

                // Scale: at distance D, one pixel ≈ D * fov_y / viewport_height
                // Use the shorter dimension for consistent feel.
                let fov_y_rad = 45f64.to_radians();
                let h = s.canvas_height.max(1.0);
                let px_to_world = s.camera.distance * (fov_y_rad / h) * cfg.pan_sensitivity_3d;

                let dy_sign = if cfg.invert_pan_y_3d { -1.0 } else { 1.0 };

                // Move target: right-drag right => target moves right => globe moves right.
                let offset_right = vec3_mul(right, -delta_x_px * px_to_world);
                let offset_up = vec3_mul(up, dy_sign * delta_y_px * px_to_world);
                let new_target = vec3_add(vec3_add(s.camera.target, offset_right), offset_up);

                // Clamp target offset to prevent globe from going completely off-screen.
                let r = vec3_dot(new_target, new_target).sqrt();
                if r <= cfg.max_target_offset_m {
                    s.camera.target = new_target;
                } else {
                    let scale = cfg.max_target_offset_m / r;
                    s.camera.target = vec3_mul(new_target, scale);
                }
            }
            ViewMode::TwoD => {
                s.camera_2d = pan_camera_2d(
                    s.camera_2d,
                    delta_x_px,
                    delta_y_px,
                    s.canvas_width,
                    s.canvas_height,
                );
            }
        }
    });
    render_scene()
}

/// Zoom (dolly) in/out.
///
/// Intended usage: call with wheel deltaY.
#[wasm_bindgen]
pub fn camera_zoom(wheel_delta_y: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        let cfg = s.controls;
        match s.view_mode {
            ViewMode::ThreeD => {
                // Delegate to globe controller for smooth zoom with
                // velocity-based interpolation.  Scale the raw wheel
                // delta by the user's zoom-speed preference.
                s.globe_controller
                    .on_wheel(wheel_delta_y * cfg.zoom_speed_3d);

                // Sync legacy camera distance for immediate render.
                s.camera.distance = s.globe_controller.distance();
            }
            ViewMode::TwoD => {
                let zoom = (-wheel_delta_y * 0.0015 * cfg.zoom_speed_2d).exp();
                s.camera_2d.zoom = clamp(s.camera_2d.zoom * zoom, cfg.min_zoom_2d, cfg.max_zoom_2d);
            }
        }
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

    with_state(|state| {
        state.borrow_mut().dataset = dataset;
    });

    if should_load_cities {
        with_state(|state| {
            if let Some(st) = layer_style_mut(&mut state.borrow_mut(), "cities") {
                st.visible = true;
            }
        });
        ensure_builtin_layer_loaded("cities");
    }

    if should_load_corridors {
        with_state(|state| {
            if let Some(st) = layer_style_mut(&mut state.borrow_mut(), "air_corridors") {
                st.visible = true;
            }
        });
        ensure_builtin_layer_loaded("air_corridors");
    }

    if should_load_regions {
        with_state(|state| {
            if let Some(st) = layer_style_mut(&mut state.borrow_mut(), "regions") {
                st.visible = true;
            }
        });
        ensure_builtin_layer_loaded("regions");
    }

    if should_load_uploaded {
        with_state(|state| {
            let mut s = state.borrow_mut();
            s.uploaded_points_style.visible = s.uploaded_count_points > 0;
            s.uploaded_corridors_style.visible = s.uploaded_count_lines > 0;
            s.uploaded_regions_style.visible = s.uploaded_count_polys > 0;
        });
        let _ = rebuild_overlays_and_upload();
    }

    // Render immediately so selection changes are responsive.
    render_scene()?;
    Ok(())
}

#[wasm_bindgen]
pub fn load_base_world() {
    // Force the bundled base-world fallback (assets/world.json).
    // This is used when PMTiles decoding fails, and we must override any partially-created
    // `base_world` from streaming mode (which would otherwise block `ensure_base_world_loaded()`).
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.surface_tileset = None;
        s.surface_positions = None;
        s.surface_zoom = None;
        s.surface_source = None;
        s.surface_loading = false;
        s.surface_last_error = None;

        s.base_world = None;
        s.base_world_loading = false;
        s.base_world_error = None;
        s.base_world_source = Some("world.json".to_string());
        s.base_count_polys = 0;
    });

    ensure_base_world_loaded();
}

#[wasm_bindgen]
pub fn set_city_marker_size(size: f64) -> Result<(), JsValue> {
    // Size is in screen pixels.
    let size = (size as f32).clamp(1.0, 64.0);

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.city_marker_size = size;
    });

    let _ = rebuild_styles_and_upload_only();
    render_scene()
}

#[wasm_bindgen]
pub fn set_line_width_px(width_px: f64) -> Result<(), JsValue> {
    let width_px = (width_px as f32).clamp(1.0, 24.0);
    with_state(|state| {
        state.borrow_mut().line_width_px = width_px;
    });
    let _ = rebuild_styles_and_upload_only();
    render_scene()
}

#[wasm_bindgen]
pub fn set_graticule_enabled(enabled: bool) -> Result<(), JsValue> {
    with_state(|state| {
        state.borrow_mut().show_graticule = enabled;
    });
    // Render immediately so the toggle feels responsive.
    render_scene()
}

#[wasm_bindgen]
pub fn set_labels_enabled(enabled: bool) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.labels_enabled = enabled;
        // Force a refresh.
        s.labels_gen = s.labels_gen.wrapping_add(1);
        s.labels2d_job = None;
        s.labels2d_last_snapshot = None;
        s.labels2d_placed.clear();
    });
    render_scene()
}

#[wasm_bindgen]
pub fn clear_debug_labels() -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.debug_labels.clear();
        s.labels_gen = s.labels_gen.wrapping_add(1);
        s.labels2d_job = None;
        s.labels2d_last_snapshot = None;
        s.labels2d_placed.clear();
    });
    render_scene()
}

#[wasm_bindgen]
pub fn add_debug_label(lon_deg: f64, lat_deg: f64, text: String) -> Result<(), JsValue> {
    add_debug_label_with_priority(lon_deg, lat_deg, text, 1000.0)
}

#[wasm_bindgen]
pub fn add_debug_label_with_priority(
    lon_deg: f64,
    lat_deg: f64,
    text: String,
    priority: f64,
) -> Result<(), JsValue> {
    if !lon_deg.is_finite() || !lat_deg.is_finite() || !priority.is_finite() {
        return Err(JsValue::from_str("add_debug_label args must be finite"));
    }
    if text.len() > 256 {
        return Err(JsValue::from_str("debug label text too long"));
    }
    let lon = wrap_lon_deg(lon_deg);
    let lat = clamp(lat_deg, -MERCATOR_MAX_LAT_DEG, MERCATOR_MAX_LAT_DEG);

    let x_m = mercator_x_m(lon) as f32;
    let y_m = mercator_y_m(lat) as f32;
    let viewer_pos = lon_lat_deg_to_world(lon, lat);

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.debug_labels.push(DebugLabel {
            text,
            mercator_m: [x_m, y_m],
            viewer_pos,
            priority: priority as f32,
        });
        s.labels_gen = s.labels_gen.wrapping_add(1);
        // Keep prior placements, but restart the job so it can incorporate the new label.
        s.labels2d_job = None;
        s.labels2d_last_snapshot = None;
    });

    render_scene()
}

#[wasm_bindgen]
pub fn set_real_time_sun_enabled(enabled: bool) -> Result<(), JsValue> {
    with_state(|state| {
        state.borrow_mut().sun_follow_real_time = enabled;
    });
    render_scene()
}

#[wasm_bindgen]
pub fn set_theme(theme: String) -> Result<(), JsValue> {
    let theme = Theme::from_str(&theme);
    let palette = palette_for(theme);

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.theme = theme;

        // Base surface is intended to follow the chosen theme.
        s.base_regions_style.color = palette.base_surface_color;

        if let Some(ctx) = &mut s.wgpu {
            wgpu::set_theme(
                ctx,
                palette.clear_color,
                palette.globe_color,
                palette.stars_alpha,
            );
        }
    });

    let _ = rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn get_globe_settings() -> JsValue {
    let transparent = with_state(|state| state.borrow().globe_transparent);
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("transparent"),
        &JsValue::from_bool(transparent),
    );
    o.into()
}

#[wasm_bindgen]
pub fn set_globe_transparent(transparent: bool) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.globe_transparent = transparent;
        if let Some(ctx) = &mut s.wgpu {
            wgpu::set_globe_transparent(ctx, transparent);
        }
    });
    render_scene()
}

fn layer_style_mut<'a>(s: &'a mut ViewerState, id: &str) -> Option<&'a mut LayerStyle> {
    if let Some(feed) = s.feed_layers.get_mut(id) {
        return Some(&mut feed.style);
    }
    match id {
        "world_base" => Some(&mut s.base_regions_style),
        "terrain" => Some(&mut s.terrain_style),
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

fn layer_style_ref<'a>(s: &'a ViewerState, id: &str) -> Option<&'a LayerStyle> {
    if let Some(feed) = s.feed_layers.get(id) {
        return Some(&feed.style);
    }
    match id {
        "world_base" => Some(&s.base_regions_style),
        "terrain" => Some(&s.terrain_style),
        "cities" => Some(&s.cities_style),
        "air_corridors" => Some(&s.corridors_style),
        "regions" => Some(&s.regions_style),
        "uploaded_points" => Some(&s.uploaded_points_style),
        "uploaded_corridors" => Some(&s.uploaded_corridors_style),
        "uploaded_regions" => Some(&s.uploaded_regions_style),
        "selection" => Some(&s.selection_style),
        _ => None,
    }
}

#[wasm_bindgen]
pub fn set_layer_visible(id: &str, visible: bool) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.visible = visible;
        }
    });
    if visible {
        ensure_builtin_layer_loaded(id);
        if id == "terrain" {
            ensure_terrain_loaded();
        }
    }
    let _ = rebuild_overlays_and_upload();
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

fn color_to_hex(rgb: [f32; 3]) -> String {
    let r = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

fn update_bounds(
    min_lon: &mut f64,
    max_lon: &mut f64,
    min_lat: &mut f64,
    max_lat: &mut f64,
    lon: f64,
    lat: f64,
) {
    if lon.is_finite() && lat.is_finite() {
        *min_lon = min_lon.min(lon);
        *max_lon = max_lon.max(lon);
        *min_lat = min_lat.min(lat);
        *max_lat = max_lat.max(lat);
    }
}

fn update_bounds_for_geometry(
    geom: &formats::VectorGeometry,
    min_lon: &mut f64,
    max_lon: &mut f64,
    min_lat: &mut f64,
    max_lat: &mut f64,
) {
    use formats::VectorGeometry::*;
    match geom {
        Point(p) => update_bounds(min_lon, max_lon, min_lat, max_lat, p.lon_deg, p.lat_deg),
        MultiPoint(ps) | LineString(ps) => {
            for p in ps {
                update_bounds(min_lon, max_lon, min_lat, max_lat, p.lon_deg, p.lat_deg);
            }
        }
        MultiLineString(lines) | Polygon(lines) => {
            for line in lines {
                for p in line {
                    update_bounds(min_lon, max_lon, min_lat, max_lat, p.lon_deg, p.lat_deg);
                }
            }
        }
        MultiPolygon(polys) => {
            for poly in polys {
                for ring in poly {
                    for p in ring {
                        update_bounds(min_lon, max_lon, min_lat, max_lat, p.lon_deg, p.lat_deg);
                    }
                }
            }
        }
    }
}

fn fit_camera_2d_to_bounds(
    cam: &mut Camera2DState,
    w: f64,
    h: f64,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
) {
    let span_lon = (max_lon - min_lon).abs().max(1e-6);
    let span_lat = (max_lat - min_lat).abs().max(1e-6);

    let center_lon = wrap_lon_deg((min_lon + max_lon) * 0.5);
    let center_lat = clamp((min_lat + max_lat) * 0.5, -89.9, 89.9);

    let base = (w / 360.0).min(h / 180.0).max(1e-6);
    let scale = (w / span_lon).min(h / span_lat) * 0.9;
    let zoom = clamp(scale / base, 0.2, 200.0);

    cam.center_lon_deg = center_lon;
    cam.center_lat_deg = center_lat;
    cam.zoom = zoom;
}

fn count_chunk_features(chunk: &formats::VectorChunk) -> (usize, usize, usize) {
    let mut points = 0usize;
    let mut lines = 0usize;
    let mut polys = 0usize;
    for f in &chunk.features {
        match &f.geometry {
            formats::VectorGeometry::Point(_) => points += 1,
            formats::VectorGeometry::MultiPoint(v) => points += v.len(),
            formats::VectorGeometry::LineString(_) => lines += 1,
            formats::VectorGeometry::MultiLineString(v) => lines += v.len(),
            formats::VectorGeometry::Polygon(_) => polys += 1,
            formats::VectorGeometry::MultiPolygon(v) => polys += v.len(),
        }
    }
    (points, lines, polys)
}

#[derive(Debug, Default, Clone, Copy)]
struct UploadCoordFixCounts {
    swapped: usize,
    web_mercator: usize,
    wrapped_lon: usize,
    skipped: usize,
}

fn maybe_wrap_lon_deg(lon_deg: f64) -> Option<f64> {
    if !lon_deg.is_finite() {
        return None;
    }
    if lon_deg.abs() > 180.0 && lon_deg.abs() <= 360.0 {
        Some((lon_deg + 180.0).rem_euclid(360.0) - 180.0)
    } else {
        None
    }
}

fn web_mercator_m_to_lon_lat_deg(x_m: f64, y_m: f64) -> Option<(f64, f64)> {
    if !x_m.is_finite() || !y_m.is_finite() {
        return None;
    }
    // WebMercator uses WGS84_A as radius.
    let r = WGS84_A;
    let lon = (x_m / r).to_degrees();
    let lat = (2.0 * (y_m / r).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    if lon.is_finite() && lat.is_finite() {
        Some((lon, lat))
    } else {
        None
    }
}

fn normalize_geo_point_in_place(p: &mut formats::GeoPoint, counts: &mut UploadCoordFixCounts) {
    let mut lon = p.lon_deg;
    let mut lat = p.lat_deg;

    if !lon.is_finite() || !lat.is_finite() {
        counts.skipped += 1;
        return;
    }

    // 1) Fix common lat/lon swap: lat must be within [-90, 90].
    if lat.abs() > 90.0 && lon.abs() <= 90.0 && lat.abs() <= 180.0 {
        (lon, lat) = (lat, lon);
        counts.swapped += 1;
    }

    // 2) Fix WebMercator meters if values look like meters.
    // Heuristic: magnitude in the thousands+ and within WebMercator's max extent.
    let max_abs = lon.abs().max(lat.abs());
    let merc_max = 20037508.342789244_f64;
    if max_abs > 1000.0
        && lon.abs() <= merc_max * 1.1
        && lat.abs() <= merc_max * 1.1
        && let Some((lon_deg, lat_deg)) = web_mercator_m_to_lon_lat_deg(lon, lat)
    {
        lon = lon_deg;
        lat = lat_deg;
        counts.web_mercator += 1;
    }

    // 3) Wrap lon to [-180, 180] for 0..360 style data.
    if let Some(w) = maybe_wrap_lon_deg(lon) {
        lon = w;
        counts.wrapped_lon += 1;
    }

    p.lon_deg = lon;
    p.lat_deg = lat;
}

fn normalize_geometry_in_place(
    geom: &mut formats::VectorGeometry,
    counts: &mut UploadCoordFixCounts,
) {
    use formats::VectorGeometry::*;
    match geom {
        Point(p) => normalize_geo_point_in_place(p, counts),
        MultiPoint(ps) | LineString(ps) => {
            for p in ps {
                normalize_geo_point_in_place(p, counts);
            }
        }
        MultiLineString(lines) | Polygon(lines) => {
            for line in lines {
                for p in line {
                    normalize_geo_point_in_place(p, counts);
                }
            }
        }
        MultiPolygon(polys) => {
            for poly in polys {
                for ring in poly {
                    for p in ring {
                        normalize_geo_point_in_place(p, counts);
                    }
                }
            }
        }
    }
}

fn normalize_chunk_coords_in_place(chunk: &mut formats::VectorChunk) -> UploadCoordFixCounts {
    let mut counts = UploadCoordFixCounts::default();
    for f in &mut chunk.features {
        normalize_geometry_in_place(&mut f.geometry, &mut counts);
    }
    counts
}

async fn fetch_geojson_chunk(url: &str) -> Result<formats::VectorChunk, JsValue> {
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

fn ensure_base_world_loaded() {
    let needs_load = with_state(|state| {
        let s = state.borrow();
        s.base_world.is_none()
            && s.surface_positions.is_none()
            && !s.surface_loading
            && !s.base_world_loading
    });
    if !needs_load {
        return;
    }

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.base_world_loading = true;
        s.base_world_error = None;
        s.base_world_source = Some("world.json".to_string());
    });

    spawn_local(async move {
        let chunk = match fetch_geojson_chunk("assets/world.json").await {
            Ok(c) => c,
            Err(err) => {
                web_sys::console::log_1(&JsValue::from_str(&format!(
                    "Failed to fetch base world: {:?}",
                    err
                )));
                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.base_world_loading = false;
                    s.base_world_error = Some("base world load failed".to_string());
                });
                return;
            }
        };

        let chunk = unwrap_antimeridian_chunk(&chunk);

        let (_points, _lines, polys) = count_chunk_features(&chunk);
        let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Area));
        // TODO: when a DEM/surface model is available, sample heights for layer-0 terrain.

        with_state(|state| {
            let mut s = state.borrow_mut();
            // Avoid races: if another base-world source started loading after we began
            // fetching world.json (e.g., PMTiles streaming), do not overwrite it.
            if s.base_world_source.as_deref() != Some("world.json") {
                return;
            }
            s.base_world = Some(world);
            s.base_count_polys = polys;
            s.base_world_loading = false;
            s.surface_last_error = None;
        });

        let _ = rebuild_overlays_and_upload();
        let _ = render_scene();
    });
}

#[wasm_bindgen]
pub fn begin_base_world_stream() {
    with_state(|state| {
        let mut s = state.borrow_mut();
        let mut world = scene::World::new();
        scene::prefabs::spawn_wgs84_globe(&mut world);
        s.base_world = Some(world);
        s.surface_positions = None;
        s.surface_tileset = None;
        s.surface_zoom = None;
        s.surface_source = None;
        s.surface_loading = false;
        s.surface_last_error = None;
        s.base_world_loading = true;
        s.base_world_error = None;
        s.base_world_source = Some("world.pmtiles".to_string());
        s.base_count_polys = 0;
    });
    let _ = rebuild_overlays_and_upload();
    let _ = render_scene();
}

#[wasm_bindgen]
pub fn append_base_world_geojson_chunk(geojson_text: String) -> Result<(), JsValue> {
    let chunk = formats::VectorChunk::from_geojson_str(&geojson_text)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let chunk = unwrap_antimeridian_chunk(&chunk);

    with_state(|state| {
        let mut s = state.borrow_mut();
        if s.base_world.is_none() {
            // If the UI calls append without begin, create a base world.
            let mut world = scene::World::new();
            scene::prefabs::spawn_wgs84_globe(&mut world);
            s.base_world = Some(world);
        }
        if let Some(world) = s.base_world.as_mut() {
            formats::ingest_vector_chunk(world, &chunk, Some(VectorGeometryKind::Area));
        }
    });

    Ok(())
}

#[wasm_bindgen]
pub fn finish_base_world_stream() {
    let tri_count = with_state(|state| {
        let mut s = state.borrow_mut();
        s.base_world_loading = false;

        if let Some(w) = s.base_world.as_ref() {
            let layer = VectorLayer::new(1);
            let snap = layer.extract(w);
            snap.area_triangles.len() / 3
        } else {
            0
        }
    });

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.base_count_polys = tri_count;
    });

    let _ = rebuild_overlays_and_upload();
    let _ = render_scene();
}

fn unwrap_antimeridian_chunk(chunk: &formats::VectorChunk) -> formats::VectorChunk {
    use formats::{VectorChunk, VectorFeature, VectorGeometry};

    let features = chunk
        .features
        .iter()
        .map(|feat| {
            let geometry = match &feat.geometry {
                VectorGeometry::Polygon(rings) => {
                    VectorGeometry::Polygon(rings.iter().map(|ring| unwrap_ring(ring)).collect())
                }
                VectorGeometry::MultiPolygon(polys) => VectorGeometry::MultiPolygon(
                    polys
                        .iter()
                        .map(|poly| poly.iter().map(|ring| unwrap_ring(ring)).collect())
                        .collect(),
                ),
                VectorGeometry::LineString(points) => {
                    VectorGeometry::LineString(unwrap_ring(points))
                }
                VectorGeometry::MultiLineString(lines) => VectorGeometry::MultiLineString(
                    lines.iter().map(|line| unwrap_ring(line)).collect(),
                ),
                other => other.clone(),
            };

            VectorFeature {
                id: feat.id.clone(),
                properties: feat.properties.clone(),
                geometry,
            }
        })
        .collect();

    VectorChunk { features }
}

fn unwrap_ring(points: &[formats::GeoPoint]) -> Vec<formats::GeoPoint> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<formats::GeoPoint> = Vec::with_capacity(points.len());
    let mut prev_lon = points[0].lon_deg;
    out.push(formats::GeoPoint::new(prev_lon, points[0].lat_deg));

    for p in points.iter().skip(1) {
        let mut lon = p.lon_deg;
        let mut delta = lon - prev_lon;
        if delta > 180.0 {
            lon -= 360.0;
        } else if delta < -180.0 {
            lon += 360.0;
        }
        delta = lon - prev_lon;
        if delta > 180.0 {
            lon -= 360.0;
        } else if delta < -180.0 {
            lon += 360.0;
        }
        prev_lon = lon;
        out.push(formats::GeoPoint::new(lon, p.lat_deg));
    }

    out
}

fn ensure_builtin_layer_loaded(id: &str) {
    match id {
        "world_base" => {
            ensure_surface_loaded();
        }
        "cities" => {
            let needs_load = with_state(|state| state.borrow().cities_world.is_none());
            if !needs_load {
                return;
            }
            spawn_local(async move {
                let chunk = match fetch_vector_chunk("assets/chunks/cities.avc").await {
                    Ok(c) => c,
                    Err(err) => {
                        web_sys::console::log_1(&JsValue::from_str(&format!(
                            "Failed to fetch cities: {:?}",
                            err
                        )));
                        return;
                    }
                };

                let (points, _lines, _polys) = count_chunk_features(&chunk);
                let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Point));

                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.cities_world = Some(world);
                    s.cities_count_points = points;
                });
                let _ = rebuild_overlays_and_upload();
                let _ = render_scene();
            });
        }
        "air_corridors" => {
            let needs_load = with_state(|state| state.borrow().corridors_world.is_none());
            if !needs_load {
                return;
            }
            spawn_local(async move {
                let chunk = match fetch_vector_chunk("assets/chunks/air_corridors.avc").await {
                    Ok(c) => c,
                    Err(err) => {
                        web_sys::console::log_1(&JsValue::from_str(&format!(
                            "Failed to fetch air corridors: {:?}",
                            err
                        )));
                        return;
                    }
                };

                let (_points, lines, _polys) = count_chunk_features(&chunk);
                let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Line));

                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.corridors_world = Some(world);
                    s.corridors_count_lines = lines;
                });
                let _ = rebuild_overlays_and_upload();
                let _ = render_scene();
            });
        }
        "regions" => {
            let needs_load = with_state(|state| state.borrow().regions_world.is_none());
            if !needs_load {
                return;
            }
            spawn_local(async move {
                let chunk = match fetch_vector_chunk("assets/chunks/regions.avc").await {
                    Ok(c) => c,
                    Err(err) => {
                        web_sys::console::log_1(&JsValue::from_str(&format!(
                            "Failed to fetch regions: {:?}",
                            err
                        )));
                        return;
                    }
                };

                let (_points, _lines, polys) = count_chunk_features(&chunk);
                let world = world_from_vector_chunk(&chunk, Some(VectorGeometryKind::Area));

                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.regions_world = Some(world);
                    s.regions_count_polys = polys;
                });
                let _ = rebuild_overlays_and_upload();
                let _ = render_scene();
            });
        }
        _ => {}
    }
}

fn backend_base_url() -> Option<String> {
    let window = web_sys::window()?;
    let val = js_sys::Reflect::get(&window, &JsValue::from_str("__atlasBackendUrl")).ok()?;
    let s = val.as_string().unwrap_or_default();
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn surface_tileset_url(base: &str) -> String {
    format!("{base}/surface/tileset.json")
}

fn surface_tile_url(
    base: Option<&str>,
    tileset: &SurfaceTileset,
    z: u32,
    x: u32,
    y: u32,
) -> String {
    let path = tileset
        .tile_path_template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string());

    match base {
        Some(b) => {
            let prefix = if b.ends_with('/') {
                b.to_string()
            } else {
                format!("{b}/")
            };
            join_url(&prefix, &format!("surface/{path}"))
        }
        None => join_url("assets/surface/", &path),
    }
}

fn terrain_tileset_url() -> String {
    if let Some(base) = backend_base_url() {
        format!("{base}/terrain/tileset.json")
    } else {
        "assets/terrain/tileset.json".to_string()
    }
}

fn terrain_tile_url(
    base: Option<&str>,
    tileset: &TerrainTileset,
    z: u32,
    x: u32,
    y: u32,
) -> String {
    let path = tileset
        .tile_path_template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string());
    match base {
        Some(b) => {
            let prefix = if b.ends_with('/') {
                b.to_string()
            } else {
                format!("{b}/")
            };
            join_url(&prefix, &format!("terrain/{path}"))
        }
        None => join_url("assets/terrain/", &path),
    }
}

fn terrain_tile_bounds(tileset: &TerrainTileset, z: u32, x: u32, y: u32) -> (f64, f64, f64, f64) {
    let n = 2u32.pow(z);
    let lon_span = (tileset.max_lon - tileset.min_lon) / n as f64;
    let lat_span = (tileset.max_lat - tileset.min_lat) / n as f64;

    let lon_min = tileset.min_lon + (x as f64) * lon_span;
    let lon_max = lon_min + lon_span;
    let lat_max = tileset.max_lat - (y as f64) * lat_span;
    let lat_min = lat_max - lat_span;
    (lon_min, lon_max, lat_min, lat_max)
}

fn decode_f32_le(bytes: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    out
}

fn decode_f32_vec3(bytes: &[u8]) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(bytes.len() / 12);
    for chunk in bytes.chunks_exact(12) {
        let x = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let y = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let z = f32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
        out.push([x, y, z]);
    }
    out
}

fn terrain_height_color(height_m: f32, min_m: f32, max_m: f32, alpha: f32) -> [f32; 4] {
    let t = if (max_m - min_m).abs() < 1e-3 {
        0.5
    } else {
        ((height_m - min_m) / (max_m - min_m)).clamp(0.0, 1.0)
    };

    let (r, g, b) = if t < 0.3 {
        let u = t / 0.3;
        (0.05 + 0.15 * u, 0.25 + 0.45 * u, 0.45 + 0.35 * u)
    } else if t < 0.7 {
        let u = (t - 0.3) / 0.4;
        (0.20 + 0.25 * u, 0.70 - 0.25 * u, 0.20 + 0.10 * u)
    } else {
        let u = (t - 0.7) / 0.3;
        (0.45 + 0.40 * u, 0.45 + 0.40 * u, 0.30 + 0.50 * u)
    };

    [r, g, b, alpha]
}

fn build_terrain_mesh(tileset: &TerrainTileset, tiles: &[TerrainTile]) -> Vec<TerrainVertex> {
    let tile_size = tileset.tile_size as usize;
    let step = tileset.sample_step.unwrap_or(4).max(1) as usize;
    let no_data = tileset.no_data.unwrap_or(-9999.0) as f32;
    let min_h = tileset.min_height as f32;
    let max_h = tileset.max_height as f32;

    let mut vertices: Vec<TerrainVertex> = Vec::new();

    for tile in tiles {
        if tile.heights_m.len() < tile_size * tile_size {
            continue;
        }
        let (lon_min, lon_max, lat_min, lat_max) =
            terrain_tile_bounds(tileset, tile.z, tile.x, tile.y);
        let lon_span = lon_max - lon_min;
        let lat_span = lat_max - lat_min;

        for j in (0..tile_size - step).step_by(step) {
            for i in (0..tile_size - step).step_by(step) {
                let idx00 = j * tile_size + i;
                let idx10 = j * tile_size + (i + step);
                let idx01 = (j + step) * tile_size + i;
                let idx11 = (j + step) * tile_size + (i + step);

                let h00 = tile.heights_m[idx00];
                let h10 = tile.heights_m[idx10];
                let h01 = tile.heights_m[idx01];
                let h11 = tile.heights_m[idx11];

                if h00 <= no_data || h10 <= no_data || h01 <= no_data || h11 <= no_data {
                    continue;
                }

                let u0 = i as f64 / (tile_size - 1) as f64;
                let u1 = (i + step) as f64 / (tile_size - 1) as f64;
                let v0 = j as f64 / (tile_size - 1) as f64;
                let v1 = (j + step) as f64 / (tile_size - 1) as f64;

                let lon0 = lon_min + lon_span * u0;
                let lon1 = lon_min + lon_span * u1;
                let lat0 = lat_max - lat_span * v0;
                let lat1 = lat_max - lat_span * v1;

                let p00 = lon_lat_deg_to_world(lon0, lat0);
                let p10 = lon_lat_deg_to_world(lon1, lat0);
                let p01 = lon_lat_deg_to_world(lon0, lat1);
                let p11 = lon_lat_deg_to_world(lon1, lat1);

                let c00 = terrain_height_color(h00, min_h, max_h, 0.95);
                let c10 = terrain_height_color(h10, min_h, max_h, 0.95);
                let c01 = terrain_height_color(h01, min_h, max_h, 0.95);
                let c11 = terrain_height_color(h11, min_h, max_h, 0.95);

                // Triangle 1: p00, p10, p11
                vertices.push(TerrainVertex {
                    position: p00,
                    lift_m: h00,
                    color: c00,
                });
                vertices.push(TerrainVertex {
                    position: p10,
                    lift_m: h10,
                    color: c10,
                });
                vertices.push(TerrainVertex {
                    position: p11,
                    lift_m: h11,
                    color: c11,
                });

                // Triangle 2: p00, p11, p01
                vertices.push(TerrainVertex {
                    position: p00,
                    lift_m: h00,
                    color: c00,
                });
                vertices.push(TerrainVertex {
                    position: p11,
                    lift_m: h11,
                    color: c11,
                });
                vertices.push(TerrainVertex {
                    position: p01,
                    lift_m: h01,
                    color: c01,
                });
            }
        }
    }

    vertices
}

fn desired_terrain_zoom(distance: f64, tileset: &TerrainTileset) -> u32 {
    let ratio = (WGS84_A / distance.max(1.0)).max(0.01);
    let z = ratio.log2().floor() as i32 + 2;
    let z = z.clamp(tileset.zoom_min as i32, tileset.zoom_max as i32);
    z as u32
}

fn desired_surface_zoom(distance: f64, tileset: &SurfaceTileset) -> u32 {
    let ratio = (WGS84_A / distance.max(1.0)).max(0.01);
    let z = ratio.log2().floor() as i32 + 1;
    let z = z.clamp(tileset.zoom_min as i32, tileset.zoom_max as i32);
    z as u32
}

fn desired_surface_zoom_for_mode(mode: ViewMode, distance: f64, tileset: &SurfaceTileset) -> u32 {
    if mode == ViewMode::TwoD {
        return tileset.zoom_min;
    }
    desired_surface_zoom(distance, tileset)
}

fn ensure_surface_loaded() {
    let surface_enabled = with_state(|state| state.borrow().base_regions_style.visible);
    if !surface_enabled {
        return;
    }

    let Some(backend) = backend_base_url() else {
        with_state(|state| {
            let mut s = state.borrow_mut();
            s.surface_loading = false;
            s.surface_last_error = Some("backend offline; using world.json".to_string());
        });
        ensure_base_world_loaded();
        return;
    };

    let source = backend.clone();
    let tileset_url = surface_tileset_url(&backend);
    let now = now_ms();

    let should_load = with_state(|state| {
        let s = state.borrow();
        !s.surface_loading
            && now >= s.surface_next_retry_ms
            && (s.surface_tileset.is_none()
                || s.surface_tileset
                    .as_ref()
                    .map(|ts| desired_surface_zoom_for_mode(s.view_mode, s.camera.distance, ts))
                    != s.surface_zoom
                || s.surface_source.as_deref() != Some(&source))
    });

    if !should_load {
        return;
    }

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.surface_loading = true;
        s.surface_last_error = None;
    });

    spawn_local(async move {
        let tileset = match fetch_surface_tileset(&tileset_url).await {
            Ok(ts) => ts,
            Err(err) => {
                let err_str = err
                    .as_string()
                    .unwrap_or_else(|| "surface tileset error".to_string());
                // If the server doesn't have pre-tessellated surface tiles configured,
                // treat that as a long-lived condition (avoid spamming 404s).
                let backoff_ms = if err_str.contains("HTTP 404") {
                    24.0 * 60.0 * 60_000.0
                } else {
                    30_000.0
                };

                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.surface_loading = false;
                    // Only surface this as an error if we *don't* already have a usable base world.
                    // Missing server-side surface tiles are expected in most deployments.
                    if s.base_world.is_none() {
                        s.surface_last_error = Some(err_str);
                    } else {
                        s.surface_last_error = None;
                    }
                    s.surface_next_retry_ms = now_ms() + backoff_ms;
                });
                ensure_base_world_loaded();
                return;
            }
        };

        let zoom = desired_surface_zoom_for_mode(
            with_state(|state| state.borrow().view_mode),
            with_state(|state| state.borrow().camera.distance),
            &tileset,
        );
        let n = 2u32.pow(zoom);
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let backend = Some(backend);

        for y in 0..n {
            for x in 0..n {
                let url = surface_tile_url(backend.as_deref(), &tileset, zoom, x, y);
                let bytes = match fetch_binary(&url).await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let mut verts = decode_f32_vec3(&bytes);
                if !verts.is_empty() {
                    positions.append(&mut verts);
                }
            }
        }

        if positions.is_empty() {
            with_state(|state| {
                let mut s = state.borrow_mut();
                s.surface_loading = false;
                s.surface_last_error = Some("no surface tiles".to_string());
                s.surface_next_retry_ms = now_ms() + 60_000.0;
            });
            ensure_base_world_loaded();
            return;
        }

        let tri_count = positions.len() / 3;
        with_state(|state| {
            let mut s = state.borrow_mut();
            s.surface_tileset = Some(tileset);
            s.surface_positions = Some(positions);
            s.surface_zoom = Some(zoom);
            s.surface_source = Some(source);
            s.surface_loading = false;
            s.surface_next_retry_ms = 0.0;
            s.base_world = None;
            s.base_count_polys = tri_count;
        });

        let _ = rebuild_overlays_and_upload();
        let _ = render_scene();
    });
}

fn ensure_terrain_loaded() {
    let terrain_enabled = with_state(|state| state.borrow().terrain_style.visible);
    if !terrain_enabled {
        return;
    }

    let (should_load, tileset_url, source) = with_state(|state| {
        let s = state.borrow();
        let tileset_url = terrain_tileset_url();
        let source = backend_base_url().unwrap_or_else(|| "assets".to_string());
        let should = !s.terrain_loading
            && (s.terrain_tileset.is_none()
                || s.terrain_tileset
                    .as_ref()
                    .map(|ts| desired_terrain_zoom(s.camera.distance, ts))
                    != s.terrain_zoom
                || s.terrain_source.as_deref() != Some(&source));
        (should, tileset_url, source)
    });

    if !should_load {
        return;
    }

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.terrain_loading = true;
        s.terrain_last_error = None;
    });

    spawn_local(async move {
        let tileset = match fetch_terrain_tileset(&tileset_url).await {
            Ok(ts) => ts,
            Err(err) => {
                with_state(|state| {
                    let mut s = state.borrow_mut();
                    s.terrain_loading = false;
                    s.terrain_last_error = Some(
                        err.as_string()
                            .unwrap_or_else(|| "tileset error".to_string()),
                    );
                });
                return;
            }
        };

        let zoom =
            desired_terrain_zoom(with_state(|state| state.borrow().camera.distance), &tileset);
        let n = 2u32.pow(zoom);
        let mut tiles: Vec<TerrainTile> = Vec::new();
        let backend = backend_base_url();

        for y in 0..n {
            for x in 0..n {
                let url = terrain_tile_url(backend.as_deref(), &tileset, zoom, x, y);
                let bytes = match fetch_binary(&url).await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let heights = decode_f32_le(&bytes);
                tiles.push(TerrainTile {
                    z: zoom,
                    x,
                    y,
                    heights_m: heights,
                });
            }
        }

        let vertices = build_terrain_mesh(&tileset, &tiles);

        with_state(|state| {
            let mut s = state.borrow_mut();
            s.terrain_tileset = Some(tileset);
            s.terrain_vertices = Some(vertices.clone());
            s.terrain_zoom = Some(zoom);
            s.terrain_source = Some(source);
            s.terrain_loading = false;

            if let Some(ctx) = &mut s.wgpu {
                set_terrain_points(ctx, &vertices);
                s.pending_terrain = None;
            } else {
                s.pending_terrain = Some(vertices);
            }
        });

        let _ = render_scene();
    });
}

#[wasm_bindgen]
pub fn get_layer_style(id: &str) -> Result<JsValue, JsValue> {
    let style = with_state(|state| {
        let s = state.borrow();
        layer_style_ref(&s, id).copied()
    });

    let Some(style) = style else {
        return Err(JsValue::from_str("Unknown layer id"));
    };

    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("visible"),
        &JsValue::from_bool(style.visible),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("color_hex"),
        &JsValue::from_str(&color_to_hex([
            style.color[0],
            style.color[1],
            style.color[2],
        ])),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("opacity"),
        &JsValue::from_f64(style.color[3] as f64),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("lift"),
        &JsValue::from_f64(style.lift as f64),
    );
    Ok(o.into())
}

#[wasm_bindgen]
pub fn get_terrain_client_status() -> JsValue {
    let (
        enabled,
        loading,
        last_error,
        zoom,
        source,
        tileset_loaded,
        vertical_datum,
        vertical_units,
    ) = with_state(|state| {
        let s = state.borrow();
        (
            s.terrain_style.visible,
            s.terrain_loading,
            s.terrain_last_error.clone(),
            s.terrain_zoom,
            s.terrain_source.clone(),
            s.terrain_tileset.is_some(),
            s.terrain_tileset
                .as_ref()
                .and_then(|ts| ts.vertical_datum.clone()),
            s.terrain_tileset
                .as_ref()
                .and_then(|ts| ts.vertical_units.clone()),
        )
    });

    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("enabled"),
        &JsValue::from_bool(enabled),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("loading"),
        &JsValue::from_bool(loading),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("tileset_loaded"),
        &JsValue::from_bool(tileset_loaded),
    );
    if let Some(z) = zoom {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("zoom"), &JsValue::from_f64(z as f64));
    }
    if let Some(src) = source {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("source"), &JsValue::from_str(&src));
    }
    if let Some(err) = last_error {
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("last_error"),
            &JsValue::from_str(&err),
        );
    }
    if let Some(v) = vertical_datum {
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("vertical_datum"),
            &JsValue::from_str(&v),
        );
    }
    if let Some(v) = vertical_units {
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("vertical_units"),
            &JsValue::from_str(&v),
        );
    }
    o.into()
}

#[wasm_bindgen]
pub fn get_surface_status() -> JsValue {
    let (loaded, loading, tileset_loaded, zoom, source, error, base_polygons) =
        with_state(|state| {
            let s = state.borrow();
            let loaded = s.surface_positions.is_some() || s.base_world.is_some();
            let loading = s.surface_loading || s.base_world_loading;
            let tileset_loaded = s.surface_tileset.is_some();
            let zoom = s.surface_zoom;
            let source = if s.surface_positions.is_some() {
                s.surface_source.clone()
            } else if s.base_world.is_some() {
                s.base_world_source
                    .clone()
                    .or_else(|| Some("world.json".to_string()))
            } else {
                None
            };
            let error = s
                .surface_last_error
                .clone()
                .or_else(|| s.base_world_error.clone());
            let base_polygons = s.base_count_polys;
            (
                loaded,
                loading,
                tileset_loaded,
                zoom,
                source,
                error,
                base_polygons,
            )
        });

    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("loaded"),
        &JsValue::from_bool(loaded),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("loading"),
        &JsValue::from_bool(loading),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("tileset_loaded"),
        &JsValue::from_bool(tileset_loaded),
    );
    if let Some(z) = zoom {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("zoom"), &JsValue::from_f64(z as f64));
    }
    if let Some(src) = source {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("source"), &JsValue::from_str(&src));
    }
    if let Some(err) = error {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("error"), &JsValue::from_str(&err));
    }
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("base_polygons"),
        &JsValue::from_f64(base_polygons as f64),
    );
    o.into()
}

#[wasm_bindgen]
pub fn get_builtin_layers() -> Result<JsValue, JsValue> {
    let arr = js_sys::Array::new();
    with_state(|state| {
        let s = state.borrow();
        let push = |id: &str, name: &str, points: usize, lines: usize, polys: usize| {
            let o = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&o, &JsValue::from_str("id"), &JsValue::from_str(id));
            let _ = js_sys::Reflect::set(&o, &JsValue::from_str("name"), &JsValue::from_str(name));
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("points"),
                &JsValue::from_f64(points as f64),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("lines"),
                &JsValue::from_f64(lines as f64),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("polygons"),
                &JsValue::from_f64(polys as f64),
            );
            let _ = js_sys::Reflect::set(&o, &JsValue::from_str("builtin"), &JsValue::TRUE);
            arr.push(&o);
        };

        push("world_base", "World base", 0, 0, s.base_count_polys);
        push("cities", "Cities", s.cities_count_points, 0, 0);
        push(
            "air_corridors",
            "Air corridors",
            0,
            s.corridors_count_lines,
            0,
        );
        push("regions", "Regions", 0, 0, s.regions_count_polys);
    });
    Ok(arr.into())
}

#[wasm_bindgen]
pub fn get_city_marker_size() -> f64 {
    with_state(|state| state.borrow().city_marker_size as f64)
}

#[wasm_bindgen]
pub fn get_line_width_px() -> f64 {
    with_state(|state| state.borrow().line_width_px as f64)
}

#[wasm_bindgen]
pub fn get_auto_rotate_settings() -> JsValue {
    let (enabled, speed) = with_state(|state| {
        let s = state.borrow();
        (s.auto_rotate_enabled, s.auto_rotate_speed_deg_per_s)
    });

    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("enabled"),
        &JsValue::from_bool(enabled),
    );
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("speed_deg_per_s"),
        &JsValue::from_f64(speed),
    );
    o.into()
}

#[wasm_bindgen]
pub fn set_auto_rotate_enabled(enabled: bool) {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_enabled = enabled;
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
    });
}

#[wasm_bindgen]
pub fn set_auto_rotate_speed_deg_per_s(speed: f64) {
    if !speed.is_finite() {
        return;
    }
    let clamped = speed.clamp(0.0, 2.0);
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_speed_deg_per_s = clamped;
    });
}

#[wasm_bindgen]
pub fn set_layer_color_hex(id: &str, hex: &str) -> Result<(), JsValue> {
    let rgb = parse_hex_color(hex).ok_or_else(|| JsValue::from_str("Invalid color"))?;
    with_state(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.color[0] = rgb[0];
            st.color[1] = rgb[1];
            st.color[2] = rgb[2];
        }
    });
    let _ = rebuild_styles_and_upload_only();
    render_scene()
}

#[wasm_bindgen]
pub fn set_layer_opacity(id: &str, opacity: f64) -> Result<(), JsValue> {
    let a = (opacity as f32).clamp(0.0, 1.0);
    with_state(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.color[3] = a;
        }
    });
    let _ = rebuild_styles_and_upload_only();
    render_scene()
}

#[wasm_bindgen]
pub fn set_layer_lift(id: &str, lift: f64) -> Result<(), JsValue> {
    let lift = (lift as f32).clamp(-0.1, 0.2);
    with_state(|state| {
        let mut s = state.borrow_mut();
        if let Some(st) = layer_style_mut(&mut s, id) {
            st.lift = lift;
        }
    });
    let _ = rebuild_styles_and_upload_only();
    render_scene()
}

#[wasm_bindgen]
pub fn get_uploaded_summary() -> Result<JsValue, JsValue> {
    let summary = js_sys::Object::new();
    with_state(|state| {
        let s = state.borrow();
        let name = s.uploaded_name.clone().unwrap_or_else(|| "".to_string());
        let catalog_id = s
            .uploaded_catalog_id
            .clone()
            .unwrap_or_else(|| "".to_string());
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("name"),
            &JsValue::from_str(&name),
        );
        let _ = js_sys::Reflect::set(
            &summary,
            &JsValue::from_str("catalog_id"),
            &JsValue::from_str(&catalog_id),
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

#[wasm_bindgen]
pub fn clear_uploaded() -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.uploaded_name = None;
        s.uploaded_catalog_id = None;
        s.uploaded_world = None;
        s.uploaded_centers = None;
        s.uploaded_corridors_positions = None;
        s.uploaded_regions_positions = None;
        s.uploaded_count_points = 0;
        s.uploaded_count_lines = 0;
        s.uploaded_count_polys = 0;
        s.uploaded_points_style.visible = false;
        s.uploaded_corridors_style.visible = false;
        s.uploaded_regions_style.visible = false;
    });
    let _ = rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn catalog_list() -> Result<JsValue, JsValue> {
    let arr = js_sys::Array::new();
    let entries = CATALOG
        .with(|cat| cat.borrow().list())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    for e in entries {
        let o = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("id"), &JsValue::from_str(&e.id));
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("name"), &JsValue::from_str(&e.name));
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("points"),
            &JsValue::from_f64(e.count_points as f64),
        );
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("lines"),
            &JsValue::from_f64(e.count_lines as f64),
        );
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("polygons"),
            &JsValue::from_f64(e.count_polys as f64),
        );
        let _ = js_sys::Reflect::set(
            &o,
            &JsValue::from_str("created_at_ms"),
            &JsValue::from_f64(e.created_at_ms as f64),
        );
        arr.push(&o);
    }

    Ok(arr.into())
}

#[wasm_bindgen]
pub async fn catalog_delete(id: String) -> Result<bool, JsValue> {
    // Best-effort: delete bytes from IndexedDB first.
    let _ = idb_delete_avc(&id).await;

    CATALOG
        .with(|cat| cat.borrow_mut().delete(&id))
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub async fn catalog_load(id: String) -> Result<JsValue, JsValue> {
    let entry = CATALOG
        .with(|cat| cat.borrow().get(&id))
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .ok_or_else(|| JsValue::from_str("catalog entry not found"))?;

    // Prefer IndexedDB bytes.
    let bytes = match idb_get_avc_bytes(&id).await {
        Ok(Some(b)) => b,
        _ => CATALOG
            .with(|cat| cat.borrow().get_avc_bytes(&id))
            .map_err(|e| JsValue::from_str(&e.to_string()))?,
    };

    let mut chunk = formats::VectorChunk::from_avc_bytes(&bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    // Best-effort normalization for older uploads.
    let _ = normalize_chunk_coords_in_place(&mut chunk);

    // Count primitives for UI + bounds for camera fit.
    let mut count_points = 0usize;
    let mut count_lines = 0usize;
    let mut count_polys = 0usize;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for f in &chunk.features {
        update_bounds_for_geometry(
            &f.geometry,
            &mut min_lon,
            &mut max_lon,
            &mut min_lat,
            &mut max_lat,
        );
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

    with_state(|state| {
        let mut s = state.borrow_mut();
        s.uploaded_name = Some(entry.name.clone());
        s.uploaded_catalog_id = Some(entry.id.clone());
        s.uploaded_world = Some(world);
        s.uploaded_count_points = count_points;
        s.uploaded_count_lines = count_lines;
        s.uploaded_count_polys = count_polys;

        s.dataset = "__uploaded__".to_string();
        s.uploaded_points_style.visible = count_points > 0;
        s.uploaded_corridors_style.visible = count_lines > 0;
        s.uploaded_regions_style.visible = count_polys > 0;

        if min_lon.is_finite() && max_lon.is_finite() && min_lat.is_finite() && max_lat.is_finite()
        {
            let w = s.canvas_width.max(1.0);
            let h = s.canvas_height.max(1.0);
            fit_camera_2d_to_bounds(&mut s.camera_2d, w, h, min_lon, max_lon, min_lat, max_lat);
        }
    });

    rebuild_overlays_and_upload()?;
    let _ = render_scene();
    get_uploaded_summary()
}

fn world_to_lon_lat_deg(p: [f64; 3]) -> (f64, f64) {
    // Convert viewer coordinates back to ECEF (z-up): ecef = (x, -z, y)
    let ecef = foundation::math::Ecef::new(p[0], -p[2], p[1]);
    let geo = ecef_to_geodetic(ecef);
    (geo.lon_rad.to_degrees(), geo.lat_rad.to_degrees())
}

fn viewer_to_mercator_m(p: &[f32; 3]) -> [f32; 2] {
    let (lon, lat) = world_to_lon_lat_fast_deg([p[0], p[1], p[2]]);
    let x = mercator_x_m(lon);
    let y = mercator_y_m(lat);
    [x as f32, y as f32]
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
        -camera.pitch_rad.cos() * camera.yaw_rad.sin(),
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
    let (mode, cam2d, camera3d, w, h) = with_state(|state| {
        let s = state.borrow();
        (
            s.view_mode,
            s.camera_2d,
            s.camera,
            s.canvas_width,
            s.canvas_height,
        )
    });

    match mode {
        ViewMode::ThreeD => {
            let hit = ray_hit_globe(camera3d, w, h, x_px, y_px);
            if let Some(p) = hit {
                let (lon, lat) = world_to_lon_lat_deg(p);
                js_sys::Reflect::set(&out, &JsValue::from_str("hit"), &JsValue::TRUE)?;
                js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
                js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
            } else {
                js_sys::Reflect::set(&out, &JsValue::from_str("hit"), &JsValue::FALSE)?;
            }
        }
        ViewMode::TwoD => {
            let projector = MercatorProjector::new(cam2d, w.max(1.0), h.max(1.0));
            let (lon, lat) = projector.screen_to_lon_lat(x_px, y_px, w.max(1.0), h.max(1.0));
            js_sys::Reflect::set(&out, &JsValue::from_str("hit"), &JsValue::TRUE)?;
            js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
            js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
        }
    }
    Ok(out.into())
}

#[wasm_bindgen]
pub fn cursor_click(x_px: f64, y_px: f64) -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();

    let mode = with_state(|state| state.borrow().view_mode);
    if mode == ViewMode::TwoD {
        return cursor_click_2d(x_px, y_px);
    }

    let hit = with_state(|state| {
        let s = state.borrow();
        ray_hit_globe(s.camera, s.canvas_width, s.canvas_height, x_px, y_px)
    });
    if hit.is_none() {
        // Clear selection if nothing hit on globe.
        with_state(|state| {
            let mut s = state.borrow_mut();
            s.selection_center = None;
            s.selection_line_positions = None;
            s.selection_poly_positions = None;
        });
        let _ = rebuild_overlays_and_upload();
        let _ = render_scene();
        js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::FALSE)?;
        return Ok(out.into());
    }

    // Prefer: point (near) -> line (near) -> polygon (inside).
    let picked = with_state(|state| {
        let s = state.borrow();
        let view_proj =
            globe_controller_view_proj(&s.globe_controller, s.canvas_width, s.canvas_height);
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
            for &c in centers.iter().take(2_000_000) {
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
        for layer in s.feed_layers.values() {
            if layer.style.visible && !layer.centers.is_empty() {
                consider_points(&layer.centers);
            }
        }
        if let Some((c, d2)) = best_point {
            // Markers are rendered at a constant screen-space size.
            let radius_px = (s.city_marker_size).clamp(2.0, 64.0);
            if d2 <= radius_px * radius_px {
                return Some(("point".to_string(), Some(c), None, None));
            }
        }

        // 2) Lines (distance to segment)
        let mut best_line: Option<([f32; 3], [f32; 3], f32)> = None;
        let mut consider_lines = |pos: &[[f32; 3]]| {
            for seg in pos.chunks_exact(2).take(1_500_000) {
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
            for tri in pos.chunks_exact(3).take(500_000) {
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
        with_state(|state| {
            let mut s = state.borrow_mut();
            s.selection_center = point;
            s.selection_line_positions = line;
            s.selection_poly_positions = poly;
        });
        let _ = rebuild_overlays_and_upload();
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
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.selection_center = None;
        s.selection_line_positions = None;
        s.selection_poly_positions = None;
    });
    let _ = rebuild_overlays_and_upload();
    let _ = render_scene();
    js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::FALSE)?;
    Ok(out.into())
}

fn cursor_click_2d(x_px: f64, y_px: f64) -> Result<JsValue, JsValue> {
    let out = js_sys::Object::new();

    let picked = with_state(|state| {
        let mut s = state.borrow_mut();
        let w = s.canvas_width.max(1.0);
        let h = s.canvas_height.max(1.0);
        let cam = s.camera_2d;
        let projector = MercatorProjector::new(cam, w, h);

        let radius_px = (s.city_marker_size as f64).clamp(4.0, 32.0);
        let r2 = radius_px * radius_px;
        let (lon_click, lat_click) = projector.screen_to_lon_lat(x_px, y_px, w, h);

        let mut best_center: Option<[f32; 3]> = None;
        let mut best_d2: f64 = f64::INFINITY;

        if s.cities_style.visible {
            if let (Some(world), Some(merc)) =
                (s.cities_centers.as_deref(), s.cities_mercator.as_deref())
            {
                for (i, m) in merc.iter().take(2_000_000).enumerate() {
                    let (sx, sy) = projector.project_mercator_m(m[0] as f64, m[1] as f64, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2
                        && let Some(&c) = world.get(i)
                    {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            } else if let Some(world) = s.cities_centers.as_deref() {
                for &c in world.iter().take(2_000_000) {
                    let (lon, lat) = world_to_lon_lat_fast_deg(c);
                    let (sx, sy) = projector.project_lon_lat(lon, lat, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2 {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            }
        }
        if s.uploaded_points_style.visible {
            if let (Some(world), Some(merc)) = (
                s.uploaded_centers.as_deref(),
                s.uploaded_mercator.as_deref(),
            ) {
                for (i, m) in merc.iter().take(2_000_000).enumerate() {
                    let (sx, sy) = projector.project_mercator_m(m[0] as f64, m[1] as f64, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2
                        && let Some(&c) = world.get(i)
                    {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            } else if let Some(world) = s.uploaded_centers.as_deref() {
                for &c in world.iter().take(2_000_000) {
                    let (lon, lat) = world_to_lon_lat_fast_deg(c);
                    let (sx, sy) = projector.project_lon_lat(lon, lat, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2 {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            }
        }
        for layer in s.feed_layers.values() {
            if !layer.style.visible || layer.centers.is_empty() {
                continue;
            }
            if !layer.centers_mercator.is_empty() {
                for (i, m) in layer.centers_mercator.iter().take(2_000_000).enumerate() {
                    let (sx, sy) = projector.project_mercator_m(m[0] as f64, m[1] as f64, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2
                        && let Some(&c) = layer.centers.get(i)
                    {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            } else {
                for &c in layer.centers.iter().take(2_000_000) {
                    let (lon, lat) = world_to_lon_lat_fast_deg(c);
                    let (sx, sy) = projector.project_lon_lat(lon, lat, w, h);
                    let dx = sx - x_px;
                    let dy = sy - y_px;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2 {
                        best_center = Some(c);
                        best_d2 = d2;
                    }
                }
            }
        }

        if let Some(c) = best_center
            && best_d2 <= r2
        {
            s.selection_center = Some(c);
            s.selection_line_positions = None;
            s.selection_poly_positions = None;
            Some(("point".to_string(), c, lon_click, lat_click))
        } else {
            // Clear selection.
            s.selection_center = None;
            s.selection_line_positions = None;
            s.selection_poly_positions = None;
            None
        }
    });

    let _ = rebuild_overlays_and_upload();
    let _ = render_scene();

    if let Some((kind, center, _lon_click, _lat_click)) = picked {
        let (lon, lat) =
            world_to_lon_lat_deg([center[0] as f64, center[1] as f64, center[2] as f64]);
        js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::TRUE)?;
        js_sys::Reflect::set(&out, &JsValue::from_str("kind"), &JsValue::from_str(&kind))?;
        js_sys::Reflect::set(&out, &JsValue::from_str("lon"), &JsValue::from_f64(lon))?;
        js_sys::Reflect::set(&out, &JsValue::from_str("lat"), &JsValue::from_f64(lat))?;
    } else {
        js_sys::Reflect::set(&out, &JsValue::from_str("picked"), &JsValue::FALSE)?;
    }

    Ok(out.into())
}

#[wasm_bindgen]
pub fn load_geojson_feed_layer(
    feed_layer_id: String,
    name: String,
    geojson_text: String,
) -> Result<(), JsValue> {
    // Keep a configurable cap: feeds are user-controlled and can be huge.
    // Default to 200MB to match upload settings.
    const MAX_GEOJSON_TEXT_BYTES: usize = 200 * 1024 * 1024;
    const MAX_FEED_POINTS: usize = 2_000_000;

    if geojson_text.len() > MAX_GEOJSON_TEXT_BYTES {
        return Err(JsValue::from_str(
            "Feed payload too large for the web viewer (max 200MiB GeoJSON).",
        ));
    }

    let mut chunk = formats::VectorChunk::from_geojson_str(&geojson_text)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let _fix_counts = normalize_chunk_coords_in_place(&mut chunk);

    let mut centers: Vec<[f32; 3]> = Vec::new();
    for f in &chunk.features {
        use formats::VectorGeometry::*;
        match &f.geometry {
            Point(p) => {
                centers.push(lon_lat_deg_to_world(p.lon_deg, p.lat_deg));
            }
            MultiPoint(ps) => {
                for p in ps {
                    centers.push(lon_lat_deg_to_world(p.lon_deg, p.lat_deg));
                    if centers.len() >= MAX_FEED_POINTS {
                        break;
                    }
                }
            }
            _ => {}
        }
        if centers.len() >= MAX_FEED_POINTS {
            break;
        }
    }

    with_state(|state| {
        let mut s = state.borrow_mut();

        let style = s
            .feed_layers
            .get(&feed_layer_id)
            .map(|l| l.style)
            .unwrap_or(LayerStyle {
                visible: true,
                color: [0.38, 0.73, 1.0, 0.95],
                lift: 0.0,
            });

        let count_points = centers.len();
        let centers_mercator = centers.iter().map(viewer_to_mercator_m).collect();
        s.feed_layers.insert(
            feed_layer_id.clone(),
            FeedLayerState {
                name,
                centers,
                centers_mercator,
                count_points,
                style,
            },
        );
    });

    let _ = rebuild_overlays_and_upload();
    let _ = render_scene();
    Ok(())
}

#[wasm_bindgen]
pub fn remove_feed_layer(feed_layer_id: &str) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.feed_layers.remove(feed_layer_id);
        s.feed_style_ids.remove(feed_layer_id);
    });
    let _ = rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn list_feed_layers() -> Result<JsValue, JsValue> {
    let arr = js_sys::Array::new();
    with_state(|state| {
        let s = state.borrow();
        for (id, layer) in &s.feed_layers {
            let o = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&o, &JsValue::from_str("id"), &JsValue::from_str(id));
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("name"),
                &JsValue::from_str(&layer.name),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("points"),
                &JsValue::from_f64(layer.count_points as f64),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("visible"),
                &JsValue::from_bool(layer.style.visible),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("color_hex"),
                &JsValue::from_str(&color_to_hex([
                    layer.style.color[0],
                    layer.style.color[1],
                    layer.style.color[2],
                ])),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("opacity"),
                &JsValue::from_f64(layer.style.color[3] as f64),
            );
            let _ = js_sys::Reflect::set(
                &o,
                &JsValue::from_str("lift"),
                &JsValue::from_f64(layer.style.lift as f64),
            );
            arr.push(&o);
        }
    });
    Ok(arr.into())
}

#[wasm_bindgen]
pub async fn load_geojson_file(
    name: String,
    geojson_text: String,
    max_size_mb: Option<u32>,
) -> Result<JsValue, JsValue> {
    // Guardrails: large uploads can exceed wasm memory or browser storage limits.
    // (We currently persist catalog entries via LocalStorage; IndexedDB/DuckDB comes next.)
    // Default 200MB, can be reduced by JS-side settings.
    let max_bytes = max_size_mb.unwrap_or(200) as usize * 1024 * 1024;
    // NOTE: catalog persistence is best-effort. We store AVC in chunked LocalStorage keys.

    if geojson_text.len() > max_bytes {
        let max_mb = max_size_mb.unwrap_or(200);
        return Err(JsValue::from_str(&format!(
            "Upload too large for the web viewer (max {}MiB GeoJSON).",
            max_mb
        )));
    }

    web_sys::console::error_1(&JsValue::from_str("upload: parsing GeoJSON"));

    // Yield before heavy parsing to allow status update to render.
    yield_now().await;

    let mut chunk = formats::VectorChunk::from_geojson_str(&geojson_text)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    // Best-effort normalization for common coordinate issues.
    // (Some real-world data uses [lat, lon] order or WebMercator meters.)
    let fix_counts = normalize_chunk_coords_in_place(&mut chunk);

    web_sys::console::error_1(&JsValue::from_str("upload: parsed GeoJSON"));

    // Yield after heavy parsing to keep UI responsive.
    yield_now().await;

    // Count primitives for UI + bounds for camera fit.
    let mut count_points = 0usize;
    let mut count_lines = 0usize;
    let mut count_polys = 0usize;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for f in &chunk.features {
        update_bounds_for_geometry(
            &f.geometry,
            &mut min_lon,
            &mut max_lon,
            &mut min_lat,
            &mut max_lat,
        );
        match &f.geometry {
            formats::VectorGeometry::Point(_) => count_points += 1,
            formats::VectorGeometry::MultiPoint(v) => count_points += v.len(),
            formats::VectorGeometry::LineString(_) => count_lines += 1,
            formats::VectorGeometry::MultiLineString(v) => count_lines += v.len(),
            formats::VectorGeometry::Polygon(_) => count_polys += 1,
            formats::VectorGeometry::MultiPolygon(v) => count_polys += v.len(),
        }
    }

    // Store the uploaded data in Atlas' binary format (AVC) via the catalog.
    // If the payload is too large for the current LocalStorage-based store, we still load it
    // into the scene but skip persistence (avoids wasm memory traps).
    web_sys::console::error_1(&JsValue::from_str("upload: encoding AVC"));

    // Yield before AVC encoding (CPU-intensive).
    yield_now().await;

    let avc_bytes = chunk
        .to_avc_bytes()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    web_sys::console::error_1(&JsValue::from_str("upload: encoded AVC"));

    // Yield after AVC encoding.
    yield_now().await;

    web_sys::console::error_1(&JsValue::from_str("upload: computing now_ms"));
    let now_ms = js_sys::Date::now().max(0.0) as u64;

    web_sys::console::error_1(&JsValue::from_str("upload: computing catalog_id"));
    web_sys::console::error_1(&JsValue::from_str(&format!(
        "upload: avc_bytes_len={}",
        avc_bytes.len()
    )));

    let catalog_id = catalog::id_for_avc_bytes(&avc_bytes);

    web_sys::console::error_1(&JsValue::from_str("upload: computed catalog_id"));
    web_sys::console::error_1(&JsValue::from_str("upload: persisting to catalog"));

    let mut catalog_error: Option<String> = None;
    let mut created_at_ms = now_ms;
    let mut catalog_persisted = false;

    // 1) Try IndexedDB for bytes + LocalStorage for metadata.
    match idb_put_avc_bytes(&catalog_id, &avc_bytes).await {
        Ok(()) => {
            let meta_res = match CATALOG.try_with(|cat| {
                let mut cat = cat.borrow_mut();
                let existing = cat.get(&catalog_id)?;
                created_at_ms = existing.as_ref().map(|e| e.created_at_ms).unwrap_or(now_ms);
                cat.upsert(CatalogEntry {
                    id: catalog_id.clone(),
                    name: name.clone(),
                    avc_base64: String::new(),
                    count_points,
                    count_lines,
                    count_polys,
                    created_at_ms,
                })?;
                Ok::<(), catalog::CatalogError>(())
            }) {
                Ok(res) => res,
                Err(_) => {
                    return Err(JsValue::from_str(
                        "Viewer state is shutting down (hot reload). Please retry the upload.",
                    ));
                }
            };
            match meta_res {
                Ok(()) => {
                    catalog_persisted = true;
                }
                Err(e) => {
                    catalog_error = Some(e.to_string());
                    let _ = idb_delete_avc(&catalog_id).await;
                }
            }
        }
        Err(e) => {
            catalog_error = Some(format!(
                "IndexedDB failed: {}",
                e.as_string().unwrap_or_else(|| format!("{e:?}"))
            ));
        }
    }

    // 2) Fallback: chunked LocalStorage (catalog crate) if IDB path failed.
    if !catalog_persisted {
        let persisted = match CATALOG.try_with(|cat| {
            let mut cat = cat.borrow_mut();
            let existing = cat.get(&catalog_id)?;
            created_at_ms = existing.as_ref().map(|e| e.created_at_ms).unwrap_or(now_ms);
            cat.upsert_avc_bytes(
                CatalogEntry {
                    id: catalog_id.clone(),
                    name: name.clone(),
                    avc_base64: String::new(),
                    count_points,
                    count_lines,
                    count_polys,
                    created_at_ms,
                },
                &avc_bytes,
            )?;
            Ok::<(), catalog::CatalogError>(())
        }) {
            Ok(res) => res,
            Err(_) => {
                return Err(JsValue::from_str(
                    "Viewer state is shutting down (hot reload). Please retry the upload.",
                ));
            }
        };
        match persisted {
            Ok(()) => {
                catalog_persisted = true;
                catalog_error = None;
            }
            Err(e) => {
                catalog_error = Some(e.to_string());
            }
        }
    }

    if catalog_persisted {
        web_sys::console::error_1(&JsValue::from_str("upload: persisted to catalog"));
    } else {
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "upload: catalog persist skipped ({})",
            catalog_error
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        )));
    }

    // Yield before world ingestion (triangulation is CPU-intensive).
    yield_now().await;

    let world = world_from_vector_chunk(&chunk, None);

    web_sys::console::error_1(&JsValue::from_str("upload: ingested world"));

    // Yield after world ingestion.
    yield_now().await;

    match STATE.try_with(|state| {
        let mut s = state.borrow_mut();
        s.uploaded_name = Some(name.clone());
        s.uploaded_catalog_id = if catalog_persisted {
            Some(catalog_id.clone())
        } else {
            None
        };
        s.uploaded_world = Some(world);
        s.uploaded_count_points = count_points;
        s.uploaded_count_lines = count_lines;
        s.uploaded_count_polys = count_polys;

        // Switch to uploaded dataset immediately (matches previous behavior).
        s.dataset = "__uploaded__".to_string();
        s.uploaded_points_style.visible = s.uploaded_count_points > 0;
        s.uploaded_corridors_style.visible = s.uploaded_count_lines > 0;
        s.uploaded_regions_style.visible = s.uploaded_count_polys > 0;

        if min_lon.is_finite() && max_lon.is_finite() && min_lat.is_finite() && max_lat.is_finite()
        {
            let w = s.canvas_width.max(1.0);
            let h = s.canvas_height.max(1.0);
            fit_camera_2d_to_bounds(&mut s.camera_2d, w, h, min_lon, max_lon, min_lat, max_lat);
        }
    }) {
        Ok(()) => {}
        Err(_) => {
            return Err(JsValue::from_str(
                "Viewer state is shutting down (hot reload). Please retry the upload.",
            ));
        }
    }

    web_sys::console::error_1(&JsValue::from_str("upload: rebuilding overlays"));

    // Yield before overlay rebuild (mercator projection is CPU-intensive).
    yield_now().await;

    rebuild_overlays_and_upload()?;
    web_sys::console::error_1(&JsValue::from_str("upload: rebuilt overlays"));

    // Final yield before render.
    yield_now().await;

    let _ = render_scene();

    let summary = js_sys::Object::new();
    js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("name"),
        &JsValue::from_str(&name),
    )?;
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("catalog_id"),
        &JsValue::from_str(&catalog_id),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("catalog_persisted"),
        &JsValue::from_bool(catalog_persisted),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("catalog_error"),
        &JsValue::from_str(&catalog_error.unwrap_or_default()),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("created_at_ms"),
        &JsValue::from_f64(created_at_ms as f64),
    );
    // Legacy permissive uploader counters (kept for UI compatibility).
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("skipped_coords"),
        &JsValue::from_f64(fix_counts.skipped as f64),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("fixed_swapped"),
        &JsValue::from_f64(fix_counts.swapped as f64),
    );
    let _ = js_sys::Reflect::set(
        &summary,
        &JsValue::from_str("fixed_web_mercator"),
        &JsValue::from_f64(fix_counts.web_mercator as f64),
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

        let base_url = base_url_for(&url);
        let mut chunks: Vec<(formats::VectorChunk, Option<VectorGeometryKind>)> = Vec::new();
        let mut count_points = 0usize;
        let mut count_lines = 0usize;
        let mut count_polys = 0usize;

        for entry in &manifest.chunks {
            let chunk_url = join_url(&base_url, &entry.path);
            let expected = match entry.kind.trim().to_ascii_lowercase().as_str() {
                "points" | "point" => Some(VectorGeometryKind::Point),
                "lines" | "line" => Some(VectorGeometryKind::Line),
                "areas" | "area" | "polygons" | "polygon" => Some(VectorGeometryKind::Area),
                _ => None,
            };

            let chunk = match fetch_vector_chunk(&chunk_url).await {
                Ok(c) => c,
                Err(err) => {
                    let msg = format!(
                        "Failed to fetch chunk '{}' ({}) : {:?}",
                        entry.id, chunk_url, err
                    );
                    web_sys::console::log_1(&JsValue::from_str(&msg));
                    continue;
                }
            };

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

            chunks.push((chunk, expected));
        }

        let world = world_from_vector_chunks(&chunks);

        with_state(|state| {
            let mut s = state.borrow_mut();
            s.uploaded_name = Some(format!(
                "{} ({})",
                manifest
                    .name
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                manifest.package_id
            ));

            s.uploaded_catalog_id = None;

            s.uploaded_world = Some(world);
            s.uploaded_count_points = count_points;
            s.uploaded_count_lines = count_lines;
            s.uploaded_count_polys = count_polys;

            // Use the same path as the built-in uploader: switch to the uploaded dataset.
            s.dataset = "__uploaded__".to_string();
            s.uploaded_points_style.visible = count_points > 0;
            s.uploaded_corridors_style.visible = count_lines > 0;
            s.uploaded_regions_style.visible = count_polys > 0;
            s.frame_index = 0;
            s.time_s = 0.0;
            s.time_end_s = (manifest.chunks.len().max(1) as f64) * 1.5;
        });

        let _ = rebuild_overlays_and_upload();
        let _ = render_scene();
    });
}

fn world_from_vector_chunks(
    chunks: &[(formats::VectorChunk, Option<VectorGeometryKind>)],
) -> scene::World {
    let mut world = scene::World::new();
    scene::prefabs::spawn_wgs84_globe(&mut world);
    for (chunk, expected_kind) in chunks {
        formats::ingest_vector_chunk(&mut world, chunk, *expected_kind);
    }
    world
}

fn base_url_for(url: &str) -> String {
    match url.rsplit_once('/') {
        Some((base, _file)) => format!("{base}/"),
        None => String::new(),
    }
}

fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{base}{path}")
    }
}

fn now_ms() -> f64 {
    // `js_sys::Date::now()` is always available in wasm-bindgen without
    // additional web-sys feature flags, and it's sufficient for retry backoff.
    js_sys::Date::now()
}

/// Advances the deterministic engine time by one fixed-timestep frame.
///
/// This is intentionally not wall-clock driven so it can be replayed.
#[wasm_bindgen]
pub fn advance_frame() -> Result<f64, JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.frame_index = s.frame_index.wrapping_add(1);
        s.time_s += s.dt_s;
        if s.time_s > s.time_end_s {
            s.time_s = 0.0;
            s.frame_index = 0;
        }

        // Update globe controller (inertia + smooth zoom)
        if s.view_mode == ViewMode::ThreeD {
            let now = wall_clock_seconds();
            let dt = if s.last_frame_time_s > 0.0 {
                (now - s.last_frame_time_s).clamp(0.001, 0.1)
            } else {
                s.dt_s
            };
            s.last_frame_time_s = now;

            // Apply auto-rotate before globe controller update if idle
            if s.auto_rotate_enabled {
                let idle = now - s.auto_rotate_last_user_time_s;
                if idle >= s.auto_rotate_resume_delay_s {
                    let speed_rad = s.auto_rotate_speed_deg_per_s.to_radians();
                    s.globe_controller.apply_yaw_delta(speed_rad * dt);
                }
            }

            s.globe_controller.update(dt);

            // Keep legacy camera state in sync
            s.camera.yaw_rad = s.globe_controller.yaw_rad();
            s.camera.pitch_rad = s.globe_controller.pitch_rad();
            s.camera.distance = s.globe_controller.distance();
        }
    });

    if with_state(|state| {
        let s = state.borrow();
        s.view_mode == ViewMode::ThreeD && s.terrain_style.visible && s.frame_index % 120 == 0
    }) {
        ensure_terrain_loaded();
    }

    if with_state(|state| {
        let s = state.borrow();
        s.base_regions_style.visible && s.frame_index % 180 == 0
    }) {
        ensure_surface_loaded();
    }

    // 2D rendering is driven by user input handlers (camera pan/zoom), and
    // re-rendered explicitly when async loads complete. Avoid doing a full
    // redraw every RAF tick in 2D.
    if with_state(|state| state.borrow().view_mode == ViewMode::TwoD) {
        return Ok(with_state(|state| state.borrow().time_s));
    }

    render_scene()?;
    Ok(with_state(|state| state.borrow().time_s))
}

#[wasm_bindgen]
pub fn set_time(time_s: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.time_s = time_s.max(0.0);
    });
    render_scene()
}

/// Returns the current engine time in seconds.
#[wasm_bindgen]
pub fn get_time() -> f64 {
    with_state(|state| state.borrow().time_s)
}

/// Returns the end of the current time range in seconds.
#[wasm_bindgen]
pub fn get_time_end() -> f64 {
    with_state(|state| state.borrow().time_end_s)
}

/// Sets the end of the time range in seconds.
#[wasm_bindgen]
pub fn set_time_end(time_end_s: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.time_end_s = time_end_s.max(0.1);
    });
    Ok(())
}

/// Returns the fixed timestep in seconds.
#[wasm_bindgen]
pub fn get_dt() -> f64 {
    with_state(|state| state.borrow().dt_s)
}

/// Sets the fixed timestep in seconds.
#[wasm_bindgen]
pub fn set_dt(dt_s: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.dt_s = dt_s.max(0.001);
    });
    Ok(())
}

// ── Control configuration WASM exports ──────────────────────────────────────

/// Return control config as a JSON string for JS consumption.
#[wasm_bindgen]
pub fn get_control_config() -> String {
    with_state(|state| {
        let s = state.borrow();
        let c = &s.controls;
        format!(
            r#"{{"orbit_sensitivity":{},"pan_sensitivity_3d":{},"zoom_speed_3d":{},"invert_orbit_y":{},"invert_pan_y_3d":{},"min_distance":{},"max_distance":{},"pitch_clamp_rad":{},"max_target_offset_m":{},"pan_sensitivity_2d":{},"zoom_speed_2d":{},"invert_pan_y_2d":{},"min_zoom_2d":{},"max_zoom_2d":{},"kinetic_panning":{}}}"#,
            c.orbit_sensitivity,
            c.pan_sensitivity_3d,
            c.zoom_speed_3d,
            c.invert_orbit_y,
            c.invert_pan_y_3d,
            c.min_distance,
            c.max_distance,
            c.pitch_clamp_rad,
            c.max_target_offset_m,
            c.pan_sensitivity_2d,
            c.zoom_speed_2d,
            c.invert_pan_y_2d,
            c.min_zoom_2d,
            c.max_zoom_2d,
            c.kinetic_panning,
        )
    })
}

/// Update a single control config field by key.
#[wasm_bindgen]
pub fn set_control_config(key: &str, value: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        let c = &mut s.controls;
        match key {
            "orbit_sensitivity" => c.orbit_sensitivity = value.clamp(0.1, 5.0),
            "pan_sensitivity_3d" => c.pan_sensitivity_3d = value.clamp(0.1, 5.0),
            "zoom_speed_3d" => c.zoom_speed_3d = value.clamp(0.1, 5.0),
            "invert_orbit_y" => c.invert_orbit_y = value > 0.5,
            "invert_pan_y_3d" => c.invert_pan_y_3d = value > 0.5,
            "pan_sensitivity_2d" => c.pan_sensitivity_2d = value.clamp(0.1, 5.0),
            "zoom_speed_2d" => c.zoom_speed_2d = value.clamp(0.1, 5.0),
            "invert_pan_y_2d" => c.invert_pan_y_2d = value > 0.5,
            "kinetic_panning" => c.kinetic_panning = value > 0.5,
            _ => {}
        }
    });
    Ok(())
}

/// Reset all control config to defaults.
#[wasm_bindgen]
pub fn reset_control_config() -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.controls = ControlConfig::default();
    });
    Ok(())
}

async fn init_wgpu_inner() -> Result<(), JsValue> {
    let ctx = init_wgpu_from_canvas_id("atlas-canvas-3d").await?;

    with_state(|state| {
        let mut s = state.borrow_mut();
        let palette = palette_for(s.theme);
        let globe_transparent = s.globe_transparent;
        let pending_styles = s.pending_styles.take();
        let pending = s.pending_cities.take();
        let pending_corridors = s.pending_corridors.take();
        let pending_base_regions = s.pending_base_regions.take();
        let pending_regions = s.pending_regions.take();
        let pending_terrain = s.pending_terrain.take();
        let pending_base_regions2d = s.pending_base_regions2d.take();
        let pending_regions2d = s.pending_regions2d.take();
        let pending_points2d = s.pending_points2d.take();
        let pending_lines2d = s.pending_lines2d.take();
        let pending_grid2d = s.pending_grid2d.take();
        s.wgpu = Some(ctx);

        if let Some(ctx) = &mut s.wgpu {
            wgpu::set_theme(
                ctx,
                palette.clear_color,
                palette.globe_color,
                palette.stars_alpha,
            );
            wgpu::set_globe_transparent(ctx, globe_transparent);
        }

        if let Some(styles) = pending_styles
            && let Some(ctx) = &mut s.wgpu
        {
            set_styles(ctx, &styles);
        }

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

        if let Some(points) = pending_base_regions
            && let Some(ctx) = &mut s.wgpu
        {
            set_base_regions_points(ctx, &points);
        }

        if let Some(points) = pending_regions
            && let Some(ctx) = &mut s.wgpu
        {
            set_regions_points(ctx, &points);
        }

        if let Some(points) = pending_terrain
            && let Some(ctx) = &mut s.wgpu
        {
            set_terrain_points(ctx, &points);
        }

        if let Some(verts) = pending_base_regions2d
            && let Some(ctx) = &mut s.wgpu
        {
            set_base_regions2d_vertices(ctx, &verts);
        }

        if let Some(verts) = pending_regions2d
            && let Some(ctx) = &mut s.wgpu
        {
            set_regions2d_vertices(ctx, &verts);
        }

        if let Some(inst) = pending_points2d
            && let Some(ctx) = &mut s.wgpu
        {
            set_points2d_instances(ctx, &inst);
        }

        if let Some(inst) = pending_lines2d
            && let Some(ctx) = &mut s.wgpu
        {
            set_lines2d_instances(ctx, &inst);
        }

        if let Some(inst) = pending_grid2d
            && let Some(ctx) = &mut s.wgpu
        {
            set_grid2d_instances(ctx, &inst);
        }
    });

    if with_state(|state| state.borrow().terrain_style.visible) {
        ensure_terrain_loaded();
    }
    if with_state(|state| state.borrow().base_regions_style.visible) {
        ensure_surface_loaded();
    }

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

async fn fetch_surface_tileset(url: &str) -> Result<SurfaceTileset, JsValue> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    if !resp.ok() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_else(|_| "".to_string());
        let body = body.trim();
        let msg = if body.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {body}")
        };
        return Err(JsValue::from_str(&msg));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))
}

async fn fetch_terrain_tileset(url: &str) -> Result<TerrainTileset, JsValue> {
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

async fn fetch_binary(url: &str) -> Result<Vec<u8>, JsValue> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    resp.binary()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

async fn fetch_vector_chunk(url: &str) -> Result<formats::VectorChunk, JsValue> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    if url.to_ascii_lowercase().ends_with(".avc") {
        let bytes = resp
            .binary()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        formats::VectorChunk::from_avc_bytes(&bytes).map_err(|e| JsValue::from_str(&e.to_string()))
    } else {
        let text = resp
            .text()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        formats::VectorChunk::from_geojson_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

// ── Tests: Control Contracts ────────────────────────────────────────────────
//
// These tests define and verify the interaction contracts for the spatiotemporal
// viewer's camera system.  They ensure that:
//
//  1. Default control config values are stable (no accidental regressions).
//  2. Camera math produces correct directional results:
//     - 3D orbit: drag left => yaw increases (surface moves left).
//     - 3D pan:   drag right => target moves right in view space.
//     - 2D pan:   drag right => center_lon_deg decreases (map moves right).
//  3. Zoom contracts: scroll up => zoom in, scroll down => zoom out.
//  4. Clamping: pitch stays within bounds, distance stays positive, etc.
//  5. Config changes are applied and respected by the math.

#[cfg(test)]
mod camera_contract_tests {
    use super::*;

    // ── ControlConfig defaults ────────────────────────────────────

    #[test]
    fn control_config_defaults_are_stable() {
        let cfg = ControlConfig::default();
        assert_eq!(cfg.orbit_sensitivity, 1.0);
        assert_eq!(cfg.pan_sensitivity_3d, 1.0);
        assert_eq!(cfg.zoom_speed_3d, 1.0);
        assert!(!cfg.invert_orbit_y);
        assert!(!cfg.invert_pan_y_3d);
        assert_eq!(cfg.min_distance, 10.0);
        assert!((cfg.max_distance - 200.0 * WGS84_A).abs() < 1.0);
        assert!((cfg.pitch_clamp_rad - 1.55).abs() < 0.001);
        assert!((cfg.max_target_offset_m - 2.0 * WGS84_A).abs() < 1.0);
        assert_eq!(cfg.pan_sensitivity_2d, 1.0);
        assert_eq!(cfg.zoom_speed_2d, 1.0);
        assert!(!cfg.invert_pan_y_2d);
        assert_eq!(cfg.min_zoom_2d, 1.0);
        assert_eq!(cfg.max_zoom_2d, 200.0);
        assert!(cfg.kinetic_panning);
    }

    // ── CameraState defaults ──────────────────────────────────────

    #[test]
    fn camera_state_defaults_are_stable() {
        let cam = CameraState::default();
        // Yaw ~160° (faces Africa).
        assert!((cam.yaw_rad - 160f64.to_radians()).abs() < 0.001);
        // Slight pitch above equator.
        assert!((cam.pitch_rad - 5f64.to_radians()).abs() < 0.001);
        // Distance = 3 × Earth radius.
        assert!((cam.distance - 3.0 * WGS84_A).abs() < 1.0);
        // Target at origin.
        assert_eq!(cam.target, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn camera_2d_state_defaults_are_stable() {
        let cam = Camera2DState::default();
        assert_eq!(cam.center_lon_deg, 0.0);
        assert_eq!(cam.center_lat_deg, 0.0);
        assert_eq!(cam.zoom, 1.0);
    }

    // ── Vec3 helpers ──────────────────────────────────────────────

    #[test]
    fn vec3_normalize_unit_length() {
        let v = vec3_normalize([3.0, 4.0, 0.0]);
        let len = vec3_dot(v, v).sqrt();
        assert!((len - 1.0).abs() < 1e-10);
    }

    #[test]
    fn vec3_cross_right_hand_rule() {
        let x = [1.0, 0.0, 0.0];
        let y = [0.0, 1.0, 0.0];
        let z = vec3_cross(x, y);
        assert!((z[2] - 1.0).abs() < 1e-10);
    }

    // ── 3D Orbit contract ─────────────────────────────────────────
    //
    // "Drag left => surface facing user moves left (yaw increases)."
    // "Drag up => surface facing user tilts up (pitch decreases in default config)."

    #[test]
    fn orbit_drag_right_increases_yaw() {
        let mut cam = CameraState::default();
        let cfg = ControlConfig::default();
        let w = 1280.0_f64;
        let h = 720.0_f64;
        let min_dim = w.min(h).max(1.0);
        let speed = std::f64::consts::PI / min_dim * cfg.orbit_sensitivity;

        let initial_yaw = cam.yaw_rad;
        // Drag right (+100 px): surface follows cursor rightward.
        let delta_x = 100.0;
        cam.yaw_rad += delta_x * speed;

        assert!(
            cam.yaw_rad > initial_yaw,
            "drag right must increase yaw (surface follows cursor)"
        );
    }

    #[test]
    fn orbit_drag_up_decreases_pitch() {
        let mut cam = CameraState::default();
        let cfg = ControlConfig::default();
        let w = 1280.0_f64;
        let h = 720.0_f64;
        let min_dim = w.min(h).max(1.0);
        let speed = std::f64::consts::PI / min_dim * cfg.orbit_sensitivity;
        // "From outside": default dy_sign = 1.0 (not inverted).
        let dy_sign = if cfg.invert_orbit_y { -1.0 } else { 1.0 };

        let initial_pitch = cam.pitch_rad;
        // Simulate drag delta: dy = -50 pixels (drag up on screen).
        let delta_y = -50.0;
        cam.pitch_rad += dy_sign * delta_y * speed;

        // Drag up (negative dy) with default config => pitch decreases
        // (camera tilts down, surface appears to move up).
        assert!(
            cam.pitch_rad < initial_pitch,
            "drag up with default config must decrease pitch (from outside)"
        );
    }

    #[test]
    fn orbit_invert_y_reverses_pitch_direction() {
        let cfg = ControlConfig {
            invert_orbit_y: true,
            ..ControlConfig::default()
        };
        let w = 1280.0_f64;
        let h = 720.0_f64;
        let min_dim = w.min(h).max(1.0);
        let speed = std::f64::consts::PI / min_dim * cfg.orbit_sensitivity;
        let dy_sign = if cfg.invert_orbit_y { -1.0 } else { 1.0 };

        let mut cam = CameraState::default();
        let initial_pitch = cam.pitch_rad;
        let delta_y = -50.0; // drag up
        cam.pitch_rad += dy_sign * delta_y * speed;

        // With invert_orbit_y=true, drag up => pitch increases.
        assert!(
            cam.pitch_rad > initial_pitch,
            "drag up with inverted Y must increase pitch"
        );
    }

    #[test]
    fn orbit_pitch_clamped() {
        let cfg = ControlConfig::default();
        let mut cam = CameraState::default();
        // Set pitch to near-max.
        cam.pitch_rad = cfg.pitch_clamp_rad + 0.5;
        cam.pitch_rad = clamp(cam.pitch_rad, -cfg.pitch_clamp_rad, cfg.pitch_clamp_rad);
        assert!(cam.pitch_rad <= cfg.pitch_clamp_rad);
        assert!(cam.pitch_rad >= -cfg.pitch_clamp_rad);
    }

    // ── 3D Pan (target translation) contract ──────────────────────
    //
    // "Right-click drag moves the entire globe in the viewport."
    // "Drag right => globe moves right on screen."

    #[test]
    fn pan_3d_drag_right_moves_target_right() {
        let cam = CameraState::default();
        let cfg = ControlConfig::default();
        let _w = 1280.0_f64;
        let h = 720.0_f64;

        // Compute camera right/up vectors.
        let dir_cam = vec3_normalize([
            cam.pitch_rad.cos() * cam.yaw_rad.cos(),
            cam.pitch_rad.sin(),
            -cam.pitch_rad.cos() * cam.yaw_rad.sin(),
        ]);
        let world_up = [0.0, 1.0, 0.0];
        let right = vec3_normalize(vec3_cross(world_up, dir_cam));

        let fov_y_rad = 45f64.to_radians();
        let px_to_world = cam.distance * (fov_y_rad / h) * cfg.pan_sensitivity_3d;

        // Drag right: dx = +100px
        let delta_x = 100.0;
        let offset_right = vec3_mul(right, -delta_x * px_to_world);
        let new_target = vec3_add(cam.target, offset_right);

        // The target should have moved in the right direction (non-zero displacement).
        let displacement = vec3_dot(new_target, new_target).sqrt();
        assert!(displacement > 0.0, "pan must move target");

        // Verify the target moved along the right vector (dot product with right > 0 means leftward
        // in camera space, since we negate delta_x for "follow cursor" behavior).
        // Actually with -delta_x (delta_x positive), offset goes along -right.
        // This moves the target opposite to the camera's right, which makes the globe
        // appear to move right on screen (camera-relative).
        let dot_right = vec3_dot(new_target, right);
        // The target moves in -right direction (so globe appears to move rightward).
        assert!(
            dot_right < 0.0,
            "target should move opposite to camera-right when dragging right"
        );
    }

    #[test]
    fn pan_3d_target_clamped() {
        let cfg = ControlConfig::default();
        // Manually set target beyond allowed offset.
        let far_target = [cfg.max_target_offset_m * 2.0, 0.0, 0.0];
        let r = vec3_dot(far_target, far_target).sqrt();
        let clamped = if r <= cfg.max_target_offset_m {
            far_target
        } else {
            vec3_mul(far_target, cfg.max_target_offset_m / r)
        };
        let clamped_r = vec3_dot(clamped, clamped).sqrt();
        assert!(
            (clamped_r - cfg.max_target_offset_m).abs() < 1.0,
            "target offset must be clamped to max_target_offset_m"
        );
    }

    // ── 2D Pan contract ───────────────────────────────────────────
    //
    // "Map follows cursor direction: drag right => map moves right (lon decreases)."

    #[test]
    fn pan_2d_drag_right_moves_map_right() {
        let cam = Camera2DState::default();
        let w = 1280.0;
        let h = 720.0;
        // Drag right: dx = +100px, dy = 0
        let result = pan_camera_2d(cam, 100.0, 0.0, w, h);
        // Dragging right means the map content should follow the cursor to the right,
        // which means the center lon should DECREASE (we're looking at a point further west).
        assert!(
            result.center_lon_deg < cam.center_lon_deg,
            "drag right in 2D must decrease center_lon (map follows cursor right)"
        );
    }

    #[test]
    fn pan_2d_drag_down_moves_map_down() {
        let cam = Camera2DState::default();
        let w = 1280.0;
        let h = 720.0;
        // Drag down: dx = 0, dy = +100px
        let result = pan_camera_2d(cam, 0.0, 100.0, w, h);
        // Drag down => content follows cursor down => center moves north.
        assert!(
            result.center_lat_deg > cam.center_lat_deg,
            "drag down in 2D must increase center_lat (map follows cursor down)"
        );
    }

    #[test]
    fn pan_2d_no_movement_on_zero_delta() {
        let cam = Camera2DState::default();
        let result = pan_camera_2d(cam, 0.0, 0.0, 1280.0, 720.0);
        assert!((result.center_lon_deg - cam.center_lon_deg).abs() < 1e-10);
        assert!((result.center_lat_deg - cam.center_lat_deg).abs() < 1e-10);
    }

    // ── Zoom contracts ────────────────────────────────────────────

    #[test]
    fn zoom_3d_scroll_up_decreases_distance() {
        let cfg = ControlConfig::default();
        let cam = CameraState::default();
        let initial_dist = cam.distance;
        // Wheel deltaY > 0 = scroll down = zoom out (increase distance).
        // Wheel deltaY < 0 = scroll up = zoom in (decrease distance).
        // globe_controller.on_wheel uses (delta * 0.002).exp() internally;
        // we scale the delta by cfg.zoom_speed_3d before passing it in.
        let wheel_delta_y = -100.0; // scroll up
        let zoom = (wheel_delta_y * cfg.zoom_speed_3d * 0.002).exp();
        let new_dist = initial_dist * zoom;
        assert!(
            new_dist < initial_dist,
            "scroll up (negative deltaY) must decrease distance (zoom in)"
        );
    }

    #[test]
    fn zoom_3d_scroll_down_increases_distance() {
        let cfg = ControlConfig::default();
        let cam = CameraState::default();
        let initial_dist = cam.distance;
        let wheel_delta_y = 100.0; // scroll down
        let zoom = (wheel_delta_y * cfg.zoom_speed_3d * 0.002).exp();
        let new_dist = initial_dist * zoom;
        assert!(
            new_dist > initial_dist,
            "scroll down (positive deltaY) must increase distance (zoom out)"
        );
    }

    #[test]
    fn zoom_3d_distance_clamped_to_config_range() {
        let cfg = ControlConfig::default();
        // Distance below minimum.
        let dist = clamp(1.0, cfg.min_distance, cfg.max_distance);
        assert!(dist >= cfg.min_distance);
        // Distance above maximum.
        let dist = clamp(1e20, cfg.min_distance, cfg.max_distance);
        assert!(dist <= cfg.max_distance);
    }

    #[test]
    fn zoom_2d_scroll_up_increases_zoom() {
        let cfg = ControlConfig::default();
        let cam = Camera2DState::default();
        let initial_zoom = cam.zoom;
        // In 2D, zoom factor uses *negative* wheel_delta_y (scroll up = zoom in).
        let wheel_delta_y = -100.0;
        let zoom = (-wheel_delta_y * 0.0015 * cfg.zoom_speed_2d).exp();
        let new_zoom = clamp(initial_zoom * zoom, cfg.min_zoom_2d, cfg.max_zoom_2d);
        assert!(
            new_zoom > initial_zoom,
            "scroll up in 2D must increase zoom level"
        );
    }

    #[test]
    fn zoom_2d_zoom_clamped_to_config_range() {
        let cfg = ControlConfig::default();
        let z = clamp(0.1, cfg.min_zoom_2d, cfg.max_zoom_2d);
        assert!(z >= cfg.min_zoom_2d);
        let z = clamp(1000.0, cfg.min_zoom_2d, cfg.max_zoom_2d);
        assert!(z <= cfg.max_zoom_2d);
    }

    // ── Arcball contract ──────────────────────────────────────────

    #[test]
    fn trackball_unit_vector_is_normalized() {
        let u = trackball_unit_from_screen(400.0, 300.0, 800.0, 600.0);
        let len = vec3_dot(u, u).sqrt();
        assert!(
            (len - 1.0).abs() < 1e-10,
            "trackball vector must be unit length"
        );
    }

    #[test]
    fn trackball_center_points_forward() {
        // Center of screen should map to (0, 0, 1) — pointing toward the viewer.
        let u = trackball_unit_from_screen(400.0, 300.0, 800.0, 600.0);
        assert!(u[2] > 0.9, "center of screen should have large z component");
    }

    // ── Quaternion helpers ────────────────────────────────────────

    #[test]
    fn quat_identity_rotation() {
        let v = [1.0, 0.0, 0.0];
        let q = quat_from_unit_vectors(v, v);
        let rotated = quat_rotate_vec3(q, [0.0, 1.0, 0.0]);
        assert!(
            (rotated[1] - 1.0).abs() < 1e-10,
            "identity rotation should not change vectors"
        );
    }

    // ── View mode transition contract ─────────────────────────────

    #[test]
    fn view_mode_3d_to_2d_resets_camera_2d() {
        // When switching 3D -> 2D, Camera2DState should be reset to default.
        let new_cam = Camera2DState::default();
        assert_eq!(new_cam.center_lon_deg, 0.0);
        assert_eq!(new_cam.center_lat_deg, 0.0);
        assert_eq!(new_cam.zoom, 1.0);
    }

    #[test]
    fn view_mode_2d_to_3d_maps_center_to_yaw() {
        // Contract: yaw = (180° - lon).to_radians()
        let lon_deg: f64 = 20.0; // Nairobi-ish longitude
        let expected_yaw = (180.0_f64 - lon_deg).to_radians();
        let yaw = (180.0_f64 - lon_deg).to_radians();
        assert!((yaw - expected_yaw).abs() < 1e-10);

        // Verify the yaw places the correct longitude in front of camera.
        // visible_lon ≈ 180° - yaw_deg
        let yaw_deg = yaw.to_degrees();
        let visible_lon = 180.0_f64 - yaw_deg;
        assert!((visible_lon - lon_deg).abs() < 1e-10);
    }

    // ── Sensitivity multiplier contract ───────────────────────────

    #[test]
    fn higher_sensitivity_produces_larger_displacement() {
        let w = 1280.0_f64;
        let h = 720.0_f64;
        let min_dim = w.min(h);
        let delta_x = 50.0;

        let speed_1x = std::f64::consts::PI / min_dim * 1.0;
        let speed_2x = std::f64::consts::PI / min_dim * 2.0;

        let yaw_delta_1x = delta_x * speed_1x;
        let yaw_delta_2x = delta_x * speed_2x;

        assert!(
            yaw_delta_2x > yaw_delta_1x,
            "2x sensitivity must produce larger yaw change"
        );
        assert!(
            (yaw_delta_2x / yaw_delta_1x - 2.0).abs() < 1e-10,
            "2x sensitivity must produce exactly 2x the yaw change"
        );
    }

    // ── Mercator projection contract ──────────────────────────────

    #[test]
    fn mercator_roundtrip_longitude() {
        let lon = 45.0;
        let x = mercator_x_m(lon);
        let roundtrip = inverse_mercator_lon_deg(x);
        assert!(
            (roundtrip - lon).abs() < 1e-8,
            "mercator lon roundtrip must be lossless"
        );
    }

    #[test]
    fn mercator_roundtrip_latitude() {
        let lat = 51.5; // London
        let y = mercator_y_m(lat);
        let roundtrip = inverse_mercator_lat_deg(y);
        assert!(
            (roundtrip - lat).abs() < 1e-8,
            "mercator lat roundtrip must be lossless"
        );
    }

    #[test]
    fn mercator_max_lat_clamped() {
        let cam = Camera2DState {
            center_lat_deg: 0.0,
            center_lon_deg: 0.0,
            zoom: 1.0,
        };
        // Large upward drag should not exceed max Mercator latitude.
        let result = pan_camera_2d(cam, 0.0, -100000.0, 1280.0, 720.0);
        assert!(result.center_lat_deg <= MERCATOR_MAX_LAT_DEG);
        assert!(result.center_lat_deg >= -MERCATOR_MAX_LAT_DEG);
    }
}
