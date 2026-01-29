use earcutr::earcut;
use foundation::math::{Vec3, WGS84_A, WGS84_B};
use scene::World;
use scene::components::VectorGeometry;

use crate::layer::{Layer, LayerId};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct VectorLayer {
    id: LayerId,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct VectorLayerSnapshot {
    pub points: Vec<Vec3>,
    pub lines: Vec<Vec<Vec3>>,
    // Flat triangle list (3 vertices per triangle) in world coordinates.
    pub area_triangles: Vec<Vec3>,
}

impl VectorLayer {
    pub fn new(id: u64) -> Self {
        Self { id: LayerId(id) }
    }

    pub fn extract(&self, world: &World) -> VectorLayerSnapshot {
        let mut out = VectorLayerSnapshot::default();

        for (_entity, _transform, component) in world.vector_geometries_by_entity() {
            let Some(geom) = world.vector_geometry(component.id) else {
                continue;
            };
            match geom {
                VectorGeometry::Point { position } => out.points.push(*position),
                VectorGeometry::Line { vertices } => out.lines.push(vertices.clone()),
                VectorGeometry::Area { rings } => {
                    out.area_triangles.extend(triangulate_area_rings(rings));
                }
            }
        }

        out
    }
}

fn triangulate_area_rings(rings: &[Vec<Vec3>]) -> Vec<Vec3> {
    const MAX_RING_VERTICES: usize = 50_000;
    const MAX_TOTAL_VERTICES: usize = 200_000;

    fn is_finite_vec3(v: Vec3) -> bool {
        v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
    }

    fn drop_consecutive_duplicates(points: &mut Vec<Vec3>) {
        points.dedup_by(|a, b| {
            (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
        });
    }

    // Triangulate in a local tangent plane at the centroid of the outer ring.
    // This is a pragmatic choice for rendering and matches the viewer's approach.
    let Some(outer) = rings.first() else {
        return Vec::new();
    };
    if outer.len() < 3 {
        return Vec::new();
    }

    let origin = centroid(outer);
    let n = ellipsoid_normal_ecef(origin);

    // Build tangent basis.
    let up = if n.z.abs() < 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let east = normalize(cross(up, n));
    let north = cross(n, east);

    // Degenerate basis (or invalid normal) -> skip triangulation rather than risk NaNs.
    let east_l2 = dot(east, east);
    let north_l2 = dot(north, north);
    if !(east_l2.is_finite() && north_l2.is_finite()) || east_l2 < 1e-20 || north_l2 < 1e-20 {
        return Vec::new();
    }

    // Flatten rings into 2D coordinates + a parallel 3D vertex list.
    // Also remove a closing duplicate point if present.
    let mut vertices_3d: Vec<Vec3> = Vec::new();
    let mut coords_2d: Vec<f64> = Vec::new();
    let mut hole_indices: Vec<usize> = Vec::new();

    // GeoJSON polygon rings are "outer first, then holes", but we may skip degenerate rings.
    // Track the first *accepted* ring as the outer ring; only then are subsequent rings holes.
    let mut have_outer = false;

    for ring in rings {
        if ring.len() > MAX_RING_VERTICES {
            continue;
        }

        let mut ring_pts: Vec<Vec3> = ring
            .iter()
            .copied()
            .filter(|p| is_finite_vec3(*p))
            .collect();
        drop_closing_duplicate(&mut ring_pts);
        drop_consecutive_duplicates(&mut ring_pts);
        if ring_pts.len() < 3 {
            continue;
        }

        // Project ring into tangent plane; if any projection goes invalid, skip the ring.
        let mut tmp_vertices: Vec<Vec3> = Vec::with_capacity(ring_pts.len());
        let mut tmp_coords: Vec<f64> = Vec::with_capacity(ring_pts.len() * 2);
        let mut projection_ok = true;
        for p in ring_pts {
            let v = Vec3::new(p.x - origin.x, p.y - origin.y, p.z - origin.z);
            let x = dot(v, east);
            let y = dot(v, north);
            if !(x.is_finite() && y.is_finite()) {
                projection_ok = false;
                break;
            }
            tmp_coords.push(x);
            tmp_coords.push(y);
            tmp_vertices.push(p);
        }
        if !projection_ok || tmp_vertices.len() < 3 {
            continue;
        }

        if vertices_3d.len().saturating_add(tmp_vertices.len()) > MAX_TOTAL_VERTICES {
            return Vec::new();
        }

        if have_outer {
            hole_indices.push(vertices_3d.len());
        } else {
            have_outer = true;
        }
        coords_2d.extend(tmp_coords);
        vertices_3d.extend(tmp_vertices);
    }

    if vertices_3d.len() < 3 {
        return Vec::new();
    }

    let indices = match earcut(&coords_2d, &hole_indices, 2) {
        Ok(ix) => ix,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<Vec3> = Vec::with_capacity(indices.len());
    for idx in indices {
        if let Some(v) = vertices_3d.get(idx) {
            out.push(*v);
        }
    }
    out
}

fn drop_closing_duplicate(points: &mut Vec<Vec3>) {
    if points.len() >= 2 {
        let first = points[0];
        let last = *points.last().unwrap();
        if (first.x - last.x).abs() < 1e-9
            && (first.y - last.y).abs() < 1e-9
            && (first.z - last.z).abs() < 1e-9
        {
            points.pop();
        }
    }
}

fn ellipsoid_normal_ecef(p: Vec3) -> Vec3 {
    // WGS84 ellipsoid in scene coordinates (ECEF): x/y semi-axis = A, z semi-axis = B.
    // Normal is gradient of (x^2/A^2 + y^2/A^2 + z^2/B^2).
    let a2 = WGS84_A * WGS84_A;
    let b2 = WGS84_B * WGS84_B;
    normalize(Vec3::new(p.x / a2, p.y / a2, p.z / b2))
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

fn dot(a: Vec3, b: Vec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

fn normalize(v: Vec3) -> Vec3 {
    let l2 = dot(v, v);
    if l2 <= 0.0 {
        return v;
    }
    let inv = 1.0 / l2.sqrt();
    Vec3::new(v.x * inv, v.y * inv, v.z * inv)
}

impl Layer for VectorLayer {
    fn id(&self) -> LayerId {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::VectorLayer;
    use formats::load_world_from_package_dir;

    #[test]
    fn extracts_demo_snapshot() {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../apps/viewer_web/assets");
        let world = load_world_from_package_dir(root).expect("load world");
        let layer = VectorLayer::new(1);
        let snap = layer.extract(&world);
        assert!(!snap.points.is_empty());
    }
}
