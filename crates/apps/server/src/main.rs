use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use streaming::StreamingConfig;
use tempfile::TempDir;
use tokio::process::Command;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

mod data_sources;
mod webhooks;
mod ws_streaming;

use data_sources::{
    DataSource, DataSourceInfo, DataSourceMetadata, FallbackSource, FilesystemSource, HttpSource,
    MemorySource, PmtilesSource,
};
use webhooks::{WebhookConfig, WebhookRegistry, WebhookSchema, WebhookSource};
use ws_streaming::DataSourceRegistry;

#[derive(Clone)]
struct AppState {
    terrain: Arc<TerrainConfig>,
    surface_root: PathBuf,
    http: reqwest::Client,
    data_sources: Arc<DataSourceRegistry>,
    webhooks: Arc<WebhookRegistry>,
    streaming_config: StreamingConfig,
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

    // Initialize data source registry
    let data_sources = Arc::new(DataSourceRegistry::new());

    // Register terrain tiles as a data source
    let terrain_source = FilesystemSource::new(
        terrain.cache_root.join("tiles"),
        "terrain",
        streaming::TileFormat::HeightmapF32,
        "bin",
    );
    data_sources.register("terrain", Arc::new(terrain_source));

    // Register surface tiles if available
    let surface_source = FilesystemSource::new(
        surface_root.clone(),
        "surface",
        streaming::TileFormat::Mvt,
        "bin",
    );
    data_sources.register("surface", Arc::new(surface_source));

    // Initialize webhook registry
    let webhooks = Arc::new(WebhookRegistry::new(WebhookConfig::default()));

    // Register default webhook sources
    webhooks.register_source(WebhookSource {
        id: "realtime".to_string(),
        name: "Real-time GeoJSON".to_string(),
        description: Some("Accept GeoJSON features for real-time display".to_string()),
        schema: WebhookSchema::GeoJson,
        transform: None,
    });

    let streaming_config = StreamingConfig::default();

    let state = AppState {
        terrain: Arc::new(terrain),
        surface_root,
        http: reqwest::Client::new(),
        data_sources: data_sources.clone(),
        webhooks: webhooks.clone(),
        streaming_config: streaming_config.clone(),
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
        // Data source management API
        .route(
            "/api/sources",
            get(list_data_sources).post(create_data_source),
        )
        .route(
            "/api/sources/:source_id",
            get(get_data_source_info).delete(delete_data_source),
        )
        .route(
            "/api/sources/:source_id/tiles/:z/:x/:y",
            get(get_source_tile),
        )
        .route(
            "/api/sources/:source_id/tiles/batch",
            post(get_source_tiles_batch),
        )
        .route(
            "/api/sources/:source_id/has/:z/:x/:y",
            get(check_source_has_tile),
        )
        // Memory source tile management
        .route(
            "/api/sources/:source_id/tiles/:z/:x/:y",
            put(put_memory_tile).delete(delete_memory_tile),
        )
        // Webhook management API
        .route(
            "/api/webhooks",
            get(list_webhook_sources).post(create_webhook_source),
        )
        .route(
            "/api/webhooks/:source_id",
            get(get_webhook_source_info).delete(delete_webhook_source),
        )
        // WebSocket tile streaming
        .route("/ws/tiles", get(ws_tiles_handler))
        // WebSocket for real-time webhook data
        .route("/ws/realtime", get(ws_realtime_handler))
        // Webhook ingestion
        .route("/webhook/:source_id", post(webhook_handler))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    info!("terrain server listening on http://{addr}");
    info!("WebSocket tile streaming available at ws://{addr}/ws/tiles");
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}

/// WebSocket upgrade handler for tile streaming.
async fn ws_tiles_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        ws_streaming::handle_ws_connection(
            socket,
            state.data_sources.clone(),
            state.streaming_config.clone(),
        )
    })
}

/// Webhook ingestion handler.
async fn webhook_handler(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    match state
        .webhooks
        .process_webhook(&source_id, &headers, &body)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response(),
        Err(e) => e.into_response(),
    }
}

// ============================================================================
// Data Source API Handlers
// ============================================================================

/// List all available data sources.
async fn list_data_sources(State(state): State<AppState>) -> impl IntoResponse {
    let sources: Vec<DataSourceInfo> = state
        .data_sources
        .list_with_metadata()
        .into_iter()
        .collect();
    Json(json!({ "sources": sources }))
}

