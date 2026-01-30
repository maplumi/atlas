use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    terrain_root: PathBuf,
    stac_url: String,
    http: reqwest::Client,
    download_state: Arc<RwLock<DownloadState>>,
}

#[derive(Debug, Clone)]
struct DownloadState {
    status: String,
    downloaded: usize,
    total: usize,
    last_error: Option<String>,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            status: "idle".to_string(),
            downloaded: 0,
            total: 0,
            last_error: None,
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let terrain_root = env::var("TERRAIN_ROOT").unwrap_or_else(|_| "data/terrain".to_string());
    let stac_url = env::var("STAC_URL")
        .unwrap_or_else(|_| "https://copernicus-dem-30m-stac.s3.amazonaws.com".to_string());
    let addr: SocketAddr = env::var("TERRAIN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9100".to_string())
        .parse()
        .expect("invalid TERRAIN_ADDR");

    let state = AppState {
        terrain_root: PathBuf::from(terrain_root),
        stac_url,
        http: reqwest::Client::new(),
        download_state: Arc::new(RwLock::new(DownloadState::default())),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS]);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/terrain/status", get(get_terrain_status))
        .route("/terrain/tileset.json", get(get_tileset))
        .route("/terrain/tiles/:z/:x/:y.bin", get(get_tile))
        .route("/stac/collections", get(stac_collections))
        .route("/stac/search", post(stac_search))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    if env_truthy("TERRAIN_AUTO_DOWNLOAD") {
        let collection = env::var("TERRAIN_COLLECTION").unwrap_or_default();
        let bbox = env::var("TERRAIN_BBOX").unwrap_or_default();
        let limit: u32 = env::var("TERRAIN_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);

        if collection.trim().is_empty() || bbox.trim().is_empty() {
            warn!("TERRAIN_AUTO_DOWNLOAD enabled but TERRAIN_COLLECTION or TERRAIN_BBOX missing");
            let download_state = state.download_state.clone();
            tokio::spawn(async move {
                let mut st = download_state.write().await;
                st.status = "error".to_string();
                st.last_error = Some(
                    "TERRAIN_COLLECTION and TERRAIN_BBOX are required to auto-download".to_string(),
                );
            });
        } else {
            let state_clone = state.clone();
            tokio::spawn(async move {
                if let Err(err) = run_auto_download(state_clone, &collection, &bbox, limit).await {
                    error!("auto-download failed: {err}");
                }
            });
        }
    }

    info!("terrain server listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}

async fn healthz() -> Response {
    (StatusCode::OK, "ok").into_response()
}

async fn get_tileset(State(state): State<AppState>) -> Response {
    let path = state.terrain_root.join("metadata").join("tileset.json");
    info!("tileset request: {}", path.display());
    serve_file(&path, "application/json").await
}

async fn get_terrain_status(State(state): State<AppState>) -> Response {
    let tileset_path = state.terrain_root.join("metadata").join("tileset.json");
    let tiles_dir = state.terrain_root.join("tiles");

    let tileset_present = tokio::fs::metadata(&tileset_path).await.is_ok();
    let (tiles_count, tiles_truncated) = count_tiles(&tiles_dir, 200_000).await;
    let status = if tileset_present {
        "ready"
    } else if tiles_count > 0 {
        "partial"
    } else {
        "missing"
    };

    let download = state.download_state.read().await;
    let body = json!({
        "status": status,
        "tileset_present": tileset_present,
        "tiles_count": tiles_count,
        "tiles_count_truncated": tiles_truncated,
        "download_status": download.status,
        "downloaded": download.downloaded,
        "download_total": download.total,
        "download_last_error": download.last_error,
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
    AxumPath((z, x, y)): AxumPath<(u32, u32, u32)>,
) -> Response {
    let path = state
        .terrain_root
        .join("tiles")
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin"));
    info!("tile request: z={z} x={x} y={y} path={}", path.display());
    serve_file(&path, "application/octet-stream").await
}

async fn stac_collections(State(state): State<AppState>) -> Response {
    let url = format!("{}/collections", state.stac_url.trim_end_matches('/'));
    info!("stac collections proxy -> {url}");
    proxy_get(&state, &url).await
}

async fn stac_search(State(state): State<AppState>, body: Bytes) -> Response {
    let url = format!("{}/search", state.stac_url.trim_end_matches('/'));
    info!("stac search proxy -> {url}");
    proxy_post(&state, &url, body).await
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

async fn proxy_get(state: &AppState, url: &str) -> Response {
    match state.http.get(url).send().await {
        Ok(resp) => map_proxy_response(resp).await,
        Err(err) => {
            error!("stac GET failed: {err}");
            (StatusCode::BAD_GATEWAY, "stac unavailable").into_response()
        }
    }
}

async fn proxy_post(state: &AppState, url: &str, body: Bytes) -> Response {
    match state.http.post(url).body(body).send().await {
        Ok(resp) => map_proxy_response(resp).await,
        Err(err) => {
            error!("stac POST failed: {err}");
            (StatusCode::BAD_GATEWAY, "stac unavailable").into_response()
        }
    }
}

async fn map_proxy_response(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    match resp.bytes().await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/json")),
            );
            (status, headers, Body::from(bytes)).into_response()
        }
        Err(err) => {
            error!("proxy response read failed: {err}");
            (StatusCode::BAD_GATEWAY, "stac unavailable").into_response()
        }
    }
}

fn env_truthy(key: &str) -> bool {
    env::var(key)
        .ok()
        .map(|v| {
            let v = v.to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

async fn run_auto_download(
    state: AppState,
    collection: &str,
    bbox: &str,
    limit: u32,
) -> Result<(), String> {
    let bbox_vals = parse_bbox(bbox).map_err(|e| e.to_string())?;
    let out_dir = state.terrain_root.join("raw");
    if let Err(err) = tokio::fs::create_dir_all(&out_dir).await {
        return Err(format!("create raw dir failed: {err}"));
    }

    {
        let mut st = state.download_state.write().await;
        st.status = "running".to_string();
        st.downloaded = 0;
        st.total = 0;
        st.last_error = None;
    }

    let search_url = format!("{}/search", state.stac_url.trim_end_matches('/'));
    let mut next_url: Option<String> = None;
    let mut next_body: Option<serde_json::Value> = Some(json!({
        "collections": [collection],
        "bbox": bbox_vals,
        "limit": limit,
    }));

    loop {
        let (resp_json, next) = stac_search_page(
            &state.http,
            &search_url,
            next_url.as_deref(),
            next_body.take(),
        )
        .await
        .map_err(|e| e.to_string())?;

        let features = resp_json
            .get("features")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();

        if features.is_empty() && next.is_none() {
            break;
        }

        {
            let mut st = state.download_state.write().await;
            st.total = st.total.saturating_add(features.len());
        }

        for feat in features {
            let assets = feat.get("assets").and_then(|a| a.as_object());
            let Some(assets) = assets else {
                continue;
            };

            let href = select_asset_href(assets);
            let Some(href) = href else {
                continue;
            };

            let filename = file_name_from_href(&href).unwrap_or_else(|| "tile.tif".to_string());
            let out_path = out_dir.join(&filename);
            if tokio::fs::metadata(&out_path).await.is_ok() {
                continue;
            }

            info!("downloading DEM COG {filename}");
            if let Err(err) = download_file(&state.http, &href, &out_path).await {
                let err_str = err.to_string();
                error!("download failed: {href} -> {err_str}");
                drop(err);
                let mut st = state.download_state.write().await;
                st.last_error = Some(format!("download failed: {href} -> {err_str}"));
                continue;
            }

            let mut st = state.download_state.write().await;
            st.downloaded = st.downloaded.saturating_add(1);
        }

        if let Some(next_link) = next {
            next_url = Some(next_link.href);
            next_body = next_link.body;
        } else {
            break;
        }
    }

    let mut st = state.download_state.write().await;
    st.status = "complete".to_string();
    Ok(())
}

#[derive(Debug)]
struct NextLink {
    href: String,
    body: Option<serde_json::Value>,
}

async fn stac_search_page(
    client: &reqwest::Client,
    base_url: &str,
    next_url: Option<&str>,
    body: Option<serde_json::Value>,
) -> Result<(serde_json::Value, Option<NextLink>), Box<dyn std::error::Error + Send + Sync>> {
    let resp = if let Some(url) = next_url {
        client.get(url).send().await?
    } else if let Some(body) = body {
        client.post(base_url).json(&body).send().await?
    } else {
        client.get(base_url).send().await?
    };

    let v: serde_json::Value = resp.json().await?;
    let next = v
        .get("links")
        .and_then(|l| l.as_array())
        .and_then(|links| {
            links.iter().find(|link| {
                link.get("rel")
                    .and_then(|r| r.as_str())
                    .map(|r| r == "next")
                    .unwrap_or(false)
            })
        })
        .and_then(|link| {
            let href = link.get("href").and_then(|h| h.as_str())?.to_string();
            let body = link.get("body").cloned();
            Some(NextLink { href, body })
        });

    Ok((v, next))
}

fn parse_bbox(bbox: &str) -> Result<[f64; 4], Box<dyn std::error::Error + Send + Sync>> {
    let parts: Vec<_> = bbox.split(',').collect();
    if parts.len() != 4 {
        return Err("bbox must be minLon,minLat,maxLon,maxLat".into());
    }
    let min_lon: f64 = parts[0].trim().parse()?;
    let min_lat: f64 = parts[1].trim().parse()?;
    let max_lon: f64 = parts[2].trim().parse()?;
    let max_lat: f64 = parts[3].trim().parse()?;
    Ok([min_lon, min_lat, max_lon, max_lat])
}

fn select_asset_href(assets: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let preferred = ["data", "dem", "cop-dem"];
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

fn file_name_from_href(href: &str) -> Option<String> {
    let url = href.split('?').next().unwrap_or(href);
    let name = url.rsplit('/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

async fn download_file(
    client: &reqwest::Client,
    href: &str,
    out_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let resp = client.get(href).send().await?;
    if !resp.status().is_success() {
        return Err(format!("download failed: {href} -> {}", resp.status()).into());
    }

    let mut file = tokio::fs::File::create(out_path).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        use tokio::io::AsyncWriteExt;
        file.write_all(&chunk).await?;
    }
    Ok(())
}
