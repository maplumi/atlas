use gloo_net::http::Request;
use serde::Deserialize;
use std::cell::RefCell;
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

use catalog::{CatalogEntry, CatalogStore};
use formats::SceneManifest;
use foundation::math::{Geodetic, WGS84_A, WGS84_B, ecef_to_geodetic, geodetic_to_ecef};
use layers::symbology::LayerStyle;
use layers::vector::VectorLayer;
use scene::components::VectorGeometryKind;
mod wgpu;
use wgpu::{
    CityVertex, CorridorVertex, OverlayVertex, WgpuContext, init_wgpu_from_canvas_id, render_mesh,
    resize_wgpu, set_base_regions_points, set_cities_points, set_corridors_points,
    set_regions_points, set_terrain_points,
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
    corridors_positions: Option<Vec<[f32; 3]>>,
    regions_positions: Option<Vec<[f32; 3]>>,

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
    terrain_vertices: Option<Vec<OverlayVertex>>,
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
    uploaded_corridors_positions: Option<Vec<[f32; 3]>>,
    uploaded_regions_positions: Option<Vec<[f32; 3]>>,
    uploaded_count_points: usize,
    uploaded_count_lines: usize,
    uploaded_count_polys: usize,

    base_count_polys: usize,
    cities_count_points: usize,
    corridors_count_lines: usize,
    regions_count_polys: usize,

    selection_center: Option<[f32; 3]>,
    selection_line_positions: Option<Vec<[f32; 3]>>,
    selection_poly_positions: Option<Vec<[f32; 3]>>,

    // Combined buffers (all visible layers), uploaded into the shared GPU buffers.
    pending_cities: Option<Vec<CityVertex>>,
    pending_corridors: Option<Vec<CorridorVertex>>,
    pending_base_regions: Option<Vec<OverlayVertex>>,
    pending_regions: Option<Vec<OverlayVertex>>,
    pending_terrain: Option<Vec<OverlayVertex>>,
    frame_index: u64,
    dt_s: f64,
    time_s: f64,
    time_end_s: f64,
    camera: CameraState,
    camera_2d: Camera2DState,
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
        corridors_positions: None,
        regions_positions: None,

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
        uploaded_corridors_positions: None,
        uploaded_regions_positions: None,
        uploaded_count_points: 0,
        uploaded_count_lines: 0,
        uploaded_count_polys: 0,

        base_count_polys: 0,
        cities_count_points: 0,
        corridors_count_lines: 0,
        regions_count_polys: 0,

        selection_center: None,
        selection_line_positions: None,
        selection_poly_positions: None,
        pending_cities: None,
        pending_corridors: None,
        pending_base_regions: None,
        pending_regions: None,
        pending_terrain: None,
        frame_index: 0,
        dt_s: 1.0 / 60.0,
        time_s: 0.0,
        time_end_s: 10.0,
        camera: CameraState::default(),
        camera_2d: Camera2DState::default(),
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

fn mercator_x_m(lon_deg: f64) -> f64 {
    WGS84_A * lon_deg.to_radians()
}

fn mercator_y_m(lat_deg: f64) -> f64 {
    let lat = clamp(lat_deg, -MERCATOR_MAX_LAT_DEG, MERCATOR_MAX_LAT_DEG).to_radians();
    WGS84_A * (0.5 * (std::f64::consts::FRAC_PI_2 + lat)).tan().ln()
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
    let base = (w / world_width_m).min(h / world_height_m);
    (base * cam.zoom).max(1e-6)
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
    let dy_m = delta_y_px / projector.scale_px_per_m;
    let center_x = projector.center_x + dx_m;
    let center_y = projector.center_y + dy_m;
    Camera2DState {
        center_lon_deg: wrap_lon_deg(inverse_mercator_lon_deg(center_x)),
        center_lat_deg: clamp(
            inverse_mercator_lat_deg(center_y),
            -MERCATOR_MAX_LAT_DEG,
            MERCATOR_MAX_LAT_DEG,
        ),
        ..cam
    }
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
            let _ = STATE.try_with(|state_ref| {
                let state = state_ref.borrow();
                if let Some(ctx) = &state.wgpu {
                    let view_proj =
                        camera_view_proj(state.camera, state.canvas_width, state.canvas_height);

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
            Ok(())
        }
        ViewMode::TwoD => render_scene_2d(),
    }
}

fn render_scene_2d() -> Result<(), JsValue> {
    match STATE.try_with(|state_ref| {
        let state = state_ref.borrow();
        let Some(ctx) = state.ctx_2d.as_ref() else {
            return Ok(());
        };

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

        // Polygons first (fills).
        let draw_poly_tris = |pos: &[[f32; 3]], color: [f32; 4]| {
            ctx_set_fill_style(ctx, &rgba_css(color));
            ctx.begin_path();
            for tri in pos.chunks_exact(3).take(200_000) {
                let a = tri[0];
                let b = tri[1];
                let c = tri[2];
                let (lon_a, lat_a) = world_to_lon_lat_deg([a[0] as f64, a[1] as f64, a[2] as f64]);
                let (lon_b, lat_b) = world_to_lon_lat_deg([b[0] as f64, b[1] as f64, b[2] as f64]);
                let (lon_c, lat_c) = world_to_lon_lat_deg([c[0] as f64, c[1] as f64, c[2] as f64]);

                let (ax, ay) = projector.project_lon_lat(lon_a, lat_a, w, h);
                let (bx, by) = projector.project_lon_lat(lon_b, lat_b, w, h);
                let (cx, cy) = projector.project_lon_lat(lon_c, lat_c, w, h);

                ctx.move_to(ax, ay);
                ctx.line_to(bx, by);
                ctx.line_to(cx, cy);
                ctx.close_path();
            }
            ctx.fill();
        };

        let draw_poly_tris_mercator = |pos: &[[f32; 2]], color: [f32; 4]| {
            ctx_set_fill_style(ctx, &rgba_css(color));
            ctx.begin_path();
            for tri in pos.chunks_exact(3).take(200_000) {
                let a = tri[0];
                let b = tri[1];
                let c = tri[2];
                let (ax, ay) = projector.project_mercator_m(a[0] as f64, a[1] as f64, w, h);
                let (bx, by) = projector.project_mercator_m(b[0] as f64, b[1] as f64, w, h);
                let (cx, cy) = projector.project_mercator_m(c[0] as f64, c[1] as f64, w, h);

                ctx.move_to(ax, ay);
                ctx.line_to(bx, by);
                ctx.line_to(cx, cy);
                ctx.close_path();
            }
            ctx.fill();
        };

        if state.base_regions_style.visible {
            if let Some(pos) = state.base_regions_mercator.as_deref() {
                draw_poly_tris_mercator(pos, state.base_regions_style.color);
            } else if let Some(pos) = state.base_regions_positions.as_deref() {
                draw_poly_tris(pos, state.base_regions_style.color);
            }
        }
        if state.regions_style.visible
            && let Some(pos) = state.regions_positions.as_deref()
        {
            draw_poly_tris(pos, state.regions_style.color);
        }
        if state.uploaded_regions_style.visible
            && let Some(pos) = state.uploaded_regions_positions.as_deref()
        {
            draw_poly_tris(pos, state.uploaded_regions_style.color);
        }
        if state.selection_style.visible
            && let Some(pos) = state.selection_poly_positions.as_deref()
        {
            draw_poly_tris(pos, state.selection_style.color);
        }

        // Lines.
        let draw_lines = |pos: &[[f32; 3]], color: [f32; 4], width_px: f64| {
            ctx_set_stroke_style(ctx, &rgba_css(color));
            ctx.set_line_width(width_px.max(1.0));
            ctx.set_line_cap("round");
            ctx.begin_path();
            for seg in pos.chunks_exact(2).take(250_000) {
                let a = seg[0];
                let b = seg[1];
                let (lon_a, lat_a) = world_to_lon_lat_deg([a[0] as f64, a[1] as f64, a[2] as f64]);
                let (lon_b, lat_b) = world_to_lon_lat_deg([b[0] as f64, b[1] as f64, b[2] as f64]);
                let (ax, ay) = projector.project_lon_lat(lon_a, lat_a, w, h);
                let (bx, by) = projector.project_lon_lat(lon_b, lat_b, w, h);
                ctx.move_to(ax, ay);
                ctx.line_to(bx, by);
            }
            ctx.stroke();
        };

        if state.corridors_style.visible
            && let Some(pos) = state.corridors_positions.as_deref()
        {
            draw_lines(pos, state.corridors_style.color, state.line_width_px as f64);
        }
        if state.uploaded_corridors_style.visible
            && let Some(pos) = state.uploaded_corridors_positions.as_deref()
        {
            draw_lines(
                pos,
                state.uploaded_corridors_style.color,
                state.line_width_px as f64,
            );
        }
        if state.selection_style.visible
            && let Some(pos) = state.selection_line_positions.as_deref()
        {
            draw_lines(
                pos,
                state.selection_style.color,
                (state.line_width_px * 1.6) as f64,
            );
        }

        // Points.
        let draw_points = |centers: &[[f32; 3]], color: [f32; 4], radius_px: f64| {
            ctx_set_fill_style(ctx, &rgba_css(color));
            ctx.begin_path();
            for &c in centers.iter().take(200_000) {
                let (lon, lat) = world_to_lon_lat_deg([c[0] as f64, c[1] as f64, c[2] as f64]);
                let (x, y) = projector.project_lon_lat(lon, lat, w, h);
                let _ = ctx.arc(x, y, radius_px, 0.0, std::f64::consts::TAU);
            }
            ctx.fill();
        };

        let r = (state.city_marker_size as f64).clamp(1.0, 32.0);
        if state.cities_style.visible
            && let Some(centers) = state.cities_centers.as_deref()
        {
            draw_points(centers, state.cities_style.color, r);
        }
        if state.uploaded_points_style.visible
            && let Some(centers) = state.uploaded_centers.as_deref()
        {
            draw_points(centers, state.uploaded_points_style.color, r);
        }
        if state.selection_style.visible
            && let Some(c) = state.selection_center
        {
            draw_points(
                std::slice::from_ref(&c),
                state.selection_style.color,
                r * 1.35,
            );
        }

        Ok(())
    }) {
        Ok(res) => res,
        Err(_) => Ok(()),
    }
}

fn build_city_vertices(
    centers: &[[f32; 3]],
    size_px: f32,
    color: [f32; 4],
    lift: f32,
) -> Vec<CityVertex> {
    let size_px = size_px.clamp(1.0, 64.0);

    // `lift` is a fraction of Earth radius (legacy UI semantics); convert to meters.
    let mut lift_m = lift * (WGS84_A as f32);
    if lift_m <= 0.0 {
        // Keep points slightly above the surface to avoid z-fighting.
        lift_m = 50.0;
    }

    let mut out: Vec<CityVertex> = Vec::with_capacity(centers.len() * 6);
    for &c in centers {
        let v0 = CityVertex {
            center: c,
            lift: lift_m,
            offset_px: [-size_px, -size_px],
            color,
        };
        let v1 = CityVertex {
            center: c,
            lift: lift_m,
            offset_px: [size_px, -size_px],
            color,
        };
        let v2 = CityVertex {
            center: c,
            lift: lift_m,
            offset_px: [size_px, size_px],
            color,
        };
        let v3 = CityVertex {
            center: c,
            lift: lift_m,
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
    let lon = u[2].atan2(u[0]).to_degrees();
    let lat = u[1].clamp(-1.0, 1.0).asin().to_degrees();
    (lon, lat)
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

    let push_segment = |out: &mut Vec<CorridorVertex>, a: [f32; 3], b: [f32; 3]| {
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
    };

    const MAX_SEGMENTS: usize = 250_000;
    let mut emitted_segments = 0usize;

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

fn rebuild_overlays_and_upload() -> Result<(), JsValue> {
    // Budget guardrails: the web viewer will trap on extremely large GPU buffers.
    // Keep these conservative; we can revisit once we stream + chunk uploads.
    const MAX_UPLOADED_POINTS: usize = 100_000;
    const MAX_UPLOADED_LINE_SEGMENTS: usize = 150_000;
    const MAX_UPLOADED_POLY_VERTS: usize = 600_000;

    match STATE.try_with(|state| {
        let mut s = state.borrow_mut();

        // Refresh cached viewer-space geometry from the engine worlds.
        // This ensures all rendered features flow through `layers`.
        let layer = VectorLayer::new(1);

        // Built-ins
        if let Some(pos) = s.surface_positions.as_ref() {
            let pos_vec = pos.clone();
            let mercator = pos_vec.iter().map(viewer_to_mercator_m).collect();
            s.base_regions_positions = Some(pos_vec);
            s.base_regions_mercator = Some(mercator);
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
                .map(|pos| pos.iter().map(viewer_to_mercator_m).collect());
        }
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
        let mut base_polys: Vec<OverlayVertex> = Vec::new();
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

        // Base polygons (triangles)
        if s.base_regions_style.visible
            && let Some(pos) = s.base_regions_positions.as_deref()
        {
            let mut lift_m = s.base_regions_style.lift * (WGS84_A as f32);
            if lift_m <= 0.0 {
                // Small lift combined with depth bias prevents z-fighting
                // without causing visible "floating" at grazing angles.
                lift_m = 100.0;
            }
            base_polys.extend(pos.iter().map(|&p| OverlayVertex {
                position: p,
                lift: lift_m,
                color: s.base_regions_style.color,
            }));
        }

        // Polygons (triangles)
        if s.regions_style.visible
            && let Some(pos) = s.regions_positions.as_deref()
        {
            let mut lift_m = s.regions_style.lift * (WGS84_A as f32);
            if lift_m <= 0.0 {
                // Small lift combined with depth bias prevents z-fighting.
                lift_m = 25.0;
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
                lift_m = 25.0;
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
            set_base_regions_points(ctx, &base_polys);
            set_regions_points(ctx, &polys);
            s.pending_cities = None;
            s.pending_corridors = None;
            s.pending_base_regions = None;
            s.pending_regions = None;
        } else {
            s.pending_cities = Some(points);
            s.pending_corridors = Some(lines);
            s.pending_base_regions = Some(base_polys);
            s.pending_regions = Some(polys);
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
    let mode = match ViewMode::from_str(mode) {
        ViewMode::TwoD => ViewMode::ThreeD,
        ViewMode::ThreeD => ViewMode::ThreeD,
    };
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.view_mode = mode;
        if mode == ViewMode::TwoD {
            s.surface_positions = None;
            s.surface_zoom = None;
            s.base_regions_positions = None;
        }
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
        }
    });
    render_scene()
}

/// Orbit around the globe.
///
/// Intended usage: call with pointer delta in pixels.
#[wasm_bindgen]
pub fn camera_orbit(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        match s.view_mode {
            ViewMode::ThreeD => {
                // Scale orbit sensitivity to viewport size so drag feels consistent.
                // Roughly: dragging across the shorter side ~= 180 degrees.
                let min_dim = s.canvas_width.min(s.canvas_height).max(1.0);
                let speed = std::f64::consts::PI / min_dim;
                // Screen drag direction should feel like you're "grabbing" the globe surface.
                // Drag left => globe rotates left (yaw increases).
                s.camera.yaw_rad += delta_x_px * speed;
                s.camera.pitch_rad = clamp(s.camera.pitch_rad - delta_y_px * speed, -1.55, 1.55);

                // Keep yaw bounded to avoid precision loss over time.
                s.camera.yaw_rad = (s.camera.yaw_rad + std::f64::consts::PI)
                    .rem_euclid(2.0 * std::f64::consts::PI)
                    - std::f64::consts::PI;
            }
            ViewMode::TwoD => {
                // In 2D, treat orbit as pan.
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

/// Pan the camera target.
///
/// Intended usage: call with pointer delta in pixels.
#[wasm_bindgen]
pub fn camera_pan(delta_x_px: f64, delta_y_px: f64) -> Result<(), JsValue> {
    with_state(|state| {
        let mut s = state.borrow_mut();
        s.auto_rotate_last_user_time_s = wall_clock_seconds();
        match s.view_mode {
            ViewMode::ThreeD => {
                let cam = s.camera;

                let dir = [
                    cam.pitch_rad.cos() * cam.yaw_rad.cos(),
                    cam.pitch_rad.sin(),
                    -cam.pitch_rad.cos() * cam.yaw_rad.sin(),
                ];
                let forward = vec3_normalize(vec3_mul(dir, -1.0));
                let up = [0.0, 1.0, 0.0];
                let right = vec3_normalize(vec3_cross(forward, up));
                let real_up = vec3_cross(right, forward);

                let pan_scale = cam.distance * 0.002;
                let delta = vec3_add(
                    vec3_mul(right, delta_x_px * pan_scale),
                    vec3_mul(real_up, delta_y_px * pan_scale),
                );
                s.camera.target = vec3_add(s.camera.target, delta);
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
        match s.view_mode {
            ViewMode::ThreeD => {
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
                    -cam.pitch_rad.cos() * cam.yaw_rad.sin(),
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
            }
            ViewMode::TwoD => {
                let zoom = (-wheel_delta_y * 0.0015).exp();
                s.camera_2d.zoom = clamp(s.camera_2d.zoom * zoom, 0.2, 200.0);
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

    let _ = rebuild_overlays_and_upload();
    render_scene()
}

#[wasm_bindgen]
pub fn set_line_width_px(width_px: f64) -> Result<(), JsValue> {
    let width_px = (width_px as f32).clamp(1.0, 24.0);
    with_state(|state| {
        state.borrow_mut().line_width_px = width_px;
    });
    let _ = rebuild_overlays_and_upload();
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

fn build_terrain_mesh(tileset: &TerrainTileset, tiles: &[TerrainTile]) -> Vec<OverlayVertex> {
    let tile_size = tileset.tile_size as usize;
    let step = tileset.sample_step.unwrap_or(4).max(1) as usize;
    let no_data = tileset.no_data.unwrap_or(-9999.0) as f32;
    let min_h = tileset.min_height as f32;
    let max_h = tileset.max_height as f32;

    let mut vertices: Vec<OverlayVertex> = Vec::new();

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
                vertices.push(OverlayVertex {
                    position: p00,
                    lift: h00,
                    color: c00,
                });
                vertices.push(OverlayVertex {
                    position: p10,
                    lift: h10,
                    color: c10,
                });
                vertices.push(OverlayVertex {
                    position: p11,
                    lift: h11,
                    color: c11,
                });

                // Triangle 2: p00, p11, p01
                vertices.push(OverlayVertex {
                    position: p00,
                    lift: h00,
                    color: c00,
                });
                vertices.push(OverlayVertex {
                    position: p11,
                    lift: h11,
                    color: c11,
                });
                vertices.push(OverlayVertex {
                    position: p01,
                    lift: h01,
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
    let _ = rebuild_overlays_and_upload();
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
    let _ = rebuild_overlays_and_upload();
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
    let _ = rebuild_overlays_and_upload();
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

    let chunk = formats::VectorChunk::from_avc_bytes(&bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

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
    let (lon, lat) = world_to_lon_lat_deg([p[0] as f64, p[1] as f64, p[2] as f64]);
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
            // Markers are rendered at a constant screen-space size.
            let radius_px = (s.city_marker_size).clamp(2.0, 64.0);
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

        let mut best: Option<([f32; 3], f64)> = None; // (center, d2)

        let mut consider = |centers: &[[f32; 3]]| {
            for &c in centers.iter().take(250_000) {
                let (lon, lat) = world_to_lon_lat_deg([c[0] as f64, c[1] as f64, c[2] as f64]);
                let (sx, sy) = projector.project_lon_lat(lon, lat, w, h);
                let dx = sx - x_px;
                let dy = sy - y_px;
                let d2 = dx * dx + dy * dy;
                if best.map(|(_, bd2)| d2 < bd2).unwrap_or(true) {
                    best = Some((c, d2));
                }
            }
        };

        if s.cities_style.visible
            && let Some(centers) = s.cities_centers.as_deref()
        {
            consider(centers);
        }
        if s.uploaded_points_style.visible
            && let Some(centers) = s.uploaded_centers.as_deref()
        {
            consider(centers);
        }

        if let Some((c, d2)) = best
            && d2 <= r2
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
pub async fn load_geojson_file(name: String, geojson_text: String) -> Result<JsValue, JsValue> {
    // Guardrails: large uploads can exceed wasm memory or browser storage limits.
    // (We currently persist catalog entries via LocalStorage; IndexedDB/DuckDB comes next.)
    const MAX_GEOJSON_TEXT_BYTES: usize = 8 * 1024 * 1024;
    // NOTE: catalog persistence is best-effort. We store AVC in chunked LocalStorage keys.

    if geojson_text.len() > MAX_GEOJSON_TEXT_BYTES {
        return Err(JsValue::from_str(
            "Upload too large for the web viewer (max 8MiB GeoJSON).",
        ));
    }

    web_sys::console::error_1(&JsValue::from_str("upload: parsing GeoJSON"));

    let chunk = formats::VectorChunk::from_geojson_str(&geojson_text)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    web_sys::console::error_1(&JsValue::from_str("upload: parsed GeoJSON"));

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
    let avc_bytes = chunk
        .to_avc_bytes()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    web_sys::console::error_1(&JsValue::from_str("upload: encoded AVC"));

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

    let world = world_from_vector_chunk(&chunk, None);

    web_sys::console::error_1(&JsValue::from_str("upload: ingested world"));

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
    rebuild_overlays_and_upload()?;
    web_sys::console::error_1(&JsValue::from_str("upload: rebuilt overlays"));
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

        if s.view_mode == ViewMode::ThreeD && s.auto_rotate_enabled {
            let idle = wall_clock_seconds() - s.auto_rotate_last_user_time_s;
            if idle >= s.auto_rotate_resume_delay_s {
                let speed_rad = s.auto_rotate_speed_deg_per_s.to_radians();
                s.camera.yaw_rad =
                    (s.camera.yaw_rad + speed_rad * s.dt_s).rem_euclid(2.0 * std::f64::consts::PI);
            }
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

async fn init_wgpu_inner() -> Result<(), JsValue> {
    let ctx = init_wgpu_from_canvas_id("atlas-canvas-3d").await?;

    with_state(|state| {
        let mut s = state.borrow_mut();
        let palette = palette_for(s.theme);
        let globe_transparent = s.globe_transparent;
        let pending = s.pending_cities.take();
        let pending_corridors = s.pending_corridors.take();
        let pending_base_regions = s.pending_base_regions.take();
        let pending_regions = s.pending_regions.take();
        let pending_terrain = s.pending_terrain.take();
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
