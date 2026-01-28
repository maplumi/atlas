use crate::vector_chunk::{GeoPoint, VectorChunk, VectorFeature, VectorGeometry};
use serde_json::{Map, Value};

const MAGIC: [u8; 4] = *b"ATVC";
const VERSION_V1: u16 = 1;
const VERSION_V2: u16 = 2;
const VERSION_LATEST: u16 = VERSION_V2;

// Quantization scale: 1e6 => ~0.11m at equator.
const DEG_Q: f64 = 1_000_000.0;

#[derive(Debug)]
pub enum AvcError {
    UnexpectedEof,
    InvalidMagic,
    UnsupportedVersion { found: u16 },
    InvalidVarint,
    InvalidUtf8,
    InvalidJson,
    InvalidGeometry { reason: String },
}

impl std::fmt::Display for AvcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvcError::UnexpectedEof => write!(f, "unexpected EOF"),
            AvcError::InvalidMagic => write!(f, "invalid ATVC magic"),
            AvcError::UnsupportedVersion { found } => {
                write!(f, "unsupported ATVC version: {found}")
            }
            AvcError::InvalidVarint => write!(f, "invalid varint"),
            AvcError::InvalidUtf8 => write!(f, "invalid utf-8"),
            AvcError::InvalidJson => write!(f, "invalid JSON"),
            AvcError::InvalidGeometry { reason } => write!(f, "invalid geometry: {reason}"),
        }
    }
}

impl std::error::Error for AvcError {}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
enum GeomTag {
    Point = 1,
    MultiPoint = 2,
    LineString = 3,
    MultiLineString = 4,
    Polygon = 5,
    MultiPolygon = 6,
}

