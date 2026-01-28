use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let mut args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        return Err(usage());
    }

    let cmd = args[1].clone();
    args.drain(0..2);

    match cmd.as_str() {
        "pack" => cmd_pack(args),
        "unpack" => cmd_unpack(args),
        _ => Err(usage()),
    }
}

fn cmd_pack(args: Vec<String>) -> Result<(), String> {
    // atlas pack <input.geojson> <output.avc> [--blob-dir DIR] [--print-chunk-entry]
    if args.len() < 2 {
        return Err(usage());
    }

    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);

    let mut blob_dir: Option<PathBuf> = None;
    let mut print_chunk_entry = false;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--blob-dir" => {
                i += 1;
                if i >= args.len() {
                    return Err("--blob-dir requires a path".to_string());
                }
                blob_dir = Some(PathBuf::from(&args[i]));
            }
            "--print-chunk-entry" => {
                print_chunk_entry = true;
            }
            other => {
                return Err(format!("unknown arg: {other}\n\n{}", usage()));
            }
        }
        i += 1;
    }

    let input_bytes = fs::read(&input).map_err(|e| format!("read {input:?}: {e}"))?;
    let input_str = std::str::from_utf8(&input_bytes).map_err(|e| format!("utf8: {e}"))?;

    let chunk = formats::VectorChunk::from_geojson_str(input_str)
        .map_err(|e| format!("parse geojson: {e}"))?;

    let avc = chunk
        .to_avc_bytes()
        .map_err(|e| format!("encode avc: {e}"))?;
    fs::write(&output, &avc).map_err(|e| format!("write {output:?}: {e}"))?;

    let chunk_hash = blake3::hash(&avc);
    let chunk_hash_hex = to_hex(chunk_hash.as_bytes());

    let mut source_blob_hash_hex: Option<String> = None;
    if let Some(dir) = blob_dir {
        fs::create_dir_all(&dir).map_err(|e| format!("create blob dir {dir:?}: {e}"))?;
        let blob_hash = blake3::hash(&input_bytes);
        let blob_hash_hex = to_hex(blob_hash.as_bytes());
        let blob_path = dir.join(format!("{blob_hash_hex}.blob"));
        if !blob_path.exists() {
            fs::write(&blob_path, &input_bytes)
                .map_err(|e| format!("write blob {blob_path:?}: {e}"))?;
        }
        source_blob_hash_hex = Some(blob_hash_hex);
    }

    eprintln!("wrote {} (blake3={})", output.display(), chunk_hash_hex);
    if let Some(h) = &source_blob_hash_hex {
        eprintln!("stored source blob (blake3={h})");
    }

    if print_chunk_entry {
        // Emit a JSON snippet that can be pasted into a SceneManifest chunk entry.
        // We only know the output filename/path here.
        let file_name = output
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("chunk.avc");

        let id = output
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("chunk")
            .to_string();

        let kind = infer_manifest_kind(&chunk);
        let (lon_lat_bounds_q, time_bounds_us) = compute_chunk_bounds_q_and_time_us(&chunk);
        let feature_count = chunk.features.len() as u32;

        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), serde_json::Value::String(id));
        obj.insert("kind".to_string(), serde_json::Value::String(kind));
        obj.insert(
            "path".to_string(),
            serde_json::Value::String(file_name.to_string()),
        );
        obj.insert(
            "content_hash".to_string(),
            serde_json::Value::String(chunk_hash_hex),
        );
        if let Some(h) = source_blob_hash_hex {
            obj.insert("source_blob_hash".to_string(), serde_json::Value::String(h));
        }

        obj.insert(
            "feature_count".to_string(),
            serde_json::Value::Number(feature_count.into()),
        );
        obj.insert(
            "lon_lat_bounds_q".to_string(),
            serde_json::Value::Array(
                lon_lat_bounds_q
                    .into_iter()
                    .map(|v| serde_json::Value::Number(serde_json::Number::from(v as i64)))
                    .collect(),
            ),
        );
        obj.insert(
            "time_bounds_us".to_string(),
            serde_json::Value::Array(
                time_bounds_us
                    .into_iter()
                    .map(|v| serde_json::Value::Number(serde_json::Number::from(v)))
                    .collect(),
            ),
        );
        let v = serde_json::Value::Object(obj);
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| format!("json: {e}"))?
        );
    }

    Ok(())
}

fn infer_manifest_kind(chunk: &formats::VectorChunk) -> String {
    use formats::VectorGeometry;

    let mut any_points = false;
    let mut any_lines = false;
    let mut any_areas = false;

    for f in &chunk.features {
        match &f.geometry {
            VectorGeometry::Point(_) | VectorGeometry::MultiPoint(_) => any_points = true,
            VectorGeometry::LineString(_) | VectorGeometry::MultiLineString(_) => any_lines = true,
            VectorGeometry::Polygon(_) | VectorGeometry::MultiPolygon(_) => any_areas = true,
        }
    }

    match (any_points, any_lines, any_areas) {
        (true, false, false) => "points".to_string(),
        (false, true, false) => "lines".to_string(),
        (false, false, true) => "areas".to_string(),
        _ => "vector".to_string(),
    }
}

