use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use foundation::math::{Ecef, Vec3, ecef_to_geodetic};
use layers::vector::VectorLayer;
use scene::components::VectorGeometryKind;
use serde::Serialize;

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
        "manifest" => cmd_manifest(args),
        "unpack" => cmd_unpack(args),
        "surface-tiles" => cmd_surface_tiles(args),
        _ => Err(usage()),
    }
}

fn cmd_manifest(args: Vec<String>) -> Result<(), String> {
    // atlas manifest <output_dir> <chunk.avc> [chunk2.avc ...] [--name NAME]
    if args.len() < 2 {
        return Err(usage());
    }

    let out_dir = PathBuf::from(&args[0]);
    let mut name: Option<String> = None;
    let mut chunk_paths: Vec<PathBuf> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                i += 1;
                if i >= args.len() {
                    return Err("--name requires a value".to_string());
                }
                name = Some(args[i].clone());
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown arg: {s}\n\n{}", usage()));
            }
            _ => {
                chunk_paths.push(PathBuf::from(&args[i]));
            }
        }
        i += 1;
    }

    if chunk_paths.is_empty() {
        return Err("manifest requires at least one chunk path".to_string());
    }

    fs::create_dir_all(&out_dir).map_err(|e| format!("create {out_dir:?}: {e}"))?;

    let mut manifest = formats::SceneManifest::new("placeholder");
    manifest.name = name;

    for p in chunk_paths {
        let bytes = fs::read(&p).map_err(|e| format!("read {p:?}: {e}"))?;
        let chunk_hash = blake3::hash(&bytes);
        let chunk_hash_hex = to_hex(chunk_hash.as_bytes());

        let chunk =
            formats::VectorChunk::from_avc_bytes(&bytes).map_err(|e| format!("decode avc: {e}"))?;
        let kind = infer_manifest_kind(&chunk);
        let (lon_lat_bounds_q, time_bounds_us) = compute_chunk_bounds_q_and_time_us(&chunk);
        let feature_count = chunk.features.len() as u32;

        let file_name = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("invalid chunk filename: {p:?}"))?
            .to_string();
        let id = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("chunk")
            .to_string();

        let out_path = out_dir.join(&file_name);
        if out_path.exists() {
            return Err(format!("output chunk already exists: {out_path:?}"));
        }
        fs::write(&out_path, &bytes).map_err(|e| format!("write {out_path:?}: {e}"))?;

        manifest.chunks.push(formats::ChunkEntry {
            id,
            kind,
            path: file_name,
            content_hash: Some(chunk_hash_hex),
            source_blob_hash: None,
            lon_lat_bounds_q: Some(lon_lat_bounds_q),
            time_bounds_us: Some(time_bounds_us),
            feature_count: Some(feature_count),
        });
    }

    manifest.compute_and_set_identity();

    let manifest_path = out_dir.join(formats::scene_package::MANIFEST_FILE_NAME);
    let payload = serde_json::to_string_pretty(&manifest).map_err(|e| format!("json: {e}"))?;
    fs::write(&manifest_path, payload).map_err(|e| format!("write {manifest_path:?}: {e}"))?;

    eprintln!(
        "wrote {} (package_id={}, content_hash={})",
        manifest_path.display(),
        manifest.package_id,
        manifest.content_hash.clone().unwrap_or_default()
    );
    Ok(())
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

    let file = fs::File::create(&output).map_err(|e| format!("create {output:?}: {e}"))?;
    let mut writer = HashingWriter::new(file);
    chunk
        .to_avc_writer(&mut writer)
        .map_err(|e| format!("encode avc: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("flush {output:?}: {e}"))?;

    let chunk_hash_hex = writer.finalize_hex();

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

struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    fn finalize_hex(&self) -> String {
        to_hex(self.hasher.clone().finalize().as_bytes())
    }
}

impl<W: std::io::Write> std::io::Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
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

#[derive(Debug, Serialize)]
struct SurfaceTileset {
    version: u32,
    zoom_min: u32,
    zoom_max: u32,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    data_type: String,
    coordinate_space: String,
    tile_path_template: String,
}

fn cmd_surface_tiles(args: Vec<String>) -> Result<(), String> {
    // atlas surface-tiles <input.geojson> <output_dir> [--zoom-min N] [--zoom-max N]
    if args.len() < 2 {
        return Err(usage());
    }

    let input = PathBuf::from(&args[0]);
    let out_dir = PathBuf::from(&args[1]);

    let mut zoom_min: u32 = 0;
    let mut zoom_max: u32 = 4;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--zoom-min" => {
                i += 1;
                if i >= args.len() {
                    return Err("--zoom-min requires a value".to_string());
                }
                zoom_min = args[i]
                    .parse::<u32>()
                    .map_err(|_| "--zoom-min must be an integer".to_string())?;
            }
            "--zoom-max" => {
                i += 1;
                if i >= args.len() {
                    return Err("--zoom-max requires a value".to_string());
                }
                zoom_max = args[i]
                    .parse::<u32>()
                    .map_err(|_| "--zoom-max must be an integer".to_string())?;
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown arg: {s}\n\n{}", usage()));
            }
            _ => {
                return Err(format!("unexpected arg: {}\n\n{}", args[i], usage()));
            }
        }
        i += 1;
    }

    if zoom_min > zoom_max {
        return Err("zoom-min must be <= zoom-max".to_string());
    }

    let text = fs::read_to_string(&input).map_err(|e| format!("read {input:?}: {e}"))?;
    let chunk = formats::VectorChunk::from_geojson_str(&text)
        .map_err(|e| format!("decode geojson: {e}"))?;
    let chunk = unwrap_antimeridian_chunk(&chunk);

    let mut world = scene::World::new();
    scene::prefabs::spawn_wgs84_globe(&mut world);
    formats::ingest_vector_chunk(&mut world, &chunk, Some(VectorGeometryKind::Area));

    let layer = VectorLayer::new(1);
    let snap = layer.extract(&world);
    let triangles = snap.area_triangles;

    if triangles.is_empty() {
        return Err("no polygons found in input".to_string());
    }

    fs::create_dir_all(&out_dir).map_err(|e| format!("create {out_dir:?}: {e}"))?;

    let tileset = SurfaceTileset {
        version: 1,
        zoom_min,
        zoom_max,
        min_lon: -180.0,
        max_lon: 180.0,
        min_lat: -90.0,
        max_lat: 90.0,
        data_type: "f32-xyz".to_string(),
        coordinate_space: "viewer".to_string(),
        tile_path_template: "tiles/{z}/{x}/{y}.bin".to_string(),
    };

    let mut tiles: HashMap<(u32, u32, u32), Vec<[f32; 3]>> = HashMap::new();

    for tri in triangles.chunks_exact(3) {
        let a = tri[0];
        let b = tri[1];
        let c = tri[2];

        let (lon_a, lat_a) = lon_lat_from_ecef(a);
        let (lon_b, lat_b) = lon_lat_from_ecef(b);
        let (lon_c, lat_c) = lon_lat_from_ecef(c);

        let mut lat_min = lat_a.min(lat_b.min(lat_c));
        let mut lat_max = lat_a.max(lat_b.max(lat_c));
        lat_min = lat_min.clamp(tileset.min_lat, tileset.max_lat);
        lat_max = lat_max.clamp(tileset.min_lat, tileset.max_lat);

        let lon_ranges = antimeridian_lon_ranges([lon_a, lon_b, lon_c]);

        let tri_view = [
            ecef_vec3_to_viewer_f32(a),
            ecef_vec3_to_viewer_f32(b),
            ecef_vec3_to_viewer_f32(c),
        ];

        for z in zoom_min..=zoom_max {
            let n = 2u32.pow(z);
            let (y_min, y_max) = lat_range_to_tile_range(lat_min, lat_max, n, &tileset);

            for (lon_min, lon_max) in &lon_ranges {
                let (x_min, x_max) = lon_range_to_tile_range(*lon_min, *lon_max, n, &tileset);
                for y in y_min..=y_max {
                    for x in x_min..=x_max {
                        tiles.entry((z, x, y)).or_default().extend(tri_view);
                    }
                }
            }
        }
    }

    for ((z, x, y), verts) in tiles {
        if verts.is_empty() {
            continue;
        }

        let tile_path = out_dir
            .join("tiles")
            .join(z.to_string())
            .join(x.to_string())
            .join(format!("{y}.bin"));
        if let Some(parent) = tile_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
        }

        let mut bytes: Vec<u8> = Vec::with_capacity(verts.len() * 3 * 4);
        for v in verts {
            bytes.extend_from_slice(&v[0].to_le_bytes());
            bytes.extend_from_slice(&v[1].to_le_bytes());
            bytes.extend_from_slice(&v[2].to_le_bytes());
        }
        fs::write(&tile_path, bytes).map_err(|e| format!("write {tile_path:?}: {e}"))?;
    }

    let tileset_path = out_dir.join("tileset.json");
    let payload = serde_json::to_string_pretty(&tileset).map_err(|e| format!("json: {e}"))?;
    fs::write(&tileset_path, payload).map_err(|e| format!("write {tileset_path:?}: {e}"))?;

    eprintln!("wrote {}", tileset_path.display());
    Ok(())
}

