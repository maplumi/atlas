use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct GeoPoint {
    pub lon_deg: f64,
    pub lat_deg: f64,
}

impl GeoPoint {
    pub fn new(lon_deg: f64, lat_deg: f64) -> Self {
        Self { lon_deg, lat_deg }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VectorGeometry {
    Point(GeoPoint),
    MultiPoint(Vec<GeoPoint>),
    LineString(Vec<GeoPoint>),
    MultiLineString(Vec<Vec<GeoPoint>>),
    Polygon(Vec<Vec<GeoPoint>>),
    MultiPolygon(Vec<Vec<Vec<GeoPoint>>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorFeature {
    pub id: Option<String>,
    pub properties: Map<String, Value>,
    pub geometry: VectorGeometry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorChunk {
    pub features: Vec<VectorFeature>,
}

#[derive(Debug)]
pub enum VectorChunkError {
    NotAFeatureCollection,
    InvalidFeature { index: usize, reason: String },
}

impl std::fmt::Display for VectorChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorChunkError::NotAFeatureCollection => {
                write!(f, "expected GeoJSON FeatureCollection")
            }
            VectorChunkError::InvalidFeature { index, reason } => {
                write!(f, "invalid feature at index {index}: {reason}")
            }
        }
    }
}

impl std::error::Error for VectorChunkError {}

impl VectorChunk {
    pub fn from_geojson_str(payload: &str) -> Result<Self, VectorChunkError> {
        let value: Value =
            serde_json::from_str(payload).map_err(|e| VectorChunkError::InvalidFeature {
                index: 0,
                reason: format!("JSON parse error: {e}"),
            })?;
        Self::from_geojson_value(value)
    }

    pub fn from_geojson_value(value: Value) -> Result<Self, VectorChunkError> {
        let obj = value
            .as_object()
            .ok_or(VectorChunkError::NotAFeatureCollection)?;
        let ty = obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or(VectorChunkError::NotAFeatureCollection)?;
        if ty != "FeatureCollection" {
            return Err(VectorChunkError::NotAFeatureCollection);
        }

        let features_val = obj
            .get("features")
            .and_then(|v| v.as_array())
            .ok_or(VectorChunkError::NotAFeatureCollection)?;

        let mut features = Vec::with_capacity(features_val.len());
        for (index, feat_val) in features_val.iter().enumerate() {
            let feat_obj = feat_val
                .as_object()
                .ok_or(VectorChunkError::InvalidFeature {
                    index,
                    reason: "feature must be an object".to_string(),
                })?;

            let feat_type = feat_obj.get("type").and_then(|v| v.as_str()).ok_or(
                VectorChunkError::InvalidFeature {
                    index,
                    reason: "feature missing type".to_string(),
                },
            )?;
            if feat_type != "Feature" {
                return Err(VectorChunkError::InvalidFeature {
                    index,
                    reason: format!("unexpected feature type: {feat_type}"),
                });
            }

            let id = match feat_obj.get("id") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            };

            let properties = feat_obj
                .get("properties")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let geometry_val =
                feat_obj
                    .get("geometry")
                    .ok_or(VectorChunkError::InvalidFeature {
                        index,
                        reason: "feature missing geometry".to_string(),
                    })?;
            let geometry = parse_geometry(geometry_val)
                .map_err(|reason| VectorChunkError::InvalidFeature { index, reason })?;

            features.push(VectorFeature {
                id,
                properties,
                geometry,
            });
        }

        Ok(Self { features })
    }
}

fn parse_geometry(value: &Value) -> Result<VectorGeometry, String> {
    let obj = value
        .as_object()
        .ok_or("geometry must be an object".to_string())?;
    let ty = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("geometry missing type".to_string())?;

    let coords = obj
        .get("coordinates")
        .ok_or("geometry missing coordinates".to_string())?;

    match ty {
        "Point" => Ok(VectorGeometry::Point(parse_point(coords)?)),
        "MultiPoint" => Ok(VectorGeometry::MultiPoint(parse_points(coords)?)),
        "LineString" => Ok(VectorGeometry::LineString(parse_points(coords)?)),
        "MultiLineString" => Ok(VectorGeometry::MultiLineString(parse_lines(coords)?)),
        "Polygon" => Ok(VectorGeometry::Polygon(parse_polygon(coords)?)),
        "MultiPolygon" => Ok(VectorGeometry::MultiPolygon(parse_multi_polygon(coords)?)),
        other => Err(format!("unsupported geometry type: {other}")),
    }
}

fn parse_point(coords: &Value) -> Result<GeoPoint, String> {
    let arr = coords
        .as_array()
        .ok_or("Point coordinates must be an array".to_string())?;
    if arr.len() < 2 {
        return Err("Point coordinates must have [lon, lat]".to_string());
    }
    let lon = arr[0]
        .as_f64()
        .ok_or("Point lon must be a number".to_string())?;
    let lat = arr[1]
        .as_f64()
        .ok_or("Point lat must be a number".to_string())?;
    Ok(GeoPoint::new(lon, lat))
}

fn parse_points(coords: &Value) -> Result<Vec<GeoPoint>, String> {
    let arr = coords
        .as_array()
        .ok_or("coordinates must be an array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(parse_point(item)?);
    }
    Ok(out)
}

fn parse_lines(coords: &Value) -> Result<Vec<Vec<GeoPoint>>, String> {
    let arr = coords
        .as_array()
        .ok_or("MultiLineString coordinates must be an array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for line in arr {
        out.push(parse_points(line)?);
    }
    Ok(out)
}

fn parse_polygon(coords: &Value) -> Result<Vec<Vec<GeoPoint>>, String> {
    let rings = coords
        .as_array()
        .ok_or("Polygon coordinates must be an array of rings".to_string())?;
    let mut out = Vec::with_capacity(rings.len());
    for ring in rings {
        out.push(parse_points(ring)?);
    }
    Ok(out)
}

fn parse_multi_polygon(coords: &Value) -> Result<Vec<Vec<Vec<GeoPoint>>>, String> {
    let polys = coords
        .as_array()
        .ok_or("MultiPolygon coordinates must be an array of polygons".to_string())?;
    let mut out = Vec::with_capacity(polys.len());
    for poly in polys {
        out.push(parse_polygon(poly)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{VectorChunk, VectorGeometry};

    #[test]
    fn parses_demo_cities_points() {
        let payload = include_str!("../../apps/viewer_web/assets/chunks/cities.json");
        let chunk = VectorChunk::from_geojson_str(payload).expect("parse VectorChunk");
        assert_eq!(chunk.features.len(), 6);
        assert!(matches!(
            chunk.features[0].geometry,
            VectorGeometry::Point(_)
        ));
    }
}
