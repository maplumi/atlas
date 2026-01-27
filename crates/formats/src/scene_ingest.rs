use foundation::math::{Geodetic, Vec3, geodetic_to_ecef};
use scene::World;
use scene::components::{ComponentVectorGeometry, Transform, VectorGeometry, VectorGeometryKind};

use crate::vector_chunk::{VectorChunk, VectorGeometry as ChunkGeometry};

pub fn ingest_vector_chunk(
    world: &mut World,
    chunk: &VectorChunk,
    expected: Option<VectorGeometryKind>,
) {
    for feature in &chunk.features {
        match &feature.geometry {
            ChunkGeometry::Point(p) => {
                ingest_point(world, p.lon_deg, p.lat_deg, expected);
            }
            ChunkGeometry::MultiPoint(points) => {
                for p in points {
                    ingest_point(world, p.lon_deg, p.lat_deg, expected);
                }
            }
            ChunkGeometry::LineString(points) => {
                ingest_line(
                    world,
                    points.iter().map(|p| (p.lon_deg, p.lat_deg)).collect(),
                    expected,
                );
            }
            ChunkGeometry::MultiLineString(lines) => {
                for line in lines {
                    ingest_line(
                        world,
                        line.iter().map(|p| (p.lon_deg, p.lat_deg)).collect(),
                        expected,
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
) {
    if let Some(exp) = expected
        && exp != VectorGeometryKind::Point
    {
        return;
    }

    let position = ecef_from_lon_lat_deg(lon_deg, lat_deg);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));

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

    let position = centroid(&vertices);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));

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

    let position = centroid(&outer);
    let entity = world.spawn();
    world.set_transform(entity, Transform::translate(position));

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