pub fn encode_avc(chunk: &VectorChunk) -> Result<Vec<u8>, AvcError> {
    let mut out: Vec<u8> = Vec::new();

    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION_LATEST.to_le_bytes());

    // flags (reserved)
    out.extend_from_slice(&0u16.to_le_bytes());

    out.extend_from_slice(&(chunk.features.len() as u32).to_le_bytes());

    // v2 baked metadata (for fast pruning / indexing)
    // lon/lat quantized bounds: [min_lon_q, max_lon_q, min_lat_q, max_lat_q]
    // time bounds in microseconds: [min_start_us, max_end_us]
    let mut min_lon_q = i32::MAX;
    let mut max_lon_q = i32::MIN;
    let mut min_lat_q = i32::MAX;
    let mut max_lat_q = i32::MIN;
    let mut min_start_us = i64::MAX;
    let mut max_end_us = i64::MIN;

    for feat in &chunk.features {
        let (start_us, end_us) = infer_time_span_micros(&feat.properties);
        min_start_us = min_start_us.min(start_us);
        max_end_us = max_end_us.max(end_us);

        update_bounds_for_geometry(
            &feat.geometry,
            &mut min_lon_q,
            &mut max_lon_q,
            &mut min_lat_q,
            &mut max_lat_q,
        );
    }

    if min_lon_q == i32::MAX {
        // empty chunk (should be rare, but keep encoding stable)
        min_lon_q = 0;
        max_lon_q = 0;
        min_lat_q = 0;
        max_lat_q = 0;
    }
    if min_start_us == i64::MAX {
        min_start_us = i64::MIN;
        max_end_us = i64::MAX;
    }

    out.extend_from_slice(&min_lon_q.to_le_bytes());
    out.extend_from_slice(&max_lon_q.to_le_bytes());
    out.extend_from_slice(&min_lat_q.to_le_bytes());
    out.extend_from_slice(&max_lat_q.to_le_bytes());
    out.extend_from_slice(&min_start_us.to_le_bytes());
    out.extend_from_slice(&max_end_us.to_le_bytes());

    for feat in &chunk.features {
        let (tag, geom_bytes) = encode_geometry(&feat.geometry)?;
        out.push(tag as u8);

        // id
        match &feat.id {
            Some(s) => {
                write_var_u64(&mut out, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            None => {
                write_var_u64(&mut out, 0);
            }
        }

        // time span micros (inferred, but properties preserved as-is)
        let (start_us, end_us) = infer_time_span_micros(&feat.properties);
        out.extend_from_slice(&start_us.to_le_bytes());
        out.extend_from_slice(&end_us.to_le_bytes());

        // properties JSON bytes (semantic round-trip)
        let props_bytes =
            serde_json::to_vec(&feat.properties).map_err(|_| AvcError::InvalidJson)?;
        write_var_u64(&mut out, props_bytes.len() as u64);
        out.extend_from_slice(&props_bytes);

        // geometry payload
        write_var_u64(&mut out, geom_bytes.len() as u64);
        out.extend_from_slice(&geom_bytes);
    }

    Ok(out)
}

pub fn decode_avc(bytes: &[u8]) -> Result<VectorChunk, AvcError> {
    let mut r = Reader::new(bytes);

    let magic = r.read_exact(4)?;
    if magic.as_slice() != MAGIC.as_slice() {
        return Err(AvcError::InvalidMagic);
    }

    let version = u16::from_le_bytes(r.read_exact(2)?.try_into().unwrap());
    if version != VERSION_V1 && version != VERSION_V2 {
        return Err(AvcError::UnsupportedVersion { found: version });
    }

    let _flags = u16::from_le_bytes(r.read_exact(2)?.try_into().unwrap());
    let feature_count = u32::from_le_bytes(r.read_exact(4)?.try_into().unwrap()) as usize;

    if version == VERSION_V2 {
        // baked metadata (currently ignored by the loader)
        let _min_lon_q = i32::from_le_bytes(r.read_exact(4)?.try_into().unwrap());
        let _max_lon_q = i32::from_le_bytes(r.read_exact(4)?.try_into().unwrap());
        let _min_lat_q = i32::from_le_bytes(r.read_exact(4)?.try_into().unwrap());
        let _max_lat_q = i32::from_le_bytes(r.read_exact(4)?.try_into().unwrap());
        let _min_start_us = i64::from_le_bytes(r.read_exact(8)?.try_into().unwrap());
        let _max_end_us = i64::from_le_bytes(r.read_exact(8)?.try_into().unwrap());
    }

    let mut features: Vec<VectorFeature> = Vec::with_capacity(feature_count);
    for _ in 0..feature_count {
        let tag = r.read_u8()?;

        let id_len = r.read_var_u64()? as usize;
        let id = if id_len == 0 {
            None
        } else {
            let b = r.read_exact(id_len)?;
            let s = std::str::from_utf8(&b).map_err(|_| AvcError::InvalidUtf8)?;
            Some(s.to_string())
        };

        let start_us = i64::from_le_bytes(r.read_exact(8)?.try_into().unwrap());
        let end_us = i64::from_le_bytes(r.read_exact(8)?.try_into().unwrap());
        let _time = (start_us, end_us);

        let props_len = r.read_var_u64()? as usize;
        let props_bytes = r.read_exact(props_len)?;
        let props_val: Value =
            serde_json::from_slice(&props_bytes).map_err(|_| AvcError::InvalidJson)?;
        let properties: Map<String, Value> = props_val.as_object().cloned().unwrap_or_default();

        let geom_len = r.read_var_u64()? as usize;
        let geom_bytes = r.read_exact(geom_len)?;
        let geometry = decode_geometry(tag, &geom_bytes)?;

        features.push(VectorFeature {
            id,
            properties,
            geometry,
        });
    }

    Ok(VectorChunk { features })
}

fn update_bounds_for_geometry(
    geometry: &VectorGeometry,
    min_lon_q: &mut i32,
    max_lon_q: &mut i32,
    min_lat_q: &mut i32,
    max_lat_q: &mut i32,
) {
    fn visit_point(
        p: &GeoPoint,
        min_lon_q: &mut i32,
        max_lon_q: &mut i32,
        min_lat_q: &mut i32,
        max_lat_q: &mut i32,
    ) {
        let lon_q = quantize_deg(p.lon_deg);
        let lat_q = quantize_deg(p.lat_deg);
        *min_lon_q = (*min_lon_q).min(lon_q);
        *max_lon_q = (*max_lon_q).max(lon_q);
        *min_lat_q = (*min_lat_q).min(lat_q);
        *max_lat_q = (*max_lat_q).max(lat_q);
    }

    match geometry {
        VectorGeometry::Point(p) => visit_point(p, min_lon_q, max_lon_q, min_lat_q, max_lat_q),
        VectorGeometry::MultiPoint(ps) | VectorGeometry::LineString(ps) => {
            for p in ps {
                visit_point(p, min_lon_q, max_lon_q, min_lat_q, max_lat_q);
            }
        }
        VectorGeometry::MultiLineString(lines) => {
            for line in lines {
                for p in line {
                    visit_point(p, min_lon_q, max_lon_q, min_lat_q, max_lat_q);
                }
            }
        }
        VectorGeometry::Polygon(rings) => {
            for ring in rings {
                for p in ring {
                    visit_point(p, min_lon_q, max_lon_q, min_lat_q, max_lat_q);
                }
            }
        }
        VectorGeometry::MultiPolygon(polys) => {
            for poly in polys {
                for ring in poly {
                    for p in ring {
                        visit_point(p, min_lon_q, max_lon_q, min_lat_q, max_lat_q);
                    }
                }
            }
        }
    }
}

fn encode_geometry(geom: &VectorGeometry) -> Result<(GeomTag, Vec<u8>), AvcError> {
    let mut out: Vec<u8> = Vec::new();
    match geom {
        VectorGeometry::Point(p) => {
            write_i32(&mut out, quantize_deg(p.lon_deg));
            write_i32(&mut out, quantize_deg(p.lat_deg));
            Ok((GeomTag::Point, out))
        }
        VectorGeometry::MultiPoint(ps) => {
            write_var_u64(&mut out, ps.len() as u64);
            for p in ps {
                write_i32(&mut out, quantize_deg(p.lon_deg));
                write_i32(&mut out, quantize_deg(p.lat_deg));
            }
            Ok((GeomTag::MultiPoint, out))
        }
        VectorGeometry::LineString(ps) => {
            write_var_u64(&mut out, ps.len() as u64);
            for p in ps {
                write_i32(&mut out, quantize_deg(p.lon_deg));
                write_i32(&mut out, quantize_deg(p.lat_deg));
            }
            Ok((GeomTag::LineString, out))
        }
        VectorGeometry::MultiLineString(lines) => {
            write_var_u64(&mut out, lines.len() as u64);
            for line in lines {
                write_var_u64(&mut out, line.len() as u64);
                for p in line {
                    write_i32(&mut out, quantize_deg(p.lon_deg));
                    write_i32(&mut out, quantize_deg(p.lat_deg));
                }
            }
            Ok((GeomTag::MultiLineString, out))
        }
        VectorGeometry::Polygon(rings) => {
            write_var_u64(&mut out, rings.len() as u64);
            for ring in rings {
                write_var_u64(&mut out, ring.len() as u64);
                for p in ring {
                    write_i32(&mut out, quantize_deg(p.lon_deg));
                    write_i32(&mut out, quantize_deg(p.lat_deg));
                }
            }
            Ok((GeomTag::Polygon, out))
        }
        VectorGeometry::MultiPolygon(polys) => {
            write_var_u64(&mut out, polys.len() as u64);
            for poly in polys {
                write_var_u64(&mut out, poly.len() as u64);
                for ring in poly {
                    write_var_u64(&mut out, ring.len() as u64);
                    for p in ring {
                        write_i32(&mut out, quantize_deg(p.lon_deg));
                        write_i32(&mut out, quantize_deg(p.lat_deg));
                    }
                }
            }
            Ok((GeomTag::MultiPolygon, out))
        }
    }
}

fn decode_geometry(tag: u8, bytes: &[u8]) -> Result<VectorGeometry, AvcError> {
    let mut r = Reader::new(bytes);
    match tag {
        x if x == GeomTag::Point as u8 => {
            let lon = dequantize_deg(r.read_i32()?);
            let lat = dequantize_deg(r.read_i32()?);
            Ok(VectorGeometry::Point(GeoPoint::new(lon, lat)))
        }
        x if x == GeomTag::MultiPoint as u8 => {
            let n = r.read_var_u64()? as usize;
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = dequantize_deg(r.read_i32()?);
                let lat = dequantize_deg(r.read_i32()?);
                out.push(GeoPoint::new(lon, lat));
            }
            Ok(VectorGeometry::MultiPoint(out))
        }
        x if x == GeomTag::LineString as u8 => {
            let n = r.read_var_u64()? as usize;
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = dequantize_deg(r.read_i32()?);
                let lat = dequantize_deg(r.read_i32()?);
                out.push(GeoPoint::new(lon, lat));
            }
            Ok(VectorGeometry::LineString(out))
        }
        x if x == GeomTag::MultiLineString as u8 => {
            let nlines = r.read_var_u64()? as usize;
            let mut out: Vec<Vec<GeoPoint>> = Vec::with_capacity(nlines);
            for _ in 0..nlines {
                let n = r.read_var_u64()? as usize;
                let mut line = Vec::with_capacity(n);
                for _ in 0..n {
                    let lon = dequantize_deg(r.read_i32()?);
                    let lat = dequantize_deg(r.read_i32()?);
                    line.push(GeoPoint::new(lon, lat));
                }
                out.push(line);
            }
            Ok(VectorGeometry::MultiLineString(out))
        }
        x if x == GeomTag::Polygon as u8 => {
            let nrings = r.read_var_u64()? as usize;
            let mut rings: Vec<Vec<GeoPoint>> = Vec::with_capacity(nrings);
            for _ in 0..nrings {
                let n = r.read_var_u64()? as usize;
                let mut ring = Vec::with_capacity(n);
                for _ in 0..n {
                    let lon = dequantize_deg(r.read_i32()?);
                    let lat = dequantize_deg(r.read_i32()?);
                    ring.push(GeoPoint::new(lon, lat));
                }
                rings.push(ring);
            }
            Ok(VectorGeometry::Polygon(rings))
        }
        x if x == GeomTag::MultiPolygon as u8 => {
            let npolys = r.read_var_u64()? as usize;
            let mut polys: Vec<Vec<Vec<GeoPoint>>> = Vec::with_capacity(npolys);
            for _ in 0..npolys {
                let nrings = r.read_var_u64()? as usize;
                let mut rings: Vec<Vec<GeoPoint>> = Vec::with_capacity(nrings);
                for _ in 0..nrings {
                    let n = r.read_var_u64()? as usize;
                    let mut ring = Vec::with_capacity(n);
                    for _ in 0..n {
                        let lon = dequantize_deg(r.read_i32()?);
                        let lat = dequantize_deg(r.read_i32()?);
                        ring.push(GeoPoint::new(lon, lat));
                    }
                    rings.push(ring);
                }
                polys.push(rings);
            }
            Ok(VectorGeometry::MultiPolygon(polys))
        }
        _ => Err(AvcError::InvalidGeometry {
            reason: format!("unknown geometry tag: {tag}"),
        }),
    }
}