fn ecef_vec3_to_viewer_f32(p: Vec3) -> [f32; 3] {
    [p.x as f32, p.z as f32, (-p.y) as f32]
}

fn lon_lat_from_ecef(p: Vec3) -> (f64, f64) {
    let geo = ecef_to_geodetic(Ecef::new(p.x, p.y, p.z));
    (geo.lon_rad.to_degrees(), geo.lat_rad.to_degrees())
}

fn antimeridian_lon_ranges(lons: [f64; 3]) -> Vec<(f64, f64)> {
    let min_lon = lons[0].min(lons[1].min(lons[2]));
    let max_lon = lons[0].max(lons[1].max(lons[2]));
    let span = max_lon - min_lon;
    if span <= 180.0 {
        return vec![(min_lon, max_lon)];
    }

    let mut ranges = Vec::new();
    let neg: Vec<f64> = lons.iter().copied().filter(|lon| *lon < 0.0).collect();
    let pos: Vec<f64> = lons.iter().copied().filter(|lon| *lon >= 0.0).collect();
    if !neg.is_empty() {
        let min_neg = neg.iter().copied().fold(f64::INFINITY, f64::min);
        ranges.push((min_neg, 180.0));
    }
    if !pos.is_empty() {
        let max_pos = pos.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        ranges.push((-180.0, max_pos));
    }

    if ranges.is_empty() {
        vec![(min_lon, max_lon)]
    } else {
        ranges
    }
}