/// Get info about a specific data source.
async fn get_data_source_info(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> impl IntoResponse {
    match state.data_sources.get_info(&source_id) {
        Some(info) => Json(json!(info)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Source not found" })),
        )
            .into_response(),
    }
}

/// Get a single tile from a data source.
async fn get_source_tile(
    State(state): State<AppState>,
    AxumPath((source_id, z, x, y)): AxumPath<(String, u8, u32, u32)>,
) -> impl IntoResponse {
    let source = match state.data_sources.get(&source_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Source not found" })),
            )
                .into_response()
        }
    };

    let coord = streaming::TileCoord::new(z, x, y);
    match source.get_tile(coord).await {
        Ok(Some(data)) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "Content-Type",
                HeaderValue::from_static("application/octet-stream"),
            );
            (StatusCode::OK, headers, Body::from(data)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Tile not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Batch request for tiles from a data source.
#[derive(serde::Deserialize)]
struct BatchTileRequest {
    tiles: Vec<TileCoordRequest>,
}

#[derive(serde::Deserialize)]
struct TileCoordRequest {
    z: u8,
    x: u32,
    y: u32,
}

#[derive(serde::Serialize)]
struct BatchTileResponse {
    tiles: Vec<TileResult>,
}

#[derive(serde::Serialize)]
struct TileResult {
    z: u8,
    x: u32,
    y: u32,
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Get multiple tiles from a data source in one request.
async fn get_source_tiles_batch(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
    Json(request): Json<BatchTileRequest>,
) -> impl IntoResponse {
    let source = match state.data_sources.get(&source_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Source not found" })),
            )
                .into_response()
        }
    };

    let coords: Vec<streaming::TileCoord> = request
        .tiles
        .iter()
        .map(|t| streaming::TileCoord::new(t.z, t.x, t.y))
        .collect();

    let results = source.get_tiles(coords.clone()).await;

    let tiles: Vec<TileResult> = coords
        .iter()
        .zip(results.into_iter())
        .map(|(coord, result)| match result {
            Ok(Some(data)) => TileResult {
                z: coord.z,
                x: coord.x,
                y: coord.y,
                found: true,
                data_base64: Some(base64_encode(&data)),
                error: None,
            },
            Ok(None) => TileResult {
                z: coord.z,
                x: coord.x,
                y: coord.y,
                found: false,
                data_base64: None,
                error: None,
            },
            Err(e) => TileResult {
                z: coord.z,
                x: coord.x,
                y: coord.y,
                found: false,
                data_base64: None,
                error: Some(e.to_string()),
            },
        })
        .collect();

    Json(BatchTileResponse { tiles }).into_response()
}

/// Check if a data source has a specific tile.
async fn check_source_has_tile(
    State(state): State<AppState>,
    AxumPath((source_id, z, x, y)): AxumPath<(String, u8, u32, u32)>,
) -> impl IntoResponse {
    let coord = streaming::TileCoord::new(z, x, y);
    match state.data_sources.has_tile(&source_id, coord).await {
        Some(true) => Json(json!({ "has_tile": true })),
        Some(false) => Json(json!({ "has_tile": false })),
        None => Json(json!({ "error": "Source not found", "has_tile": false })),
    }
}

/// Request to create a new data source.
#[derive(serde::Deserialize)]
struct CreateDataSourceRequest {
    /// Unique identifier for the source.
    id: String,
    /// Type of data source: "filesystem", "http", "pmtiles", "memory", "fallback".
    source_type: String,
    /// Path (for filesystem/pmtiles) or URL template (for http).
    path_or_url: Option<String>,
    /// Display name for the source.
    name: Option<String>,
    /// Tile format: "mvt", "png", "jpg", "webp", "quantized_mesh".
    format: Option<String>,
    /// File extension for filesystem sources (e.g., "png", "bin").
    extension: Option<String>,
    /// Attribution string for the source.
    attribution: Option<String>,
    /// Description of the source.
    description: Option<String>,
    /// Minimum zoom level.
    min_zoom: Option<u8>,
    /// Maximum zoom level.
    max_zoom: Option<u8>,
    /// For fallback source: list of existing source IDs to try in order.
    fallback_sources: Option<Vec<String>>,
}

