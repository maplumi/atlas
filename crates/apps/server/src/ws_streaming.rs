//! WebSocket tile streaming handler.
//!
//! This module implements view-driven tile streaming over WebSocket:
//! - Client sends ViewState updates as camera moves
//! - Server prioritizes tiles by visibility and distance
//! - Server pushes tile data with backpressure control
//! - Supports multiple data sources (PMTiles, filesystem, remote)

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use streaming::{ClientMessage, ServerMessage, StreamingConfig, TileCoord, ViewId, ViewState};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::data_sources::DataSource;

/// Per-session state for a WebSocket connection.
pub struct WsSession {
    pub session_id: String,
    pub config: StreamingConfig,
    pub current_view: Option<ViewState>,
    pub last_view_time: Instant,
    pub inflight_tiles: HashSet<(ViewId, TileCoord)>,
    pub data_sources: Arc<DataSourceRegistry>,
    pub subscriptions: HashSet<String>,
}

impl WsSession {
    pub fn new(data_sources: Arc<DataSourceRegistry>, config: StreamingConfig) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            config,
            current_view: None,
            last_view_time: Instant::now() - Duration::from_secs(10),
            inflight_tiles: HashSet::new(),
            data_sources,
            subscriptions: HashSet::new(),
        }
    }
}

/// Registry of available data sources.
pub struct DataSourceRegistry {
    sources: RwLock<HashMap<String, Arc<dyn DataSource + Send + Sync>>>,
}

impl DataSourceRegistry {
    pub fn new() -> Self {
        Self {
            sources: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, name: &str, source: Arc<dyn DataSource + Send + Sync>) {
        self.sources.write().insert(name.to_string(), source);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn DataSource + Send + Sync>> {
        self.sources.read().get(name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.sources.read().keys().cloned().collect()
    }
}

impl Default for DataSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tile with priority for the priority queue.
#[derive(Debug, Clone)]
struct PrioritizedTile {
    coord: TileCoord,
    layer: String,
    priority: u32,
    view_id: ViewId,
}

impl PartialEq for PrioritizedTile {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for PrioritizedTile {}

impl PartialOrd for PrioritizedTile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedTile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering: lower priority value = higher priority = should come first
        other.priority.cmp(&self.priority)
    }
}

/// Handle a WebSocket connection for tile streaming.
pub async fn handle_ws_connection(
    socket: WebSocket,
    data_sources: Arc<DataSourceRegistry>,
    config: StreamingConfig,
) {
    let mut session = WsSession::new(data_sources, config);
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send hello message
    let hello = ServerMessage::Hello {
        session_id: session.session_id.clone(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![
            "view_streaming".to_string(),
            "tile_priority".to_string(),
            "subscriptions".to_string(),
        ],
    };

    if let Err(e) = ws_tx
        .send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
    {
        error!("Failed to send hello: {e}");
        return;
    }

    info!("WS session {} connected", session.session_id);

    // Channel for sending tiles from the tile scheduler
    let (tile_tx, mut tile_rx) = mpsc::channel::<ServerMessage>(256);

    // Spawn tile sender task
    let sender_task = tokio::spawn(async move {
        while let Some(msg) = tile_rx.recv().await {
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to serialize message: {e}");
                    continue;
                }
            };
            if let Err(e) = ws_tx.send(Message::Text(text)).await {
                warn!("Failed to send message: {e}");
                break;
            }
        }
    });

    // Main message loop
    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!("WS receive error: {e}");
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                if let Err(e) = handle_client_message(&mut session, &text, tile_tx.clone()).await {
                    let error_msg = ServerMessage::Error {
                        code: "parse_error".to_string(),
                        message: e.to_string(),
                    };
                    let _ = tile_tx.send(error_msg).await;
                }
            }
            Message::Binary(_) => {
                // Binary messages not expected from client currently
            }
            Message::Ping(data) => {
                let _ = tile_tx
                    .send(ServerMessage::Pong {
                        seq: data.first().copied().unwrap_or(0) as u64,
                    })
                    .await;
            }
            Message::Pong(_) => {}
            Message::Close(_) => {
                info!("WS session {} closed by client", session.session_id);
                break;
            }
        }
    }

    drop(tile_tx);
    let _ = sender_task.await;
    info!("WS session {} disconnected", session.session_id);
}

