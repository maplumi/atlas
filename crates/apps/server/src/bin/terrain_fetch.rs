use std::env;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(author, version, about = "STAC downloader for Copernicus DEM tiles")]
struct Args {
    /// Base STAC API URL (default: Copernicus DEM 30m STAC)
    #[arg(long)]
    stac_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List available STAC collections
    ListCollections,

    /// Download COGs for a bbox into a local folder
    Download {
        /// STAC collection id (e.g., from list-collections)
        #[arg(long)]
        collection: String,

        /// Bounding box: minLon,minLat,maxLon,maxLat
        #[arg(long)]
        bbox: String,

        /// Output directory
        #[arg(long, default_value = "data/terrain/raw")]
        out: String,

        /// Max number of items to download
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },

    /// Download global coverage in bbox chunks
    DownloadGlobal {
        /// STAC collection id (e.g., from list-collections)
        #[arg(long)]
        collection: String,

        /// Output directory
        #[arg(long, default_value = "data/terrain/raw")]
        out: String,

        /// Chunk size in degrees (e.g., 5 or 10)
        #[arg(long, default_value_t = 10.0)]
        chunk_deg: f64,

        /// Max number of items per STAC request
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let stac_url = args.stac_url.unwrap_or_else(|| {
        env::var("STAC_URL")
            .unwrap_or_else(|_| "https://copernicus-dem-30m-stac.s3.amazonaws.com".to_string())
    });

    let client = Client::new();

    match args.command {
        Command::ListCollections => list_collections(&client, &stac_url).await?,
        Command::Download {
            collection,
            bbox,
            out,
            limit,
        } => download_bbox(&client, &stac_url, &collection, &bbox, &out, limit).await?,
        Command::DownloadGlobal {
            collection,
            out,
            chunk_deg,
            limit,
        } => download_global(&client, &stac_url, &collection, &out, chunk_deg, limit).await?,
    }

    Ok(())
}

async fn list_collections(
    client: &Client,
    stac_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let trimmed = stac_url.trim_end_matches('/');
    if trimmed.ends_with(".json") {
        let v: serde_json::Value = client.get(trimmed).send().await?.json().await?;
        print_collection(&v);
        return Ok(());
    }

    let url = format!("{trimmed}/collections");
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(format!(
            "STAC collections endpoint not available at {url}. For static collections, pass --stac-url <collection.json>."
        )
        .into());
    }

    let v: serde_json::Value = resp.json().await?;
    let collections = v
        .get("collections")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    for c in collections {
        print_collection(&c);
    }

    Ok(())
}

fn print_collection(c: &serde_json::Value) {
    let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("<unknown>");
    let title = c.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let description = c.get("description").and_then(|v| v.as_str()).unwrap_or("");
    println!("{id}\t{title}\t{description}");
}

async fn download_bbox(
    client: &Client,
    stac_url: &str,
    collection: &str,
    bbox: &str,
    out_dir: &str,
    limit: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let bbox_vals = parse_bbox(bbox)?;
    download_bbox_vals(client, stac_url, collection, bbox_vals, out_dir, limit).await
}

async fn download_bbox_vals(
    client: &Client,
    stac_url: &str,
    collection: &str,
    bbox: [f64; 4],
    out_dir: &str,
    limit: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    // Prefer STAC API Search when available (STAC API), but fall back to the
    // Copernicus DEM static STAC bucket layout which does not support /search.
    let trimmed = stac_url.trim_end_matches('/');
    let url = format!("{trimmed}/search");
    let body = json!({
        "collections": [collection],
        "bbox": bbox,
        "limit": limit
    });

    info!("querying STAC for collection {collection}");
    let out_dir = PathBuf::from(out_dir);
    tokio::fs::create_dir_all(&out_dir).await?;

    let mut next_url: Option<String> = None;
    let mut next_body: Option<serde_json::Value> = Some(body);

    let mut downloaded: u32 = 0;
    let mut used_api_search = false;

    loop {
        let page = stac_search_page(client, &url, next_url.as_deref(), next_body.take()).await;
        let (resp_json, next) = match page {
            Ok(v) => {
                used_api_search = true;
                v
            }
            Err(err) if !used_api_search && is_likely_static_stac_error(err.as_ref()) => {
                info!("STAC API search unavailable; falling back to static Copernicus DEM layout");
                return download_bbox_static_copernicus(
                    client, trimmed, collection, bbox, &out_dir, limit,
                )
                .await;
            }
            Err(err) => return Err(err),
        };

        let features = resp_json
            .get("features")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();

        if features.is_empty() && next.is_none() {
            break;
        }

        for feat in features {
            if downloaded >= limit {
                break;
            }

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
            if out_path.exists() {
                continue;
            }

            info!("downloading {filename}");
            download_file(client, &href, &out_path).await?;
            downloaded += 1;
        }

        if let Some(next_link) = next {
            next_url = Some(next_link.href);
            next_body = next_link.body;
        } else {
            break;
        }
    }

    Ok(())
}