/// Create a new data source dynamically.
async fn create_data_source(
    State(state): State<AppState>,
    Json(request): Json<CreateDataSourceRequest>,
) -> impl IntoResponse {
    use streaming::TileFormat;

    // Parse tile format
    let format = match request.format.as_deref() {
        Some("mvt") | None => TileFormat::Mvt,
        Some("png") => TileFormat::Png,
        Some("jpg") | Some("jpeg") => TileFormat::Jpeg,
        Some("webp") => TileFormat::Webp,
        Some("quantized_mesh") | Some("terrain") => TileFormat::QuantizedMesh,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Unknown tile format: {}", other) })),
            )
                .into_response()
        }
    };

    let extension = request.extension.clone().unwrap_or_else(|| match format {
        streaming::TileFormat::Png => "png".to_string(),
        streaming::TileFormat::Jpeg => "jpg".to_string(),
        streaming::TileFormat::Webp => "webp".to_string(),
        streaming::TileFormat::Mvt => "mvt".to_string(),
        streaming::TileFormat::QuantizedMesh => "bin".to_string(),
        streaming::TileFormat::GeoJson => "geojson".to_string(),
        streaming::TileFormat::HeightmapF32 | streaming::TileFormat::HeightmapI16 => {
            "bin".to_string()
        }
        streaming::TileFormat::Other => "bin".to_string(),
    });

    // Create the appropriate source type
    let name = request.name.clone().unwrap_or_else(|| request.id.clone());

    // Helper to build metadata with overrides
    let build_metadata =
        |source_name: String, default_format: streaming::TileFormat| DataSourceMetadata {
            name: source_name,
            description: request.description.clone(),
            attribution: request.attribution.clone(),
            min_zoom: request.min_zoom.unwrap_or(0),
            max_zoom: request.max_zoom.unwrap_or(22),
            bounds: None,
            center: None,
            format: default_format,
            layers: vec![],
        };

    let source: Arc<dyn DataSource + Send + Sync> = match request.source_type.as_str() {
        "filesystem" => {
            let path = match &request.path_or_url {
                Some(p) => p.clone(),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "path_or_url required for filesystem source" })),
                    )
                        .into_response()
                }
            };
            Arc::new(
                FilesystemSource::new(&path, &name, format, &extension)
                    .with_metadata(build_metadata(name.clone(), format)),
            )
        }
        "http" => {
            let url_template = match &request.path_or_url {
                Some(u) => u.clone(),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "path_or_url required for http source" })),
                    )
                        .into_response()
                }
            };
            Arc::new(
                HttpSource::new(&url_template, &name, format)
                    .with_metadata(build_metadata(name.clone(), format)),
            )
        }
        "pmtiles" => {
            let path = match &request.path_or_url {
                Some(p) => p.clone(),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "path_or_url required for pmtiles source" })),
                    )
                        .into_response()
                }
            };
            Arc::new(
                PmtilesSource::new(&path, &name)
                    .with_metadata(build_metadata(name.clone(), streaming::TileFormat::Mvt)),
            )
        }
        "memory" => Arc::new(MemorySource::new(&name, format)),
        "fallback" => {
            let source_ids = match &request.fallback_sources {
                Some(ids) if !ids.is_empty() => ids,
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "fallback_sources required for fallback source" })),
                    )
                        .into_response()
                }
            };
            // Collect the referenced sources
            let sources: Vec<Arc<dyn DataSource>> = source_ids
                .iter()
                .filter_map(|id| state.data_sources.get(id))
                .map(|s| s as Arc<dyn DataSource>)
                .collect();
            if sources.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "No valid fallback sources found" })),
                )
                    .into_response();
            }
            Arc::new(FallbackSource::new(&name, sources))
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Unknown source type: {}", other) })),
            )
                .into_response()
        }
    };

    state.data_sources.register(&request.id, source);
    (
        StatusCode::CREATED,
        Json(json!({ "id": request.id, "created": true })),
    )
        .into_response()
}