async fn handle_client_message(
    session: &mut WsSession,
    text: &str,
    tile_tx: mpsc::Sender<ServerMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let msg: ClientMessage = serde_json::from_str(text)?;

    match msg {
        ClientMessage::ViewUpdate(view) => {
            handle_view_update(session, view, tile_tx).await?;
        }
        ClientMessage::RequestTiles { view_id, tiles } => {
            handle_explicit_tile_request(session, view_id, tiles, tile_tx).await?;
        }
        ClientMessage::CancelView { view_id } => {
            // Remove inflight tiles for cancelled view
            session.inflight_tiles.retain(|(vid, _)| *vid != view_id);
            debug!("Cancelled view {view_id}");
        }
        ClientMessage::Ping { seq } => {
            tile_tx.send(ServerMessage::Pong { seq }).await?;
        }
        ClientMessage::Subscribe { source } => {
            session.subscriptions.insert(source.clone());
            debug!("Subscribed to {source}");
        }
        ClientMessage::Unsubscribe { source } => {
            session.subscriptions.remove(&source);
            debug!("Unsubscribed from {source}");
        }
    }

    Ok(())
}

async fn handle_view_update(
    session: &mut WsSession,
    view: ViewState,
    tile_tx: mpsc::Sender<ServerMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Rate limit view updates
    let now = Instant::now();
    if now.duration_since(session.last_view_time)
        < Duration::from_millis(session.config.min_view_interval_ms)
    {
        return Ok(());
    }
    session.last_view_time = now;

    let view_id = view.view_id;
    let estimated_zoom = view.estimated_zoom();

    debug!(
        "View update: id={view_id}, lon={:.2}, lat={:.2}, alt={:.0}m, zoom=z{estimated_zoom}",
        view.lon, view.lat, view.altitude_m
    );

    // Build priority queue of visible tiles
    let mut tile_queue: BinaryHeap<PrioritizedTile> = BinaryHeap::new();

    // Determine which layers to serve
    let layers = if view.layers.is_empty() {
        session.data_sources.list()
    } else {
        view.layers.clone()
    };

    // For each layer, enumerate visible tiles at appropriate zoom levels
    for layer in &layers {
        // Start from estimated zoom and include a few levels below for context
        let min_z = estimated_zoom.saturating_sub(2);
        let max_z = estimated_zoom.min(view.max_zoom);

        for z in min_z..=max_z {
            let tiles_per_side = 1u32 << z;

            // Instead of iterating all tiles, compute the visible tile range
            let _view_radius = view_radius_deg(&view);
            let (x_min, x_max, y_min, y_max) = visible_tile_range(&view, z);

            for x in x_min..=x_max {
                for y in y_min..=y_max {
                    let x = x % tiles_per_side;
                    let y = y.min(tiles_per_side - 1);

                    let coord = TileCoord::new(z, x, y);
                    if !view.tile_visible(&coord) {
                        continue;
                    }

                    // Skip if already inflight
                    if session.inflight_tiles.contains(&(view_id, coord)) {
                        continue;
                    }

                    let priority = view.tile_priority(&coord);
                    tile_queue.push(PrioritizedTile {
                        coord,
                        layer: layer.clone(),
                        priority,
                        view_id,
                    });
                }
            }
        }
    }

    // Send tiles up to limit, respecting inflight cap
    let tiles_to_send = session.config.max_tiles_per_view.min(
        session
            .config
            .max_inflight
            .saturating_sub(session.inflight_tiles.len()),
    );

    let mut sent = 0u32;
    let total = tile_queue.len() as u32;

    while let Some(tile) = tile_queue.pop() {
        if sent >= tiles_to_send as u32 {
            break;
        }

        // Get data source
        let source = match session.data_sources.get(&tile.layer) {
            Some(s) => s,
            None => continue,
        };

        // Fetch tile data
        match source.get_tile(tile.coord).await {
            Ok(Some(data)) => {
                session.inflight_tiles.insert((tile.view_id, tile.coord));

                let msg = ServerMessage::TileHeader {
                    view_id: tile.view_id,
                    coord: tile.coord,
                    layer: tile.layer,
                    format: source.tile_format(),
                    size_bytes: data.len() as u32,
                    binary_follows: false,
                    data_base64: Some(base64_encode(&data)),
                };
                tile_tx.send(msg).await?;
                sent += 1;
            }
            Ok(None) => {
                let msg = ServerMessage::TileNotFound {
                    view_id: tile.view_id,
                    coord: tile.coord,
                    layer: tile.layer,
                };
                tile_tx.send(msg).await?;
            }
            Err(e) => {
                warn!("Tile fetch error: {e}");
            }
        }
    }

    // Send progress update
    let progress = ServerMessage::ViewProgress {
        view_id,
        tiles_sent: sent,
        tiles_total: total,
    };
    tile_tx.send(progress).await?;

    if sent >= total {
        tile_tx
            .send(ServerMessage::ViewComplete { view_id })
            .await?;
    }

    session.current_view = Some(view);
    Ok(())
}