fn compute_chunk_bounds_q_and_time_us(chunk: &formats::VectorChunk) -> ([i32; 4], [i64; 2]) {
    // Quantization must match AVc: 1e-6 degrees.
    fn quantize_deg(v: f64) -> i32 {
        let q = (v * 1_000_000.0).round();
        q.clamp(i32::MIN as f64, i32::MAX as f64) as i32
    }

    fn infer_time_span_micros(props: &serde_json::Map<String, serde_json::Value>) -> (i64, i64) {
        fn get_num(props: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<f64> {
            props.get(k).and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_f64(),
                serde_json::Value::String(s) => s.parse::<f64>().ok(),
                _ => None,
            })
        }

        if let Some(t) = get_num(props, "time").or_else(|| get_num(props, "timestamp")) {
            let us = (t * 1_000_000.0).round();
            let us = us.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
            return (us, us);
        }

        if let (Some(s), Some(e)) = (get_num(props, "start"), get_num(props, "end")) {
            let s_us = (s * 1_000_000.0).round();
            let e_us = (e * 1_000_000.0).round();
            let s_us = s_us.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
            let e_us = e_us.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
            return (s_us, e_us);
        }

        (i64::MIN, i64::MAX)
    }

    fn update_point_bounds(
        p: &formats::GeoPoint,
        min_lon: &mut i32,
        min_lat: &mut i32,
        max_lon: &mut i32,
        max_lat: &mut i32,
    ) {
        let lon_q = quantize_deg(p.lon_deg);
        let lat_q = quantize_deg(p.lat_deg);
        *min_lon = (*min_lon).min(lon_q);
        *min_lat = (*min_lat).min(lat_q);
        *max_lon = (*max_lon).max(lon_q);
        *max_lat = (*max_lat).max(lat_q);
    }

    let mut min_lon = i32::MAX;
    let mut min_lat = i32::MAX;
    let mut max_lon = i32::MIN;
    let mut max_lat = i32::MIN;
    let mut min_start_us = i64::MAX;
    let mut max_end_us = i64::MIN;

    for f in &chunk.features {
        let (s, e) = infer_time_span_micros(&f.properties);
        min_start_us = min_start_us.min(s);
        max_end_us = max_end_us.max(e);

        match &f.geometry {
            formats::VectorGeometry::Point(p) => {
                update_point_bounds(p, &mut min_lon, &mut min_lat, &mut max_lon, &mut max_lat)
            }
            formats::VectorGeometry::MultiPoint(ps) | formats::VectorGeometry::LineString(ps) => {
                for p in ps {
                    update_point_bounds(p, &mut min_lon, &mut min_lat, &mut max_lon, &mut max_lat);
                }
            }
            formats::VectorGeometry::MultiLineString(lines) => {
                for line in lines {
                    for p in line {
                        update_point_bounds(
                            p,
                            &mut min_lon,
                            &mut min_lat,
                            &mut max_lon,
                            &mut max_lat,
                        );
                    }
                }
            }
            formats::VectorGeometry::Polygon(rings) => {
                for ring in rings {
                    for p in ring {
                        update_point_bounds(
                            p,
                            &mut min_lon,
                            &mut min_lat,
                            &mut max_lon,
                            &mut max_lat,
                        );
                    }
                }
            }
            formats::VectorGeometry::MultiPolygon(polys) => {
                for poly in polys {
                    for ring in poly {
                        for p in ring {
                            update_point_bounds(
                                p,
                                &mut min_lon,
                                &mut min_lat,
                                &mut max_lon,
                                &mut max_lat,
                            );
                        }
                    }
                }
            }
        }
    }

    if min_lon == i32::MAX {
        min_lon = 0;
        min_lat = 0;
        max_lon = 0;
        max_lat = 0;
    }
    if min_start_us == i64::MAX {
        min_start_us = i64::MIN;
        max_end_us = i64::MAX;
    }

    (
        [min_lon, min_lat, max_lon, max_lat],
        [min_start_us, max_end_us],
    )
}

fn cmd_unpack(args: Vec<String>) -> Result<(), String> {
    // atlas unpack <input.avc> <output.geojson>
    if args.len() != 2 {
        return Err(usage());
    }

    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);

    let bytes = fs::read(&input).map_err(|e| format!("read {input:?}: {e}"))?;
    let chunk =
        formats::VectorChunk::from_avc_bytes(&bytes).map_err(|e| format!("decode avc: {e}"))?;

    let geojson = chunk
        .to_geojson_string_pretty()
        .map_err(|e| format!("encode geojson: {e}"))?;

    fs::write(&output, geojson).map_err(|e| format!("write {output:?}: {e}"))?;
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn usage() -> String {
    let exe = env::args().next().unwrap_or_else(|| "atlas".to_string());
    format!(
        "Usage:\n  {exe} pack <input.geojson> <output.avc> [--blob-dir DIR] [--print-chunk-entry]\n  {exe} unpack <input.avc> <output.geojson>\n\nNotes:\n- Uses lon/lat quantization (1e-6 degrees).\n- Semantic round-trip: unpacked GeoJSON preserves geometry + properties, but JSON ordering may differ.\n- Blob storage is only active when --blob-dir is provided (stores original source bytes by content hash).\n"
    )
}
