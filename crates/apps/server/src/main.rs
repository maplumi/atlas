use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::json;
use tempfile::TempDir;
use tokio::process::Command;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    terrain: Arc<TerrainConfig>,
    surface_root: PathBuf,
    http: reqwest::Client,
}

#[derive(Clone, Debug)]
struct TerrainConfig {
    cache_root: PathBuf,
    stac_url: String,
    collection: String,
    vertical_datum: String,
    vertical_units: String,
    tile_size: u32,
    zoom_min: u32,
    zoom_max: u32,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    min_height: f64,
    max_height: f64,
    no_data: f64,
    sample_step: u32,
    max_cogs_per_tile: usize,
}

#[derive(Debug, Serialize)]
struct TerrainTileset {
    version: u32,
    tile_size: u32,
    zoom_min: u32,
    zoom_max: u32,
    data_type: String,
    tile_path_template: String,
    vertical_datum: String,
    vertical_units: String,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    min_height: f64,
    max_height: f64,
    no_data: Option<f64>,
    sample_step: Option<u32>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let terrain_root = env::var("TERRAIN_ROOT").unwrap_or_else(|_| "/data/terrain".to_string());
    let stac_url = env::var("STAC_URL")
        .unwrap_or_else(|_| "https://copernicus-dem-30m-stac.s3.amazonaws.com".to_string());
    let addr: SocketAddr = env::var("TERRAIN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9100".to_string())
        .parse()
        .expect("invalid TERRAIN_ADDR");

