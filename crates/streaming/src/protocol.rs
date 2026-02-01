//! Streaming protocol types for client-server tile communication.
//!
//! This module defines the wire format for:
//! - View state updates (client → server)
//! - Tile data responses (server → client)
//! - Control messages (both directions)
//!
//! The protocol is designed to be transport-agnostic (WebSocket, HTTP/2 streams, etc.)
//! and supports view-driven tile prioritization.

use serde::{Deserialize, Serialize};

/// Unique identifier for a streaming session.
pub type SessionId = String;

/// Unique identifier for a view state snapshot.
pub type ViewId = u64;

/// Tile coordinate in ZXY scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileCoord {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileCoord {
    pub fn new(z: u8, x: u32, y: u32) -> Self {
        Self { z, x, y }
    }

    /// Returns the number of tiles at this zoom level (2^z × 2^z).
    pub fn tiles_at_zoom(z: u8) -> u64 {
        1u64 << (2 * z as u64)
    }

    /// Returns the geographic bounds of this tile in WGS84 (lon_min, lat_min, lon_max, lat_max).
    pub fn bounds_wgs84(&self) -> (f64, f64, f64, f64) {
        let n = (1u32 << self.z) as f64;
        let lon_min = (self.x as f64 / n) * 360.0 - 180.0;
        let lon_max = ((self.x + 1) as f64 / n) * 360.0 - 180.0;

        // Web Mercator Y flip
        let lat_max = tile_y_to_lat(self.y, self.z);
        let lat_min = tile_y_to_lat(self.y + 1, self.z);

        (lon_min, lat_min, lon_max, lat_max)
    }
}

fn tile_y_to_lat(y: u32, z: u8) -> f64 {
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * (y as f64) / (1u32 << z) as f64;
    (0.5 * (n.exp() - (-n).exp())).atan().to_degrees()
}

/// Camera/view state sent by the client to drive tile prioritization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewState {
    /// Monotonically increasing view ID for ordering and cancellation.
    pub view_id: ViewId,

    /// Camera position in WGS84.
    pub lon: f64,
    pub lat: f64,
    pub altitude_m: f64,

    /// Camera orientation (optional).
    #[serde(default)]
    pub yaw_deg: f64,
    #[serde(default)]
    pub pitch_deg: f64,

    /// Viewport size in pixels.
    pub viewport_width: u32,
    pub viewport_height: u32,

    /// Field of view in degrees (for 3D).
    #[serde(default = "default_fov")]
    pub fov_deg: f64,

    /// Maximum zoom level the client wants.
    #[serde(default = "default_max_zoom")]
    pub max_zoom: u8,

    /// Tile layers/sources the client is interested in.
    #[serde(default)]
    pub layers: Vec<String>,
}

fn default_fov() -> f64 {
    60.0
}

fn default_max_zoom() -> u8 {
    14
}

impl ViewState {
    /// Estimate the appropriate zoom level for the current altitude.
    pub fn estimated_zoom(&self) -> u8 {
        // Rough heuristic: at ~20,000 km altitude, z=0; halve altitude per zoom level.
        let z = (20_000_000.0 / self.altitude_m.max(1.0)).log2().floor() as i32;
        z.clamp(0, self.max_zoom as i32) as u8
    }

    /// Check if a tile is likely visible from this view state.
    pub fn tile_visible(&self, coord: &TileCoord) -> bool {
        let (lon_min, lat_min, lon_max, lat_max) = coord.bounds_wgs84();

        // Simple visibility: check if tile overlaps a region around the camera.
        // A more accurate implementation would use the actual frustum.
        let view_radius = self.view_radius_deg();
        let lon_ok = lon_max >= self.lon - view_radius && lon_min <= self.lon + view_radius;
        let lat_ok = lat_max >= self.lat - view_radius && lat_min <= self.lat + view_radius;

        lon_ok && lat_ok
    }

    /// Estimate the visible radius in degrees based on altitude and FOV.
    fn view_radius_deg(&self) -> f64 {
        // Approximate: at Earth's surface, 1 degree ≈ 111 km.
        // Visible radius ≈ altitude * tan(fov/2) / 111000 (very rough).
        let half_fov_rad = (self.fov_deg / 2.0).to_radians();
        let ground_radius_m = self.altitude_m * half_fov_rad.tan();
        (ground_radius_m / 111_000.0).min(180.0)
    }

