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
    let url = format!("{}/search", stac_url.trim_end_matches('/'));
    let body = json!({
        "collections": [collection],
        "bbox": bbox,
        "limit": limit
    });

    info!("querying STAC search for collection {collection}");
    let out_dir = PathBuf::from(out_dir);
    tokio::fs::create_dir_all(&out_dir).await?;

    let mut next_url: Option<String> = None;
    let mut next_body: Option<serde_json::Value> = Some(body);

    loop {
        let (resp_json, next) =
            stac_search_page(client, &url, next_url.as_deref(), next_body.take()).await?;
        let features = resp_json
            .get("features")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();

        if features.is_empty() && next.is_none() {
            break;
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
            if out_path.exists() {
                continue;
            }

            info!("downloading {filename}");
            download_file(client, &href, &out_path).await?;
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
