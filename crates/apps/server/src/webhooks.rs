//! Webhook and event ingestion for real-time data streams.
//!
//! This module provides endpoints and handlers for receiving real-time data
//! via HTTP webhooks. Incoming data is validated, transformed, and broadcast
//! to subscribed WebSocket clients.
//!
//! Supported webhook types:
//! - GeoJSON feature/FeatureCollection
//! - Custom event formats (configurable)
//!
//! Security considerations:
//! - Webhook endpoints can require authentication tokens
//! - Rate limiting prevents abuse
//! - Payload size limits prevent memory exhaustion

use std::collections::HashMap;
use std::time::Instant;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::debug;

/// Configuration for webhook ingestion.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Maximum payload size in bytes.
    pub max_payload_size: usize,
    /// Rate limit: max requests per source per second.
    pub rate_limit_per_second: f64,
    /// Whether to require authentication tokens.
    pub require_auth: bool,
    /// Valid authentication tokens (source_id -> token).
    pub auth_tokens: HashMap<String, String>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            max_payload_size: 10 * 1024 * 1024, // 10 MB
            rate_limit_per_second: 100.0,
            require_auth: false,
            auth_tokens: HashMap::new(),
        }
    }
}

/// Webhook registry and broadcaster.
pub struct WebhookRegistry {
    config: WebhookConfig,
    /// Broadcast channel for data updates.
    broadcaster: broadcast::Sender<DataUpdate>,
    /// Rate limiting state per source.
    rate_limits: RwLock<HashMap<String, RateLimitState>>,
    /// Registered sources and their schemas.
    sources: RwLock<HashMap<String, WebhookSource>>,
}

struct RateLimitState {
    tokens: f64,
    last_update: Instant,
}

/// Registered webhook source.
#[derive(Debug, Clone)]
pub struct WebhookSource {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub schema: WebhookSchema,
    pub transform: Option<String>, // JSONPath or jq-like transform
}

/// Info about a webhook source for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookSourceInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub schema: WebhookSchema,
}

impl From<&WebhookSource> for WebhookSourceInfo {
    fn from(source: &WebhookSource) -> Self {
        Self {
            id: source.id.clone(),
            name: source.name.clone(),
            description: source.description.clone(),
            schema: source.schema.clone(),
        }
    }
}

/// Schema for validating incoming webhook data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookSchema {
    /// GeoJSON Feature or FeatureCollection.
    GeoJson,
    /// Custom JSON with specified fields.
    Custom {
        required_fields: Vec<String>,
        geometry_path: Option<String>,
        timestamp_path: Option<String>,
    },
    /// Raw bytes (no validation).
    Raw,
}

/// Data update broadcast to subscribers.
#[derive(Debug, Clone)]
pub struct DataUpdate {
    pub source_id: String,
    pub timestamp: std::time::SystemTime,
    pub data: serde_json::Value,
}

