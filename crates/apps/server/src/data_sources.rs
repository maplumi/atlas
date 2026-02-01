//! Data source abstraction for tile providers.
//!
//! This module defines the `DataSource` trait and common implementations
//! for various tile providers:
//! - PMTiles (local or remote)
//! - MBTiles (SQLite)
//! - Filesystem (z/x/y directory structure)
//! - Remote HTTP (TMS, XYZ)
//! - STAC catalogs
//!
//! New data sources can be added by implementing the `DataSource` trait.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use streaming::{TileCoord, TileFormat};
use tokio::sync::RwLock;

/// Error type for data source operations.
#[derive(Debug)]
pub struct DataSourceError {
    pub message: String,
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for DataSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DataSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e.as_ref() as _)
    }
}

impl DataSourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

/// Metadata about a data source.
#[derive(Debug, Clone)]
pub struct DataSourceMetadata {
    pub name: String,
    pub description: Option<String>,
    pub attribution: Option<String>,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub bounds: Option<(f64, f64, f64, f64)>, // lon_min, lat_min, lon_max, lat_max
    pub center: Option<(f64, f64, u8)>,       // lon, lat, zoom
    pub format: TileFormat,
    pub layers: Vec<String>,
}

/// Type alias for a boxed future that can be sent between threads.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait for tile data sources.
///
/// Implementations must be `Send + Sync` for use across async tasks.
/// Methods return boxed futures for dyn-compatibility.
pub trait DataSource: Send + Sync {
    /// Get metadata about this data source.
    fn metadata(&self) -> &DataSourceMetadata;

    /// Get the tile format for this source.
    fn tile_format(&self) -> TileFormat {
        self.metadata().format
    }

    /// Get tile data for a given coordinate.
    ///
    /// Returns `Ok(None)` if the tile doesn't exist (equivalent to 404).
    /// Returns `Ok(Some(bytes))` if the tile exists.
    /// Returns `Err` on actual errors (IO, network, etc.).
    fn get_tile(&self, coord: TileCoord)
        -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>>;

    /// Check if this source has a tile without fetching data.
    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>>;

    /// Get tiles for multiple coordinates.
    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>>;
}

/// Filesystem-based tile source (z/x/y.ext directory structure).
pub struct FilesystemSource {
    metadata: DataSourceMetadata,
    root: PathBuf,
    extension: String,
}

impl FilesystemSource {
    pub fn new(
        root: impl AsRef<Path>,
        name: impl Into<String>,
        format: TileFormat,
        extension: impl Into<String>,
    ) -> Self {
        Self {
            metadata: DataSourceMetadata {
                name: name.into(),
                description: None,
                attribution: None,
                min_zoom: 0,
                max_zoom: 22,
                bounds: None,
                center: None,
                format,
                layers: vec![],
            },
            root: root.as_ref().to_path_buf(),
            extension: extension.into(),
        }
    }

    pub fn with_metadata(mut self, metadata: DataSourceMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

impl DataSource for FilesystemSource {
    fn metadata(&self) -> &DataSourceMetadata {
        &self.metadata
    }

    fn get_tile(
        &self,
        coord: TileCoord,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>> {
        let path = self.root.join(format!(
            "{}/{}/{}.{}",
            coord.z, coord.x, coord.y, self.extension
        ));

        Box::pin(async move {
            match tokio::fs::read(&path).await {
                Ok(data) => Ok(Some(data)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(DataSourceError::with_source("Failed to read tile", e)),
            }
        })
    }

    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>> {
        let path = self.root.join(format!(
            "{}/{}/{}.{}",
            coord.z, coord.x, coord.y, self.extension
        ));
        Box::pin(async move { Ok(tokio::fs::metadata(&path).await.is_ok()) })
    }

    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(coords.len());
            for coord in coords {
                results.push(self.get_tile(coord).await);
            }
            results
        })
    }
}

/// HTTP-based tile source (TMS/XYZ URL template).
pub struct HttpSource {
    metadata: DataSourceMetadata,
    url_template: String,
    client: reqwest::Client,
}

impl HttpSource {
    pub fn new(
        url_template: impl Into<String>,
        name: impl Into<String>,
        format: TileFormat,
    ) -> Self {
        Self {
            metadata: DataSourceMetadata {
                name: name.into(),
                description: None,
                attribution: None,
                min_zoom: 0,
                max_zoom: 22,
                bounds: None,
                center: None,
                format,
                layers: vec![],
            },
            url_template: url_template.into(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_metadata(mut self, metadata: DataSourceMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    fn tile_url(&self, coord: TileCoord) -> String {
        self.url_template
            .replace("{z}", &coord.z.to_string())
            .replace("{x}", &coord.x.to_string())
            .replace("{y}", &coord.y.to_string())
    }
}

impl DataSource for HttpSource {
    fn metadata(&self) -> &DataSourceMetadata {
        &self.metadata
    }

    fn get_tile(
        &self,
        coord: TileCoord,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>> {
        let url = self.tile_url(coord);
        Box::pin(async move {
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(|e| DataSourceError::with_source("HTTP request failed", e))?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }

            if !resp.status().is_success() {
                return Err(DataSourceError::new(format!(
                    "HTTP error: {}",
                    resp.status()
                )));
            }

            let bytes = resp
                .bytes()
                .await
                .map_err(|e| DataSourceError::with_source("Failed to read response", e))?;

            Ok(Some(bytes.to_vec()))
        })
    }

    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>> {
        Box::pin(async move { Ok(self.get_tile(coord).await?.is_some()) })
    }

    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(coords.len());
            for coord in coords {
                results.push(self.get_tile(coord).await);
            }
            results
        })
    }
}

/// PMTiles data source (local file or remote URL).
///
/// This is a placeholder; full implementation would use the pmtiles crate.
pub struct PmtilesSource {
    metadata: DataSourceMetadata,
    path_or_url: String,
    // In a full implementation, this would hold a PMTiles reader.
}

impl PmtilesSource {
    pub fn new(path_or_url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            metadata: DataSourceMetadata {
                name: name.into(),
                description: None,
                attribution: None,
                min_zoom: 0,
                max_zoom: 14,
                bounds: None,
                center: None,
                format: TileFormat::Mvt,
                layers: vec![],
            },
            path_or_url: path_or_url.into(),
        }
    }

