use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    terrain_root: PathBuf,
    stac_url: String,
    http: reqwest::Client,
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
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/terrain/tileset.json", get(get_tileset))
        .route("/terrain/tiles/:z/:x/:y.bin", get(get_tile))
        .route("/stac/collections", get(stac_collections))
        .route("/stac/search", post(stac_search))
        .with_state(state);

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
    serve_file(&path, "application/json").await
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
    serve_file(&path, "application/octet-stream").await
}

async fn stac_collections(State(state): State<AppState>) -> Response {
    let url = format!("{}/collections", state.stac_url.trim_end_matches('/'));
    proxy_get(&state, &url).await
}

async fn stac_search(State(state): State<AppState>, body: Bytes) -> Response {
    let url = format!("{}/search", state.stac_url.trim_end_matches('/'));
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
            error!("file read failed: {path:?} -> {err}");
            (StatusCode::NOT_FOUND, "not found").into_response()
        }
    }
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