    let terrain_root = PathBuf::from(terrain_root);
    let cache_root = env::var("TERRAIN_CACHE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| terrain_root.join("cache"));
    let surface_root = env::var("SURFACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| terrain_root.join("surface"));

    let terrain = TerrainConfig {
        cache_root,
        stac_url,
        collection: env::var("TERRAIN_COLLECTION").unwrap_or_else(|_| "dem_cop_30".to_string()),
        // Most global DEM products (including Copernicus DEM) are distributed as orthometric height
        // relative to a geoid ("MSL-like"). Keep this explicit and configurable.
        vertical_datum: env::var("TERRAIN_VERTICAL_DATUM")
            .unwrap_or_else(|_| "msl-egm2008".to_string()),
        vertical_units: env::var("TERRAIN_VERTICAL_UNITS").unwrap_or_else(|_| "m".to_string()),
        tile_size: env_var_u32("TERRAIN_TILE_SIZE", 256),
        zoom_min: env_var_u32("TERRAIN_ZOOM_MIN", 0),
        zoom_max: env_var_u32("TERRAIN_ZOOM_MAX", 8),
        min_lon: env_var_f64("TERRAIN_MIN_LON", -180.0),
        max_lon: env_var_f64("TERRAIN_MAX_LON", 180.0),
        min_lat: env_var_f64("TERRAIN_MIN_LAT", -90.0),
        max_lat: env_var_f64("TERRAIN_MAX_LAT", 90.0),
        min_height: env_var_f64("TERRAIN_MIN_HEIGHT", -500.0),
        max_height: env_var_f64("TERRAIN_MAX_HEIGHT", 9000.0),
        no_data: env_var_f64("TERRAIN_NO_DATA", -9999.0),
        sample_step: env_var_u32("TERRAIN_SAMPLE_STEP", 4),
        max_cogs_per_tile: env_var_usize("TERRAIN_MAX_COGS_PER_TILE", 16),
    };

    let state = AppState {
        terrain: Arc::new(terrain),
        surface_root,
        http: reqwest::Client::new(),
    };

    if let Err(err) = tokio::fs::create_dir_all(&state.terrain.cache_root).await {
        warn!("failed to create cache root: {err}");
    }
    if let Err(err) = tokio::fs::create_dir_all(&state.surface_root).await {
        warn!("failed to create surface root: {err}");
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS]);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/terrain/status", get(get_terrain_status))
        .route("/terrain/tileset.json", get(get_tileset))
        .route("/terrain/tiles/:z/:x/:y.bin", get(get_tile))
        .route("/surface/tileset.json", get(get_surface_tileset))
        .route("/surface/tiles/:z/:x/:y.bin", get(get_surface_tile))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    info!("terrain server listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}

async fn healthz() -> Response {
    (StatusCode::OK, "ok").into_response()
}

async fn get_tileset(State(state): State<AppState>) -> Response {
    let cfg = &state.terrain;
    let tileset = TerrainTileset {
        version: 1,
        tile_size: cfg.tile_size,
        zoom_min: cfg.zoom_min,
        zoom_max: cfg.zoom_max,
        data_type: "f32-le".to_string(),
        tile_path_template: "tiles/{z}/{x}/{y}.bin".to_string(),
        vertical_datum: cfg.vertical_datum.clone(),
        vertical_units: cfg.vertical_units.clone(),
        min_lon: cfg.min_lon,
        max_lon: cfg.max_lon,
        min_lat: cfg.min_lat,
        max_lat: cfg.max_lat,
        min_height: cfg.min_height,
        max_height: cfg.max_height,
        no_data: Some(cfg.no_data),
        sample_step: Some(cfg.sample_step),
    };

    let body = match serde_json::to_string(&tileset) {
        Ok(v) => v,
        Err(err) => {
            error!("tileset serialization failed: {err}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "tileset error").into_response();
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (StatusCode::OK, headers, Body::from(body)).into_response()
}

async fn get_terrain_status(State(state): State<AppState>) -> Response {
    let tiles_dir = state.terrain.cache_root.join("tiles");
    let (tiles_count, tiles_truncated) = count_tiles(&tiles_dir, 200_000).await;
    let status = if tiles_count > 0 { "ready" } else { "empty" };

    let body = json!({
        "status": status,
        "cache_root": state.terrain.cache_root,
        "tiles_count": tiles_count,
        "tiles_count_truncated": tiles_truncated,
        "tile_size": state.terrain.tile_size,
        "zoom_min": state.terrain.zoom_min,
        "zoom_max": state.terrain.zoom_max,
        "vertical_datum": state.terrain.vertical_datum,
        "vertical_units": state.terrain.vertical_units,
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (StatusCode::OK, headers, Body::from(body.to_string())).into_response()
}

async fn get_tile(
    State(state): State<AppState>,
    AxumPath((z, x, y)): AxumPath<(u32, u32, String)>,
) -> Response {
    let Some(y) = parse_tile_y(&y) else {
        return (StatusCode::BAD_REQUEST, "invalid tile index").into_response();
    };
    let cfg = &state.terrain;
    if z < cfg.zoom_min || z > cfg.zoom_max {
        return (StatusCode::NOT_FOUND, "zoom out of range").into_response();
    }

    let cache_path = cfg
        .cache_root
        .join("tiles")
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin"));

    if tokio::fs::metadata(&cache_path).await.is_ok() {
        return serve_file(&cache_path, "application/octet-stream").await;
    }

    match build_tile(&state, z, x, y, &cache_path).await {
        Ok(()) => serve_file(&cache_path, "application/octet-stream").await,
        Err(err) => {
            warn!("tile build failed: z={z} x={x} y={y} -> {err}");
            (StatusCode::NOT_FOUND, "tile unavailable").into_response()
        }
    }
}

async fn get_surface_tileset(State(state): State<AppState>) -> Response {
    let path = state.surface_root.join("tileset.json");
    if tokio::fs::metadata(&path).await.is_err() {
        return (StatusCode::NOT_FOUND, "surface tileset missing").into_response();
    }
    serve_file(&path, "application/json").await
}

async fn get_surface_tile(
    State(state): State<AppState>,
    AxumPath((z, x, y)): AxumPath<(u32, u32, String)>,
) -> Response {
    let Some(y) = parse_tile_y(&y) else {
        return (StatusCode::BAD_REQUEST, "invalid tile index").into_response();
    };
    let path = state
        .surface_root
        .join("tiles")
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin"));

    if tokio::fs::metadata(&path).await.is_err() {
        return (StatusCode::NOT_FOUND, "surface tile missing").into_response();
    }
    serve_file(&path, "application/octet-stream").await
}

fn parse_tile_y(raw: &str) -> Option<u32> {
    let trimmed = raw.trim_end_matches(".bin");
    trimmed.parse::<u32>().ok()
}

async fn serve_file(path: &Path, content_type: &str) -> Response {
    match tokio::fs::read(path).await {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_str(content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            (StatusCode::OK, headers, Body::from(data)).into_response()
        }
        Err(err) => {
            warn!("file read failed: {path:?} -> {err}");
            (StatusCode::NOT_FOUND, "not found").into_response()
        }
    }
}

async fn count_tiles(dir: &Path, max: usize) -> (usize, bool) {
    let mut count = 0usize;
    let mut truncated = false;
    let mut stack = vec![dir.to_path_buf()];

    while let Some(path) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let entry_path = entry.path();
            if file_type.is_dir() {
                stack.push(entry_path);
            } else if file_type.is_file()
                && entry_path.extension().and_then(|e| e.to_str()) == Some("bin")
            {
                count += 1;
                if count >= max {
                    truncated = true;
                    return (count, truncated);
                }
            }
        }
    }

    (count, truncated)
}

async fn build_tile(
    state: &AppState,
    z: u32,
    x: u32,
    y: u32,
    cache_path: &Path,
) -> Result<(), String> {
    let cfg = &state.terrain;
    let (lon_min, lon_max, lat_min, lat_max) = terrain_tile_bounds(cfg, z, x, y);

    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("cache dir create failed: {e}"))?;
    }

    let hrefs = fetch_cog_hrefs(state, lon_min, lon_max, lat_min, lat_max).await?;
    if hrefs.is_empty() {
        return Err("no COGs for tile".to_string());
    }

    let temp_dir =
        TempDir::new_in(&cfg.cache_root).map_err(|e| format!("temp dir create failed: {e}"))?;
    let list_path = temp_dir.path().join("inputs.txt");
    let vrt_path = temp_dir.path().join("tile.vrt");
    let tmp_bin_path = temp_dir.path().join("tile.bin");

    let mut list_body = String::new();
    for href in hrefs.iter().take(cfg.max_cogs_per_tile) {
        let href = if href.starts_with("/vsicurl/") {
            href.to_string()
        } else {
            format!("/vsicurl/{href}")
        };
        list_body.push_str(&href);
        list_body.push('\n');
    }

    tokio::fs::write(&list_path, list_body)
        .await
        .map_err(|e| format!("write input list failed: {e}"))?;

    run_command(
        "gdalbuildvrt",
        &[
            "-input_file_list",
            list_path.to_str().ok_or("input list path invalid")?,
            vrt_path.to_str().ok_or("vrt path invalid")?,
        ],
    )
    .await?;

    run_command(
        "gdal_translate",
        &[
            "-of",
            "ENVI",
            "-ot",
            "Float32",
            "-co",
            "INTERLEAVE=BSQ",
            "-co",
            "ENDIANNESS=LITTLE",
            "-a_nodata",
            &cfg.no_data.to_string(),
            "-projwin",
            &lon_min.to_string(),
            &lat_max.to_string(),
            &lon_max.to_string(),
            &lat_min.to_string(),
            "-outsize",
            &cfg.tile_size.to_string(),
            &cfg.tile_size.to_string(),
            vrt_path.to_str().ok_or("vrt path invalid")?,
            tmp_bin_path.to_str().ok_or("tmp bin path invalid")?,
        ],
    )
    .await?;

    let bytes = tokio::fs::read(&tmp_bin_path)
        .await
        .map_err(|e| format!("read temp tile failed: {e}"))?;

    let expected_len = cfg.tile_size as usize * cfg.tile_size as usize * 4;
    if bytes.len() != expected_len {
        return Err(format!(
            "tile size mismatch: got {} bytes, expected {}",
            bytes.len(),
            expected_len
        ));
    }

    tokio::fs::write(cache_path, bytes)
        .await
        .map_err(|e| format!("cache write failed: {e}"))?;

    Ok(())
}

fn terrain_tile_bounds(cfg: &TerrainConfig, z: u32, x: u32, y: u32) -> (f64, f64, f64, f64) {
    let n = 2u32.pow(z);
    let lon_span = (cfg.max_lon - cfg.min_lon) / n as f64;
    let lat_span = (cfg.max_lat - cfg.min_lat) / n as f64;

    let lon_min = cfg.min_lon + (x as f64) * lon_span;
    let lon_max = lon_min + lon_span;
    let lat_max = cfg.max_lat - (y as f64) * lat_span;
    let lat_min = lat_max - lat_span;
    (lon_min, lon_max, lat_min, lat_max)
}

async fn fetch_cog_hrefs(
    state: &AppState,
    lon_min: f64,
    lon_max: f64,
    lat_min: f64,
    lat_max: f64,
) -> Result<Vec<String>, String> {
    let cfg = &state.terrain;
    let search_url = format!("{}/search", cfg.stac_url.trim_end_matches('/'));
    let mut hrefs = Vec::new();

    let body = json!({
        "collections": [cfg.collection],
        "bbox": [lon_min, lat_min, lon_max, lat_max],
        "limit": cfg.max_cogs_per_tile as u32,
    });

    let search_result = stac_search_page(&state.http, &search_url, Some(body)).await;
    if let Ok(resp_json) = search_result {
        let features = resp_json
            .get("features")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();

        for feat in features {
            let assets = feat.get("assets").and_then(|a| a.as_object());
            let Some(assets) = assets else {
                continue;
            };
            if let Some(href) = select_asset_href(assets) {
                hrefs.push(href);
            }
        }
    }

    if !hrefs.is_empty() {
        return Ok(hrefs);
    }

    // Fallback: static Copernicus layout
    let static_hrefs = fetch_static_copernicus_hrefs(state, lon_min, lon_max, lat_min, lat_max)
        .await
        .map_err(|e| format!("static STAC lookup failed: {e}"))?;

    Ok(static_hrefs)
}

async fn fetch_static_copernicus_hrefs(
    state: &AppState,
    lon_min: f64,
    lon_max: f64,
    lat_min: f64,
    lat_max: f64,
) -> Result<Vec<String>, String> {
    let cfg = &state.terrain;
    let min_lon = lon_min.max(-180.0);
    let max_lon = lon_max.min(180.0);
    let min_lat = lat_min.max(-90.0);
    let max_lat = lat_max.min(90.0);

    let lon0 = min_lon.floor() as i32;
    let lon1 = max_lon.ceil() as i32;
    let lat0 = min_lat.floor() as i32;
    let lat1 = max_lat.ceil() as i32;

    let mut hrefs = Vec::new();
    for lat in lat0..lat1 {
        for lon in lon0..lon1 {
            if hrefs.len() >= cfg.max_cogs_per_tile {
                return Ok(hrefs);
            }

            let item_id = copernicus_item_id(lat, lon);
            let item_url = format!(
                "{}/items/{}.json",
                cfg.stac_url.trim_end_matches('/'),
                item_id
            );
            let resp = state
                .http
                .get(&item_url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                continue;
            }

            let feat: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let assets = feat.get("assets").and_then(|a| a.as_object());
            let Some(assets) = assets else {
                continue;
            };
            if let Some(href) = select_asset_href(assets) {
                hrefs.push(href);
            }
        }
    }

    Ok(hrefs)
}

async fn stac_search_page(
    client: &reqwest::Client,
    base_url: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let resp = if let Some(body) = body {
        client.post(base_url).json(&body).send().await?
    } else {
        client.get(base_url).send().await?
    };

    if !resp.status().is_success() {
        return Err(format!("STAC request failed: {}", resp.status()).into());
    }

    let v: serde_json::Value = resp.json().await?;
    Ok(v)
}

fn select_asset_href(assets: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let preferred = ["elevation", "data", "dem", "cop-dem"];
    for key in preferred.iter() {
        if let Some(href) = assets
            .get(*key)
            .and_then(|v| v.get("href"))
            .and_then(|v| v.as_str())
        {
            return Some(href.to_string());
        }
    }

    let mut keys: Vec<_> = assets.keys().collect();
    keys.sort();
    keys.first().and_then(|k| {
        assets
            .get(*k)
            .and_then(|v| v.get("href"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    })
}

fn copernicus_item_id(lat_deg: i32, lon_deg: i32) -> String {
    let (lat_hemi, lat_abs) = if lat_deg >= 0 {
        ('N', lat_deg as u32)
    } else {
        ('S', (-lat_deg) as u32)
    };
    let (lon_hemi, lon_abs) = if lon_deg >= 0 {
        ('E', lon_deg as u32)
    } else {
        ('W', (-lon_deg) as u32)
    };

    format!("Copernicus_DSM_COG_10_{lat_hemi}{lat_abs:02}_00_{lon_hemi}{lon_abs:03}_00")
}

async fn run_command(command: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(command)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("{command} failed to start: {e}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{command} failed: {stderr}"))
}

fn env_var_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_var_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_var_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
