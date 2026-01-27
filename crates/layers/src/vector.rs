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

    // Flatten rings into 2D coordinates + a parallel 3D vertex list.
    // Also remove a closing duplicate point if present.
    let mut vertices_3d: Vec<Vec3> = Vec::new();
    let mut coords_2d: Vec<f64> = Vec::new();
    let mut hole_indices: Vec<usize> = Vec::new();

    for (ring_i, ring) in rings.iter().enumerate() {
        let mut ring_pts: Vec<Vec3> = ring.clone();
        drop_closing_duplicate(&mut ring_pts);
        if ring_pts.len() < 3 {
            continue;
        }

        if ring_i > 0 {
            hole_indices.push(vertices_3d.len());
        }

        for p in ring_pts {
            let v = Vec3::new(p.x - origin.x, p.y - origin.y, p.z - origin.z);
            let x = dot(v, east);
            let y = dot(v, north);
            coords_2d.push(x);
            coords_2d.push(y);
            vertices_3d.push(p);
        }
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