    pub fn with_metadata(mut self, metadata: DataSourceMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

impl DataSource for PmtilesSource {
    fn metadata(&self) -> &DataSourceMetadata {
        &self.metadata
    }

    fn get_tile(
        &self,
        _coord: TileCoord,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>> {
        // Placeholder: full implementation would use HTTP range requests
        // or local file reading with the pmtiles crate.
        Box::pin(async move {
            Err(DataSourceError::new(
                "PMTiles source not fully implemented yet",
            ))
        })
    }

    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>> {
        Box::pin(async move { Ok(self.get_tile(coord).await?.is_some()) })
    }

    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(coords.len());
            for coord in coords {
                results.push(self.get_tile(coord).await);
            }
            results
        })
    }
}

/// In-memory tile source for testing or dynamic tile generation.
pub struct MemorySource {
    metadata: DataSourceMetadata,
    tiles: RwLock<std::collections::HashMap<TileCoord, Vec<u8>>>,
}

impl MemorySource {
    pub fn new(name: impl Into<String>, format: TileFormat) -> Self {
        Self {
            metadata: DataSourceMetadata {
                name: name.into(),
                description: None,
                attribution: None,
                min_zoom: 0,
                max_zoom: 22,
                bounds: None,
                center: None,
                format,
                layers: vec![],
            },
            tiles: RwLock::new(std::collections::HashMap::new()),
        }
    }

    pub async fn set_tile(&self, coord: TileCoord, data: Vec<u8>) {
        self.tiles.write().await.insert(coord, data);
    }

    pub async fn remove_tile(&self, coord: TileCoord) -> Option<Vec<u8>> {
        self.tiles.write().await.remove(&coord)
    }
}

impl DataSource for MemorySource {
    fn metadata(&self) -> &DataSourceMetadata {
        &self.metadata
    }

    fn get_tile(
        &self,
        coord: TileCoord,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>> {
        Box::pin(async move { Ok(self.tiles.read().await.get(&coord).cloned()) })
    }

    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>> {
        Box::pin(async move { Ok(self.tiles.read().await.contains_key(&coord)) })
    }

    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(coords.len());
            for coord in coords {
                results.push(self.get_tile(coord).await);
            }
            results
        })
    }
}

/// Composite source that tries multiple sources in order.
pub struct FallbackSource {
    metadata: DataSourceMetadata,
    sources: Vec<Arc<dyn DataSource>>,
}

impl FallbackSource {
    pub fn new(name: impl Into<String>, sources: Vec<Arc<dyn DataSource>>) -> Self {
        let format = sources
            .first()
            .map(|s| s.tile_format())
            .unwrap_or(TileFormat::Other);

        Self {
            metadata: DataSourceMetadata {
                name: name.into(),
                description: Some("Fallback source".to_string()),
                attribution: None,
                min_zoom: 0,
                max_zoom: 22,
                bounds: None,
                center: None,
                format,
                layers: vec![],
            },
            sources,
        }
    }
}

impl DataSource for FallbackSource {
    fn metadata(&self) -> &DataSourceMetadata {
        &self.metadata
    }

    fn get_tile(
        &self,
        coord: TileCoord,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, DataSourceError>> {
        Box::pin(async move {
            for source in &self.sources {
                match source.get_tile(coord).await {
                    Ok(Some(data)) => return Ok(Some(data)),
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::debug!("Fallback source error: {e}");
                        continue;
                    }
                }
            }
            Ok(None)
        })
    }

    fn has_tile(&self, coord: TileCoord) -> BoxFuture<'_, Result<bool, DataSourceError>> {
        Box::pin(async move { Ok(self.get_tile(coord).await?.is_some()) })
    }

    fn get_tiles(
        &self,
        coords: Vec<TileCoord>,
    ) -> BoxFuture<'_, Vec<Result<Option<Vec<u8>>, DataSourceError>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(coords.len());
            for coord in coords {
                results.push(self.get_tile(coord).await);
            }
            results
        })
    }
}