/// Delete a data source.
async fn delete_data_source(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> impl IntoResponse {
    match state.data_sources.unregister(&source_id) {
        Some(_) => Json(json!({ "id": source_id, "deleted": true })),
        None => Json(json!({ "error": "Source not found", "deleted": false })),
    }
}

/// Write a tile to a memory data source.
async fn put_memory_tile(
    State(state): State<AppState>,
    AxumPath((source_id, z, x, y)): AxumPath<(String, u8, u32, u32)>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let coord = streaming::TileCoord::new(z, x, y);

    // Get the source and check if it's a MemorySource
    let source = match state.data_sources.get(&source_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Source not found" })),
            )
        }
    };

    // Try to downcast to MemorySource
    if let Some(memory_source) = source.as_any().downcast_ref::<MemorySource>() {
        memory_source.set_tile(coord, body.to_vec()).await;
        (
            StatusCode::OK,
            Json(json!({ "z": z, "x": x, "y": y, "written": true })),
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Source is not a memory source" })),
        )
    }
}

/// Delete a tile from a memory data source.
async fn delete_memory_tile(
    State(state): State<AppState>,
    AxumPath((source_id, z, x, y)): AxumPath<(String, u8, u32, u32)>,
) -> impl IntoResponse {
    let coord = streaming::TileCoord::new(z, x, y);

    let source = match state.data_sources.get(&source_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Source not found" })),
            )
        }
    };

    if let Some(memory_source) = source.as_any().downcast_ref::<MemorySource>() {
        let removed = memory_source.remove_tile(coord).await.is_some();
        (
            StatusCode::OK,
            Json(json!({ "z": z, "x": x, "y": y, "deleted": removed })),
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Source is not a memory source" })),
        )
    }
}

/// Simple base64 encoding for tile data.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        result.push(ALPHABET[(b0 >> 2) as usize] as char);
        result.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

// ============================================================================
// Webhook API Handlers
// ============================================================================

/// List all registered webhook sources.
async fn list_webhook_sources(State(state): State<AppState>) -> impl IntoResponse {
    let sources = state.webhooks.list_sources();
    Json(json!({ "sources": sources }))
}

/// Get info about a specific webhook source.
async fn get_webhook_source_info(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> impl IntoResponse {
    match state.webhooks.get_source_info(&source_id) {
        Some(info) => Json(json!(info)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Webhook source not found" })),
        )
            .into_response(),
    }
}

/// Request to create a new webhook source.
#[derive(serde::Deserialize)]
struct CreateWebhookSourceRequest {
    /// Unique identifier for the source.
    id: String,
    /// Display name.
    name: String,
    /// Description of the webhook source.
    description: Option<String>,
    /// Schema definition for data validation.
    schema: Option<WebhookSchema>,
    /// Optional JSONPath/jq-like transform expression.
    transform: Option<String>,
}

/// Create a new webhook source.
async fn create_webhook_source(
    State(state): State<AppState>,
    Json(request): Json<CreateWebhookSourceRequest>,
) -> impl IntoResponse {
    let source = WebhookSource {
        id: request.id.clone(),
        name: request.name,
        description: request.description,
        schema: request.schema.unwrap_or(WebhookSchema::Raw),
        transform: request.transform,
    };
    state.webhooks.register_source(source);
    (
        StatusCode::CREATED,
        Json(json!({ "id": request.id, "created": true })),
    )
}

/// Delete a webhook source.
async fn delete_webhook_source(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> impl IntoResponse {
    state.webhooks.unregister_source(&source_id);
    Json(json!({ "id": source_id, "deleted": true }))
}

/// WebSocket handler for real-time webhook data streaming.
async fn ws_realtime_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_realtime_ws(socket, state.webhooks.clone()))
}

/// Handle real-time WebSocket connection - streams webhook data to clients.
async fn handle_realtime_ws(socket: axum::extract::ws::WebSocket, webhooks: Arc<WebhookRegistry>) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut subscriber = webhooks.subscribe();

    // Send hello message
    let hello = json!({
        "type": "hello",
        "message": "Connected to real-time webhook stream"
    });
    if ws_tx
        .send(axum::extract::ws::Message::Text(hello.to_string()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            // Forward webhook data to client
            Ok(update) = subscriber.recv() => {
                let msg = json!({
                    "type": "data",
                    "source_id": update.source_id,
                    "timestamp": update.timestamp.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    "data": update.data
                });
                if ws_tx
                    .send(axum::extract::ws::Message::Text(msg.to_string()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            // Handle client messages (ping/pong, close)
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Close(_))) | None => break,
                    Some(Ok(axum::extract::ws::Message::Ping(data))) => {
                        if ws_tx.send(axum::extract::ws::Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
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