async fn download_global(
    client: &Client,
    stac_url: &str,
    collection: &str,
    out_dir: &str,
    chunk_deg: f64,
    limit: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let step = chunk_deg.clamp(1.0, 30.0);
    let mut lat = -90.0;
    while lat < 90.0 {
        let mut lon = -180.0;
        let next_lat = (lat + step).min(90.0);
        while lon < 180.0 {
            let next_lon = (lon + step).min(180.0);
            let bbox = [lon, lat, next_lon, next_lat];
            info!("chunk bbox: {lon},{lat},{next_lon},{next_lat}");
            download_bbox_vals(client, stac_url, collection, bbox, out_dir, limit).await?;
            lon += step;
        }
        lat += step;
    }
    Ok(())
}

#[derive(Debug)]
struct NextLink {
    href: String,
    body: Option<serde_json::Value>,
}

async fn stac_search_page(
    client: &Client,
    base_url: &str,
    next_url: Option<&str>,
    body: Option<serde_json::Value>,
) -> Result<(serde_json::Value, Option<NextLink>), Box<dyn std::error::Error>> {
    let resp = if let Some(url) = next_url {
        client.get(url).send().await?
    } else if let Some(body) = body {
        client.post(base_url).json(&body).send().await?
    } else {
        client.get(base_url).send().await?
    };

    if !resp.status().is_success() {
        return Err(format!(
            "STAC request failed: {} {}",
            resp.status(),
            next_url.unwrap_or(base_url)
        )
        .into());
    }

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

fn parse_bbox(bbox: &str) -> Result<[f64; 4], Box<dyn std::error::Error>> {
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
    // Copernicus DEM items use `elevation` for the primary COG.
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

fn is_likely_static_stac_error(err: &dyn std::error::Error) -> bool {
    let msg = err.to_string();
    msg.contains(" 404 ")
        || msg.contains(" 405 ")
        || msg.contains(" 403 ")
        || msg.contains("collections endpoint not available")
        || msg.contains("STAC request failed")
}

async fn download_bbox_static_copernicus(
    client: &Client,
    stac_root: &str,
    collection: &str,
    bbox: [f64; 4],
    out_dir: &Path,
    limit: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    // The public bucket exposes a single collection json at the root.
    // Collection id should be `dem_cop_30`.
    if collection != "dem_cop_30" {
        info!("static Copernicus DEM layout detected; overriding collection to dem_cop_30 (got {collection})");
    }

    let min_lon = bbox[0].max(-180.0);
    let min_lat = bbox[1].max(-90.0);
    let max_lon = bbox[2].min(180.0);
    let max_lat = bbox[3].min(90.0);

    if max_lon <= min_lon || max_lat <= min_lat {
        return Err("bbox is empty or invalid".into());
    }

    let lon0 = min_lon.floor() as i32;
    let lon1 = max_lon.ceil() as i32;
    let lat0 = min_lat.floor() as i32;
    let lat1 = max_lat.ceil() as i32;

    let mut downloaded: u32 = 0;

    for lat in lat0..lat1 {
        for lon in lon0..lon1 {
            if downloaded >= limit {
                return Ok(());
            }

            let item_id = copernicus_item_id(lat, lon);
            let item_url = format!("{}/items/{}.json", stac_root.trim_end_matches('/'), item_id);

            let resp = client.get(&item_url).send().await?;
            if !resp.status().is_success() {
                // Missing tiles near edges can happen; continue.
                continue;
            }

            let feat: serde_json::Value = resp.json().await?;
            let assets = feat.get("assets").and_then(|a| a.as_object());
            let Some(assets) = assets else {
                continue;
            };

            let href = select_asset_href(assets);
            let Some(href) = href else {
                continue;
            };

            let filename = file_name_from_href(&href).unwrap_or_else(|| format!("{item_id}.tif"));
            let out_path = out_dir.join(&filename);
            if out_path.exists() {
                continue;
            }

            info!("downloading {filename}");
            download_file(client, &href, &out_path).await?;
            downloaded += 1;
        }
    }

    Ok(())
}

fn copernicus_item_id(lat_deg: i32, lon_deg: i32) -> String {
    // Matches names like:
    //   Copernicus_DSM_COG_10_N06_00_E010_00
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
    client: &Client,
    href: &str,
    out_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let resp = client.get(href).send().await?;
    if !resp.status().is_success() {
        error!("download failed: {href} -> {}", resp.status());
        return Err("download failed".into());
    }

    let mut file = tokio::fs::File::create(out_path).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
    }
    Ok(())
}