impl WebhookRegistry {
    pub fn new(config: WebhookConfig) -> Self {
        let (broadcaster, _) = broadcast::channel(1024);
        Self {
            config,
            broadcaster,
            rate_limits: RwLock::new(HashMap::new()),
            sources: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new webhook source.
    pub fn register_source(&self, source: WebhookSource) {
        self.sources.write().insert(source.id.clone(), source);
    }

    /// Unregister a webhook source.
    pub fn unregister_source(&self, source_id: &str) {
        self.sources.write().remove(source_id);
    }

    /// Get a broadcast receiver for data updates.
    pub fn subscribe(&self) -> broadcast::Receiver<DataUpdate> {
        self.broadcaster.subscribe()
    }

    /// List all registered webhook sources.
    pub fn list_sources(&self) -> Vec<WebhookSourceInfo> {
        self.sources
            .read()
            .values()
            .map(WebhookSourceInfo::from)
            .collect()
    }

    /// Get info about a specific webhook source.
    pub fn get_source_info(&self, source_id: &str) -> Option<WebhookSourceInfo> {
        self.sources
            .read()
            .get(source_id)
            .map(WebhookSourceInfo::from)
    }

    /// Check rate limit for a source.
    fn check_rate_limit(&self, source_id: &str) -> bool {
        let mut limits = self.rate_limits.write();
        let now = Instant::now();

        let state = limits
            .entry(source_id.to_string())
            .or_insert(RateLimitState {
                tokens: self.config.rate_limit_per_second,
                last_update: now,
            });

        // Token bucket refill
        let elapsed = now.duration_since(state.last_update).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.config.rate_limit_per_second)
            .min(self.config.rate_limit_per_second * 2.0);
        state.last_update = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Validate authentication for a request.
    fn check_auth(&self, source_id: &str, headers: &HeaderMap) -> bool {
        if !self.config.require_auth {
            return true;
        }

        let expected = match self.config.auth_tokens.get(source_id) {
            Some(t) => t,
            None => return false,
        };

        let provided = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        provided == Some(expected)
    }

    /// Process an incoming webhook.
    pub async fn process_webhook(
        &self,
        source_id: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), WebhookError> {
        // Check auth
        if !self.check_auth(source_id, headers) {
            return Err(WebhookError::Unauthorized);
        }

        // Check rate limit
        if !self.check_rate_limit(source_id) {
            return Err(WebhookError::RateLimited);
        }

        // Check payload size
        if body.len() > self.config.max_payload_size {
            return Err(WebhookError::PayloadTooLarge);
        }

        // Get source config
        let source = self
            .sources
            .read()
            .get(source_id)
            .cloned()
            .ok_or(WebhookError::UnknownSource)?;

        // Parse and validate
        let mut data = self.validate_and_parse(&source.schema, body)?;

        // Apply transform if specified (simple JSONPath-like extraction)
        if let Some(ref transform) = source.transform {
            data = self.apply_transform(transform, data)?;
        }

        // Broadcast to subscribers
        let update = DataUpdate {
            source_id: source_id.to_string(),
            timestamp: std::time::SystemTime::now(),
            data,
        };

        // Ignore send errors (no subscribers)
        let _ = self.broadcaster.send(update);

        debug!("Processed webhook for source: {source_id}");
        Ok(())
    }

    /// Apply a simple JSONPath-like transform to extract data.
    /// Supports paths like "data", "features[0]", "properties.name".
    fn apply_transform(
        &self,
        path: &str,
        mut value: serde_json::Value,
    ) -> Result<serde_json::Value, WebhookError> {
        for segment in path.split('.') {
            // Check for array index: field[n]
            if let Some(bracket_pos) = segment.find('[') {
                let field = &segment[..bracket_pos];
                let idx_str = segment[bracket_pos + 1..].trim_end_matches(']');

                if !field.is_empty() {
                    value = value.get(field).cloned().ok_or_else(|| {
                        WebhookError::InvalidPayload(format!("Field '{}' not found", field))
                    })?;
                }

                if let Ok(idx) = idx_str.parse::<usize>() {
                    value = value.get(idx).cloned().ok_or_else(|| {
                        WebhookError::InvalidPayload(format!("Index {} out of bounds", idx))
                    })?;
                }
            } else if !segment.is_empty() {
                value = value.get(segment).cloned().ok_or_else(|| {
                    WebhookError::InvalidPayload(format!("Field '{}' not found", segment))
                })?;
            }
        }
        Ok(value)
    }

    fn validate_and_parse(
        &self,
        schema: &WebhookSchema,
        body: &[u8],
    ) -> Result<serde_json::Value, WebhookError> {
        match schema {
            WebhookSchema::GeoJson => {
                let value: serde_json::Value = serde_json::from_slice(body)
                    .map_err(|e| WebhookError::InvalidPayload(e.to_string()))?;

                // Validate it's a Feature or FeatureCollection
                let obj = value.as_object().ok_or_else(|| {
                    WebhookError::InvalidPayload("Expected JSON object".to_string())
                })?;

                let typ = obj.get("type").and_then(|t| t.as_str()).ok_or_else(|| {
                    WebhookError::InvalidPayload("Missing 'type' field".to_string())
                })?;

                if typ != "Feature" && typ != "FeatureCollection" {
                    return Err(WebhookError::InvalidPayload(format!(
                        "Expected Feature or FeatureCollection, got {typ}"
                    )));
                }

                Ok(value)
            }
            WebhookSchema::Custom {
                required_fields, ..
            } => {
                let value: serde_json::Value = serde_json::from_slice(body)
                    .map_err(|e| WebhookError::InvalidPayload(e.to_string()))?;

                let obj = value.as_object().ok_or_else(|| {
                    WebhookError::InvalidPayload("Expected JSON object".to_string())
                })?;

                for field in required_fields {
                    if !obj.contains_key(field) {
                        return Err(WebhookError::InvalidPayload(format!(
                            "Missing required field: {field}"
                        )));
                    }
                }

                Ok(value)
            }
            WebhookSchema::Raw => {
                // Just wrap raw bytes as base64 in JSON
                let encoded = base64::encode(body);
                Ok(serde_json::json!({
                    "type": "raw",
                    "data": encoded
                }))
            }
        }
    }
}

/// Webhook processing errors.
#[derive(Debug)]
pub enum WebhookError {
    Unauthorized,
    RateLimited,
    PayloadTooLarge,
    UnknownSource,
    InvalidPayload(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "Unauthorized"),
            Self::RateLimited => write!(f, "Rate limited"),
            Self::PayloadTooLarge => write!(f, "Payload too large"),
            Self::UnknownSource => write!(f, "Unknown source"),
            Self::InvalidPayload(msg) => write!(f, "Invalid payload: {msg}"),
        }
    }
}

impl std::error::Error for WebhookError {}

impl IntoResponse for WebhookError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "Rate limited"),
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large"),
            Self::UnknownSource => (StatusCode::NOT_FOUND, "Unknown source"),
            Self::InvalidPayload(_) => (StatusCode::BAD_REQUEST, "Invalid payload"),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Simple base64 encoding (for raw payloads).
mod base64 {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
        let mut i = 0;

        while i + 2 < data.len() {
            let b0 = data[i];
            let b1 = data[i + 1];
            let b2 = data[i + 2];

            result.push(ALPHABET[(b0 >> 2) as usize] as char);
            result.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            result.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
            result.push(ALPHABET[(b2 & 0x3f) as usize] as char);

            i += 3;
        }

        if i + 1 == data.len() {
            let b0 = data[i];
            result.push(ALPHABET[(b0 >> 2) as usize] as char);
            result.push(ALPHABET[((b0 & 0x03) << 4) as usize] as char);
            result.push('=');
            result.push('=');
        } else if i + 2 == data.len() {
            let b0 = data[i];
            let b1 = data[i + 1];
            result.push(ALPHABET[(b0 >> 2) as usize] as char);
            result.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            result.push(ALPHABET[((b1 & 0x0f) << 2) as usize] as char);
            result.push('=');
        }

        result
    }
}