fn infer_time_span_micros(props: &Map<String, Value>) -> (i64, i64) {
    // Same convention as ingest:
    // - numeric/string "time" or "timestamp" => seconds => instant
    // - numeric/string "start" and "end" => seconds => range
    // - else => forever
    fn get_num(props: &Map<String, Value>, k: &str) -> Option<f64> {
        props.get(k).and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
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

fn quantize_deg(v: f64) -> i32 {
    let q = (v * DEG_Q).round();
    q.clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn dequantize_deg(q: i32) -> f64 {
    (q as f64) / DEG_Q
}

fn write_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_var_u64(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(((v as u8) & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, AvcError> {
        if self.pos >= self.bytes.len() {
            return Err(AvcError::UnexpectedEof);
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, AvcError> {
        if self.pos + n > self.bytes.len() {
            return Err(AvcError::UnexpectedEof);
        }
        let out = self.bytes[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(out)
    }

    fn read_i32(&mut self) -> Result<i32, AvcError> {
        let b = self.read_exact(4)?;
        Ok(i32::from_le_bytes(b.try_into().unwrap()))
    }

    fn read_var_u64(&mut self) -> Result<u64, AvcError> {
        let mut out: u64 = 0;
        let mut shift = 0;
        for _ in 0..10 {
            let b = self.read_u8()?;
            out |= ((b & 0x7F) as u64) << shift;
            if (b & 0x80) == 0 {
                return Ok(out);
            }
            shift += 7;
        }
        Err(AvcError::InvalidVarint)
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_avc, encode_avc};
    use crate::VectorChunk;

    fn assert_close(a: f64, b: f64, eps: f64) {
        let d = (a - b).abs();
        assert!(d <= eps, "expected {a} ~= {b} (diff {d})");
    }

    #[test]
    fn avc_round_trip_demo_points_quantized() {
        let payload = include_str!("../../apps/viewer_web/assets/chunks/cities.json");
        let chunk = VectorChunk::from_geojson_str(payload).expect("parse");

        let bytes = encode_avc(&chunk).expect("encode");
        let rt = decode_avc(&bytes).expect("decode");

        assert_eq!(rt.features.len(), chunk.features.len());

        for (a, b) in rt.features.iter().zip(chunk.features.iter()) {
            // properties should be semantically identical (ordering irrelevant)
            assert_eq!(a.properties, b.properties);
            if let (crate::VectorGeometry::Point(pa), crate::VectorGeometry::Point(pb)) =
                (&a.geometry, &b.geometry)
            {
                assert_close(pa.lon_deg, pb.lon_deg, 1e-6);
                assert_close(pa.lat_deg, pb.lat_deg, 1e-6);
            }
        }
    }
}