async fn handle_explicit_tile_request(
    session: &mut WsSession,
    view_id: ViewId,
    tiles: Vec<TileCoord>,
    tile_tx: mpsc::Sender<ServerMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let layers = session.data_sources.list();

    for coord in tiles {
        for layer in &layers {
            let source = match session.data_sources.get(layer) {
                Some(s) => s,
                None => continue,
            };

            match source.get_tile(coord).await {
                Ok(Some(data)) => {
                    let msg = ServerMessage::TileHeader {
                        view_id,
                        coord,
                        layer: layer.clone(),
                        format: source.tile_format(),
                        size_bytes: data.len() as u32,
                        binary_follows: false,
                        data_base64: Some(base64_encode(&data)),
                    };
                    tile_tx.send(msg).await?;
                }
                Ok(None) => {
                    let msg = ServerMessage::TileNotFound {
                        view_id,
                        coord,
                        layer: layer.clone(),
                    };
                    tile_tx.send(msg).await?;
                }
                Err(e) => {
                    warn!("Tile fetch error: {e}");
                }
            }
        }
    }

    Ok(())
}

/// Compute the visible tile range for a given view and zoom.
fn visible_tile_range(view: &ViewState, z: u8) -> (u32, u32, u32, u32) {
    let _tiles_per_side = 1u32 << z;
    let radius = view_radius_deg(view);

    let lon_min = view.lon - radius;
    let lon_max = view.lon + radius;
    let lat_min = (view.lat - radius).max(-85.0);
    let lat_max = (view.lat + radius).min(85.0);

    let x_min = lon_to_tile_x(lon_min, z);
    let x_max = lon_to_tile_x(lon_max, z);
    let y_min = lat_to_tile_y(lat_max, z); // Note: Y is flipped
    let y_max = lat_to_tile_y(lat_min, z);

    (x_min, x_max, y_min, y_max)
}

fn lon_to_tile_x(lon: f64, z: u8) -> u32 {
    let n = 1u32 << z;
    let x = ((lon + 180.0) / 360.0 * n as f64).floor() as i32;
    x.clamp(0, n as i32 - 1) as u32
}

fn lat_to_tile_y(lat: f64, z: u8) -> u32 {
    let n = 1u32 << z;
    let lat_rad = lat.to_radians();
    let y = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n as f64).floor() as i32;
    y.clamp(0, n as i32 - 1) as u32
}

fn base64_encode(data: &[u8]) -> String {
    use std::io::Write;
    let mut buf = Vec::with_capacity(data.len() * 4 / 3 + 4);
    let mut encoder = base64_encoder(&mut buf);
    encoder.write_all(data).unwrap();
    drop(encoder);
    String::from_utf8(buf).unwrap()
}

fn base64_encoder<W: std::io::Write>(writer: W) -> impl std::io::Write {
    Base64Encoder {
        writer,
        pending: [0; 3],
        pending_len: 0,
    }
}

struct Base64Encoder<W> {
    writer: W,
    pending: [u8; 3],
    pending_len: usize,
}

impl<W: std::io::Write> std::io::Write for Base64Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        let mut pos = 0;
        while pos < buf.len() {
            // Fill pending buffer
            while self.pending_len < 3 && pos < buf.len() {
                self.pending[self.pending_len] = buf[pos];
                self.pending_len += 1;
                pos += 1;
            }

            if self.pending_len == 3 {
                let b0 = self.pending[0];
                let b1 = self.pending[1];
                let b2 = self.pending[2];

                let out = [
                    ALPHABET[(b0 >> 2) as usize],
                    ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize],
                    ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize],
                    ALPHABET[(b2 & 0x3f) as usize],
                ];
                self.writer.write_all(&out)?;
                self.pending_len = 0;
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        if self.pending_len > 0 {
            let b0 = self.pending[0];
            let b1 = if self.pending_len > 1 {
                self.pending[1]
            } else {
                0
            };
            let b2 = if self.pending_len > 2 {
                self.pending[2]
            } else {
                0
            };

            let mut out = [b'='; 4];
            out[0] = ALPHABET[(b0 >> 2) as usize];
            out[1] = ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize];
            if self.pending_len > 1 {
                out[2] = ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize];
            }
            if self.pending_len > 2 {
                out[3] = ALPHABET[(b2 & 0x3f) as usize];
            }
            self.writer.write_all(&out)?;
            self.pending_len = 0;
        }
        self.writer.flush()
    }
}

impl<W> Drop for Base64Encoder<W> {
    fn drop(&mut self) {
        // Note: We can't flush in Drop for a generic W since W may not be Write.
        // In practice, caller should manually flush if needed before dropping.
    }
}

/// Calculate the view radius in degrees for a given view state.
fn view_radius_deg(view: &ViewState) -> f64 {
    let half_fov_rad = (view.fov_deg / 2.0).to_radians();
    let ground_radius_m = view.altitude_m * half_fov_rad.tan();
    (ground_radius_m / 111_000.0).min(180.0)
}