fn lon_range_to_tile_range(
    lon_min: f64,
    lon_max: f64,
    n: u32,
    tileset: &SurfaceTileset,
) -> (u32, u32) {
    let span = tileset.max_lon - tileset.min_lon;
    let t_min = ((lon_min - tileset.min_lon) / span).clamp(0.0, 1.0 - 1e-9);
    let t_max = ((lon_max - tileset.min_lon) / span).clamp(0.0, 1.0 - 1e-9);
    let x_min = (t_min * n as f64).floor() as u32;
    let x_max = (t_max * n as f64).floor() as u32;
    (x_min.min(n - 1), x_max.min(n - 1))
}

fn lat_range_to_tile_range(
    lat_min: f64,
    lat_max: f64,
    n: u32,
    tileset: &SurfaceTileset,
) -> (u32, u32) {
    let span = tileset.max_lat - tileset.min_lat;
    let t_min = ((tileset.max_lat - lat_max) / span).clamp(0.0, 1.0 - 1e-9);
    let t_max = ((tileset.max_lat - lat_min) / span).clamp(0.0, 1.0 - 1e-9);
    let y_min = (t_min * n as f64).floor() as u32;
    let y_max = (t_max * n as f64).floor() as u32;
    (y_min.min(n - 1), y_max.min(n - 1))
}

fn unwrap_antimeridian_chunk(chunk: &formats::VectorChunk) -> formats::VectorChunk {
    use formats::{VectorChunk, VectorFeature, VectorGeometry};

    let features = chunk
        .features
        .iter()
        .map(|feat| {
            let geometry = match &feat.geometry {
                VectorGeometry::Polygon(rings) => {
                    VectorGeometry::Polygon(rings.iter().map(|ring| unwrap_ring(ring)).collect())
                }
                VectorGeometry::MultiPolygon(polys) => VectorGeometry::MultiPolygon(
                    polys
                        .iter()
                        .map(|poly| poly.iter().map(|ring| unwrap_ring(ring)).collect())
                        .collect(),
                ),
                VectorGeometry::LineString(points) => {
                    VectorGeometry::LineString(unwrap_ring(points))
                }
                VectorGeometry::MultiLineString(lines) => VectorGeometry::MultiLineString(
                    lines.iter().map(|line| unwrap_ring(line)).collect(),
                ),
                other => other.clone(),
            };

            VectorFeature {
                id: feat.id.clone(),
                properties: feat.properties.clone(),
                geometry,
            }
        })
        .collect();

    VectorChunk { features }
}

fn unwrap_ring(points: &[formats::GeoPoint]) -> Vec<formats::GeoPoint> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<formats::GeoPoint> = Vec::with_capacity(points.len());
    let mut prev_lon = points[0].lon_deg;
    out.push(formats::GeoPoint::new(prev_lon, points[0].lat_deg));

    for p in points.iter().skip(1) {
        let mut lon = p.lon_deg;
        let mut delta = lon - prev_lon;
        if delta > 180.0 {
            lon -= 360.0;
        } else if delta < -180.0 {
            lon += 360.0;
        }
        delta = lon - prev_lon;
        if delta > 180.0 {
            lon -= 360.0;
        } else if delta < -180.0 {
            lon += 360.0;
        }
        prev_lon = lon;
        out.push(formats::GeoPoint::new(lon, p.lat_deg));
    }

    out
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
        "Usage:\n  {exe} pack <input.geojson> <output.avc> [--blob-dir DIR] [--print-chunk-entry]\n  {exe} manifest <output_dir> <chunk.avc> [chunk2.avc ...] [--name NAME]\n  {exe} unpack <input.avc> <output.geojson>\n  {exe} surface-tiles <input.geojson> <output_dir> [--zoom-min N] [--zoom-max N]\n\nNotes:\n- Uses lon/lat quantization (1e-6 degrees).\n- Semantic round-trip: unpacked GeoJSON preserves geometry + properties, but JSON ordering may differ.\n- Blob storage is only active when --blob-dir is provided (stores original source bytes by content hash).\n- `manifest` writes a self-contained scene package directory with `scene.manifest.json`.\n"
    )
}
