# Streaming and Cache

Streaming is explicit and budgeted.

Resource lifecycle (target model):
Requested → Downloading → Decoding → Building → Uploading → Resident → Evicted

Implemented core primitives:
- `streaming::Cache` with an explicit byte budget and deterministic LRU eviction
- `streaming::ResidencyState` for the lifecycle states above
- `streaming::Pipeline` to submit/cancel requests deterministically under a `FrameBudget`

Dataset version pinning:
- `streaming::Cache::pin_dataset_version(dataset_id, version)` records an immutable version (typically a content hash).
- Pinning deterministically invalidates stale resident entries for that dataset (stable traversal order + deterministic eviction).
- New `request()` / `mark_resident()` calls record the currently pinned version on the entry.

## WebSocket Tile Streaming

The server supports WebSocket-based tile streaming for improved performance compared to HTTP polling.

### Protocol

The client-server protocol is defined in `crates/streaming/src/protocol.rs`:

#### Client Messages
- `UpdateView { view_id, view_state }` - Client sends camera position updates
- `SubscribeSource { source_id }` - Subscribe to a tile data source  
- `UnsubscribeSource { source_id }` - Unsubscribe from a source
- `CancelTiles { tile_ids }` - Cancel in-flight tile requests
- `SetBudget { max_bytes_per_second, max_tiles_in_flight }` - Set bandwidth limits
- `Ping` - Keep-alive

#### Server Messages
- `TileData { coord, source_id, data, format }` - Tile payload (binary or base64)
- `TileNotFound { coord, source_id }` - 404 equivalent
- `TileError { coord, source_id, error }` - Error fetching tile
- `ViewAck { view_id, tiles_queued }` - Acknowledgment of view update
- `SourceStatus { source_id, status, tile_count }` - Source metadata
- `Pong` - Keep-alive response

### View-Driven Prioritization

Tiles are prioritized by:
1. **Angular distance** - Tiles closer to view center have higher priority
2. **Zoom level** - Current zoom level tiles load first
3. **Visibility** - Only tiles within the view frustum are requested

The server maintains a priority queue per view, recomputed when camera moves:

```rust
fn calculate_priority(view: &ViewState, coord: &TileCoord) -> Priority {
    let tile_center = tile_center_ll(coord);
    let angular_dist = haversine(view.lat, view.lon, tile_center.lat, tile_center.lon);
    let zoom_penalty = (coord.z as i32 - view.estimated_zoom() as i32).abs();
    Priority::from(angular_dist + zoom_penalty as f64 * 0.1)
}
```

### Backpressure

The server respects client-specified limits:
- `max_bytes_per_second` - Bandwidth cap
- `max_tiles_in_flight` - Concurrent tile limit

If the client falls behind, the server pauses sending until acknowledged.

## Data Source Abstraction

The `DataSource` trait (`crates/apps/server/src/data_sources.rs`) provides a uniform interface for tile providers:

```rust
pub trait DataSource: Send + Sync {
    fn metadata(&self) -> &DataSourceMetadata;
    fn get_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>>;
    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>>;
    fn get_tiles(&self, coords: Vec<TileCoord>) -> BoxFuture<'_, Vec<Result<...>>>;
}
```

### Implementations

| Source | Description |
|--------|-------------|
| `FilesystemSource` | Local z/x/y directory structure |
| `HttpSource` | Remote TMS/XYZ URL template |
| `PmtilesSource` | PMTiles archive (local or HTTP range) |
| `MemorySource` | In-memory tiles for testing/dynamic generation |
| `FallbackSource` | Composite that tries sources in order |

### Adding New Sources

1. Implement `DataSource` trait
2. Register with `DataSourceRegistry::register()`
3. Clients subscribe via `SubscribeSource` message

## Webhook Ingestion

Real-time data can be pushed to the server via HTTP webhooks:

```
POST /webhook/:source_id
Content-Type: application/json
X-Webhook-Token: <auth_token>

{ "type": "FeatureCollection", "features": [...] }
```

### Configuration

```rust
WebhookConfig {
    max_payload_size: 10 * 1024 * 1024,  // 10MB
    rate_limit_requests_per_minute: 60,
    require_auth: true,
    allowed_origins: vec!["*"],
}
```

### Schema Validation

Webhooks can enforce schema:
- `GeoJson` - Validate as GeoJSON Feature/FeatureCollection
- `Custom { schema }` - JSON Schema validation
- `Raw` - No validation, pass through

### Broadcasting

Ingested data is broadcast to all subscribed WebSocket clients via `tokio::sync::broadcast` channel.
