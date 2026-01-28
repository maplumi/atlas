use foundation::math::{Geodetic, Vec3, geodetic_to_ecef};
use foundation::time::{Time, TimeSpan};
use scene::World;
use scene::components::{
    ComponentBounds, ComponentProperties, ComponentTimeSpan, ComponentVectorGeometry, Transform,
    VectorGeometry, VectorGeometryKind,
};
use serde_json::Value;

use crate::vector_chunk::{VectorChunk, VectorFeature, VectorGeometry as ChunkGeometry};

fn infer_time_span(feature: &VectorFeature) -> TimeSpan {
    // Very small-but-useful convention:
    // - If properties contain numeric "time" or "timestamp": treat as seconds and create an instant span.
    // - Else if contain numeric "start" and "end": treat as seconds and create a range.
    // - Else: forever.
    let get_num = |k: &str| -> Option<f64> {
        feature.properties.get(k).and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
    };

    if let Some(t) = get_num("time").or_else(|| get_num("timestamp")) {
        return TimeSpan::instant(Time(t));
    }
    if let (Some(s), Some(e)) = (get_num("start"), get_num("end")) {
        return TimeSpan {
            start: Time(s),
            end: Time(e),
        };
    }

    TimeSpan::forever()
}

fn properties_to_pairs(feature: &VectorFeature) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(feature.properties.len() + 1);
    if let Some(id) = &feature.id {
        out.push(("id".to_string(), id.clone()));
    }
    for (k, v) in &feature.properties {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
            other => other.to_string(),
        };
        out.push((k.clone(), s));
    }
    out
}

pub fn ingest_vector_chunk(
    world: &mut World,
    chunk: &VectorChunk,
    expected: Option<VectorGeometryKind>,
) {
    for feature in &chunk.features {
        let span = infer_time_span(feature);
        let props = ComponentProperties::new(properties_to_pairs(feature));

        match &feature.geometry {
            ChunkGeometry::Point(p) => {
                ingest_point(world, p.lon_deg, p.lat_deg, expected, span, &props);
            }
            ChunkGeometry::MultiPoint(points) => {
                for p in points {
                    ingest_point(world, p.lon_deg, p.lat_deg, expected, span, &props);
                }
            }
            ChunkGeometry::LineString(points) => {
                ingest_line(
                    world,
                    points.iter().map(|p| (p.lon_deg, p.lat_deg)).collect(),
                    expected,
                    span,
                    &props,
                );
            }
            ChunkGeometry::MultiLineString(lines) => {
                for line in lines {
                    ingest_line(
                        world,
                        line.iter().map(|p| (p.lon_deg, p.lat_deg)).collect(),
                        expected,
                        span,
                        &props,
                    );
                }
            }
            ChunkGeometry::Polygon(rings) => {
                ingest_area(
                    world,
                    rings
                        .iter()
                        .map(|ring| ring.iter().map(|p| (p.lon_deg, p.lat_deg)).collect())
                        .collect(),
                    expected,
                    span,
                    &props,
                );
            }
            ChunkGeometry::MultiPolygon(polys) => {
                for poly in polys {
                    ingest_area(
                        world,
                        poly.iter()
                            .map(|ring| ring.iter().map(|p| (p.lon_deg, p.lat_deg)).collect())
                            .collect(),
                        expected,
                        span,
                        &props,
                    );
                }
            }
        }
    }
}

fn ingest_point(
    world: &mut World,
    lon_deg: f64,
    lat_deg: f64,
    expected: Option<VectorGeometryKind>,
    span: TimeSpan,
    props: &ComponentProperties,
) {
    if let Some(exp) = expected
        && exp != VectorGeometryKind::Point
    {
        return;
    }

    let position = ecef_from_lon_lat_deg(lon_deg, lat_deg);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));
    world.set_bounds(entity, bounds_from_points(std::iter::once(position)));
    world.set_time_span(entity, ComponentTimeSpan::new(span));
    world.set_properties(entity, props.clone());

    let geom_id = world.add_vector_geometry(VectorGeometry::Point { position });
    world.set_vector_geometry(
        entity,
        ComponentVectorGeometry::new(geom_id, VectorGeometryKind::Point),
    );
}

fn ingest_line(
    world: &mut World,
    points_lon_lat: Vec<(f64, f64)>,
    expected: Option<VectorGeometryKind>,
    span: TimeSpan,
    props: &ComponentProperties,
) {
    if let Some(exp) = expected
        && exp != VectorGeometryKind::Line
    {
        return;
    }

    let vertices: Vec<Vec3> = points_lon_lat
        .into_iter()
        .map(|(lon, lat)| ecef_from_lon_lat_deg(lon, lat))
        .collect();
    if vertices.is_empty() {
        return;
    }

    let bounds = bounds_from_points(vertices.iter().copied());

    let position = centroid(&vertices);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));
    world.set_bounds(entity, bounds);
    world.set_time_span(entity, ComponentTimeSpan::new(span));
    world.set_properties(entity, props.clone());

    let geom_id = world.add_vector_geometry(VectorGeometry::Line { vertices });
    world.set_vector_geometry(
        entity,
        ComponentVectorGeometry::new(geom_id, VectorGeometryKind::Line),
    );
}

fn ingest_area(
    world: &mut World,
    rings_lon_lat: Vec<Vec<(f64, f64)>>,
    expected: Option<VectorGeometryKind>,
    span: TimeSpan,
    props: &ComponentProperties,
) {
    if let Some(exp) = expected
        && exp != VectorGeometryKind::Area
    {
        return;
    }

    let rings: Vec<Vec<Vec3>> = rings_lon_lat
        .into_iter()
        .map(|ring| {
            ring.into_iter()
                .map(|(lon, lat)| ecef_from_lon_lat_deg(lon, lat))
                .collect()
        })
        .collect();
    let outer = rings.first().cloned().unwrap_or_default();
    if outer.is_empty() {
        return;
    }

    let bounds = bounds_from_rings(&rings);

    let position = centroid(&outer);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));
    world.set_bounds(entity, bounds);
    world.set_time_span(entity, ComponentTimeSpan::new(span));
    world.set_properties(entity, props.clone());

    let geom_id = world.add_vector_geometry(VectorGeometry::Area { rings });
    world.set_vector_geometry(
        entity,
        ComponentVectorGeometry::new(geom_id, VectorGeometryKind::Area),
    );
}

fn ecef_from_lon_lat_deg(lon_deg: f64, lat_deg: f64) -> Vec3 {
    let geo = Geodetic::new(lat_deg.to_radians(), lon_deg.to_radians(), 0.0);
    let ecef = geodetic_to_ecef(geo);
    Vec3::new(ecef.x, ecef.y, ecef.z)
}

fn centroid(vertices: &[Vec3]) -> Vec3 {
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut sz = 0.0;
    for v in vertices {
        sx += v.x;
        sy += v.y;
        sz += v.z;
    }
    let n = vertices.len() as f64;
    Vec3::new(sx / n, sy / n, sz / n)
}

fn bounds_from_points(points: impl Iterator<Item = Vec3>) -> ComponentBounds {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        min.z = min.z.min(p.z);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
        max.z = max.z.max(p.z);
    }
    ComponentBounds::new(min, max)
}

fn bounds_from_rings(rings: &[Vec<Vec3>]) -> ComponentBounds {
    bounds_from_points(rings.iter().flat_map(|r| r.iter().copied()))
}
