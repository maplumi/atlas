use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::sync::Mutex;

use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedSpec {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub lat_field: String,
    #[serde(default)]
    pub lon_field: String,
    #[serde(default)]
    pub max_rows: u32,
    #[serde(default)]
    pub filter: String,

    // CSV-specific options.
    #[serde(default)]
    pub csv_skip_rows: u32,
    /// Additional CSV row skips, expressed as 1-based row numbers/ranges.
    /// Example: "2,5,10-12".
    #[serde(default)]
    pub csv_skip: String,
    #[serde(default = "default_true")]
    pub csv_first_row_header: bool,

    // Timestamps (ms since epoch) for simple sync.
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

fn default_true() -> bool {
    true
}

fn api_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

pub struct FeedsStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FeedsStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    async fn load_unlocked(&self) -> Result<Vec<FeedSpec>, String> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(s) => {
                let feeds: Vec<FeedSpec> = serde_json::from_str(&s).map_err(|e| e.to_string())?;
                Ok(feeds)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn save_unlocked(&self, feeds: &[FeedSpec]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }

        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(feeds).map_err(|e| e.to_string())?;
        tokio::fs::write(&tmp, text)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<FeedSpec>, String> {
        let _g = self.lock.lock().await;
        self.load_unlocked().await
    }

    pub async fn upsert(&self, mut feed: FeedSpec, now_ms: u64) -> Result<FeedSpec, String> {
        let _g = self.lock.lock().await;
        let mut feeds = self.load_unlocked().await?;

        // Normalize.
        if feed.format.trim().is_empty() {
            feed.format = "auto".to_string();
        }
        if feed.max_rows == 0 {
            feed.max_rows = 5000;
        }
        feed.updated_at = now_ms;
        if feed.created_at == 0 {
            feed.created_at = now_ms;
        }

        feeds.retain(|f| f.id != feed.id);
        feeds.push(feed.clone());
        self.save_unlocked(&feeds).await?;
        Ok(feed)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, String> {
        let _g = self.lock.lock().await;
        let mut feeds = self.load_unlocked().await?;
        let before = feeds.len();
        feeds.retain(|f| f.id != id);
        let removed = feeds.len() != before;
        if removed {
            self.save_unlocked(&feeds).await?;
        }
        Ok(removed)
    }

    pub async fn get(&self, id: &str) -> Result<Option<FeedSpec>, String> {
        let _g = self.lock.lock().await;
        let feeds = self.load_unlocked().await?;
        Ok(feeds.into_iter().find(|f| f.id == id))
    }
}

pub async fn list_feeds(
    State(state): State<AppState>,
) -> Result<Json<Vec<FeedSpec>>, (StatusCode, Json<Value>)> {
    let feeds = state.feeds.list().await.map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read feeds store: {e}"),
        )
    })?;
    Ok(Json(feeds))
}

pub async fn upsert_feed(
    State(state): State<AppState>,
    Json(feed): Json<FeedSpec>,
) -> Result<Json<FeedSpec>, (StatusCode, Json<Value>)> {
    if feed.id.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Feed id is required"));
    }
    if feed.url.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Feed url is required"));
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let saved = state.feeds.upsert(feed, now_ms).await.map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write feeds store: {e}"),
        )
    })?;

    Ok(Json(saved))
}

pub async fn delete_feed(
    State(state): State<AppState>,
    AxumPath(feed_id): AxumPath<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let removed = state.feeds.delete(&feed_id).await.map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update feeds store: {e}"),
        )
    })?;

    if !removed {
        return Err(api_error(StatusCode::NOT_FOUND, "Feed not found"));
    }

    Ok((StatusCode::NO_CONTENT, ""))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedFetchResponse {
    pub status: u16,
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedFetchRequest {
    pub url: String,
}

async fn fetch_text_via_backend(
    state: &AppState,
    url: &str,
) -> Result<FeedFetchResponse, (StatusCode, Json<Value>)> {
    const MAX_BYTES: usize = 8 * 1024 * 1024;

    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Only http(s) URLs are allowed",
        ));
    }

    let resp = state
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("Fetch failed: {e}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(api_error(
            StatusCode::BAD_GATEWAY,
            format!("Upstream HTTP {status}"),
        ));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("Read failed: {e}")))?;

    if bytes.len() > MAX_BYTES {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Feed payload too large (max {} bytes)", MAX_BYTES),
        ));
    }

    let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Upstream response was not valid UTF-8",
        )
    })?;

    Ok(FeedFetchResponse {
        status,
        content_type,
        text,
    })
}

pub async fn fetch_url(
    State(state): State<AppState>,
    Json(req): Json<FeedFetchRequest>,
) -> Result<Json<FeedFetchResponse>, (StatusCode, Json<Value>)> {
    if req.url.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "url is required"));
    }
    let resp = fetch_text_via_backend(&state, &req.url).await?;
    Ok(Json(resp))
}

pub async fn fetch_feed(
    State(state): State<AppState>,
    AxumPath(feed_id): AxumPath<String>,
) -> Result<Json<FeedFetchResponse>, (StatusCode, Json<Value>)> {
    let Some(feed) = state.feeds.get(&feed_id).await.map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read feeds store: {e}"),
        )
    })?
    else {
        return Err(api_error(StatusCode::NOT_FOUND, "Feed not found"));
    };

    let resp = fetch_text_via_backend(&state, &feed.url).await?;
    Ok(Json(resp))
}