    /// Calculate priority for a tile (lower = higher priority).
    pub fn tile_priority(&self, coord: &TileCoord) -> u32 {
        let (lon_min, lat_min, lon_max, lat_max) = coord.bounds_wgs84();
        let tile_center_lon = (lon_min + lon_max) / 2.0;
        let tile_center_lat = (lat_min + lat_max) / 2.0;

        // Distance from camera center (in degrees, rough).
        let dlon = (tile_center_lon - self.lon).abs();
        let dlat = (tile_center_lat - self.lat).abs();
        let dist = (dlon * dlon + dlat * dlat).sqrt();

        // Priority: closer tiles and appropriate zoom levels get lower (better) priority.
        let zoom_diff = (coord.z as i32 - self.estimated_zoom() as i32).unsigned_abs();
        let dist_score = (dist * 1000.0) as u32;

        zoom_diff * 10000 + dist_score
    }
}

/// Message from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Update the current view state.
    ViewUpdate(ViewState),

    /// Request specific tiles (optional, for explicit requests).
    RequestTiles {
        view_id: ViewId,
        tiles: Vec<TileCoord>,
    },

    /// Cancel tiles for an old view.
    CancelView { view_id: ViewId },

    /// Ping for keepalive.
    Ping { seq: u64 },

    /// Subscribe to a data source for real-time updates.
    Subscribe { source: String },

    /// Unsubscribe from a data source.
    Unsubscribe { source: String },
}

/// Message from server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Session established.
    Hello {
        session_id: SessionId,
        server_version: String,
        capabilities: Vec<String>,
    },

    /// Tile data (JSON metadata; binary data follows or is inlined as base64).
    TileHeader {
        view_id: ViewId,
        coord: TileCoord,
        layer: String,
        format: TileFormat,
        size_bytes: u32,
        /// If true, binary data follows immediately (for binary WS frames).
        /// If false, `data_base64` contains the tile data.
        binary_follows: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_base64: Option<String>,
    },

    /// Tile data not available (404 equivalent).
    TileNotFound {
        view_id: ViewId,
        coord: TileCoord,
        layer: String,
    },

    /// Progress update for a view.
    ViewProgress {
        view_id: ViewId,
        tiles_sent: u32,
        tiles_total: u32,
    },

    /// View fully loaded.
    ViewComplete { view_id: ViewId },

    /// Pong response.
    Pong { seq: u64 },

    /// Real-time data update (for subscribed sources).
    DataUpdate {
        source: String,
        /// GeoJSON feature or feature collection.
        data: serde_json::Value,
    },

    /// Error message.
    Error { code: String, message: String },
}

/// Tile data format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileFormat {
    /// Mapbox Vector Tile (protobuf).
    Mvt,
    /// GeoJSON.
    GeoJson,
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// WebP image.
    Webp,
    /// Raw heightmap (f32 little-endian).
    HeightmapF32,
    /// Raw heightmap (i16 little-endian).
    HeightmapI16,
    /// Quantized mesh terrain.
    QuantizedMesh,
    /// Unknown/custom format.
    Other,
}

impl TileFormat {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "mvt" | "pbf" => Self::Mvt,
            "json" | "geojson" => Self::GeoJson,
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "webp" => Self::Webp,
            "bin" | "raw" | "f32" => Self::HeightmapF32,
            "terrain" => Self::QuantizedMesh,
            _ => Self::Other,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Mvt => "application/vnd.mapbox-vector-tile",
            Self::GeoJson => "application/geo+json",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::HeightmapF32 | Self::HeightmapI16 => "application/octet-stream",
            Self::QuantizedMesh => "application/vnd.quantized-mesh",
            Self::Other => "application/octet-stream",
        }
    }
}

/// Configuration for tile streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Maximum tiles to send per view update.
    pub max_tiles_per_view: usize,

    /// Maximum inflight tiles (backpressure).
    pub max_inflight: usize,

    /// Tile priority decay factor for older view IDs.
    pub view_decay_factor: f64,

    /// Minimum interval between view updates (ms) to prevent spam.
    pub min_view_interval_ms: u64,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_tiles_per_view: 256,
            max_inflight: 32,
            view_decay_factor: 0.8,
            min_view_interval_ms: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_coord_bounds() {
        let tile = TileCoord::new(0, 0, 0);
        let (lon_min, lat_min, lon_max, lat_max) = tile.bounds_wgs84();
        assert!((lon_min - (-180.0)).abs() < 0.01);
        assert!((lon_max - 180.0).abs() < 0.01);
        assert!(lat_min < lat_max);
    }

    #[test]
    fn view_state_zoom_estimate() {
        let view = ViewState {
            view_id: 1,
            lon: 0.0,
            lat: 0.0,
            altitude_m: 10_000_000.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            viewport_width: 1920,
            viewport_height: 1080,
            fov_deg: 60.0,
            max_zoom: 14,
            layers: vec![],
        };
        let z = view.estimated_zoom();
        assert!(z <= 2, "high altitude should give low zoom, got {z}");

        let view2 = ViewState {
            altitude_m: 1000.0,
            ..view
        };
        let z2 = view2.estimated_zoom();
        assert!(z2 >= 10, "low altitude should give high zoom, got {z2}");
    }
}
