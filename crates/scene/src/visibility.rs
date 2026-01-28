use foundation::bounds::Aabb3;

use crate::World;
use crate::components::VectorGeometryKind;
use crate::selection::SelectionSet;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Plane {
    pub n: [f64; 3],
    pub d: f64,
}

impl Plane {
    pub fn new(n: [f64; 3], d: f64) -> Self {
        Self { n, d }
    }

    pub fn normalize(self) -> Self {
        let l2 = self.n[0] * self.n[0] + self.n[1] * self.n[1] + self.n[2] * self.n[2];
        if l2 <= 0.0 {
            return self;
        }
        let inv = 1.0 / l2.sqrt();
        Self {
            n: [self.n[0] * inv, self.n[1] * inv, self.n[2] * inv],
            d: self.d * inv,
        }
    }

    pub fn distance(&self, p: [f64; 3]) -> f64 {
        self.n[0] * p[0] + self.n[1] * p[1] + self.n[2] * p[2] + self.d
    }
}

/// View frustum as 6 planes.
///
/// Convention:
/// - A point `p` is inside iff `plane.distance(p) >= 0` for all planes.
/// - Planes are expected to be in world space.
///
/// Ordering contract:
/// - `cull_vector_entities_in_frustum` returns a `SelectionSet` (ascending `EntityId::index()` iteration).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Frustum {
    pub left: Plane,
    pub right: Plane,
    pub bottom: Plane,
    pub top: Plane,
    pub near: Plane,
    pub far: Plane,
}

impl Frustum {
    pub fn new(
        left: Plane,
        right: Plane,
        bottom: Plane,
        top: Plane,
        near: Plane,
        far: Plane,
    ) -> Self {
        Self {
            left,
            right,
            bottom,
            top,
            near,
            far,
        }
    }

    pub fn normalize(self) -> Self {
        Self {
            left: self.left.normalize(),
            right: self.right.normalize(),
            bottom: self.bottom.normalize(),
            top: self.top.normalize(),
            near: self.near.normalize(),
            far: self.far.normalize(),
        }
    }

    /// Build a frustum from a row-major view-projection matrix.
    ///
    /// This expects the clip-space convention where visible points satisfy:
    /// - `-w <= x <= w`
    /// - `-w <= y <= w`
    /// - `0 <= z <= w` (z0)
    pub fn from_view_proj_row_major(m: [[f64; 4]; 4]) -> Self {
        // Rows r0..r3
        let r0 = m[0];
        let r1 = m[1];
        let r2 = m[2];
        let r3 = m[3];

        // Planes: r3 +/- r{0,1,2}
        // Left:  r3 + r0
        // Right: r3 - r0
        // Bottom:r3 + r1
        // Top:   r3 - r1
        // Near:  r3 + r2  (z0)
        // Far:   r3 - r2
        let left = Plane::new([r3[0] + r0[0], r3[1] + r0[1], r3[2] + r0[2]], r3[3] + r0[3]);
        let right = Plane::new([r3[0] - r0[0], r3[1] - r0[1], r3[2] - r0[2]], r3[3] - r0[3]);
        let bottom = Plane::new([r3[0] + r1[0], r3[1] + r1[1], r3[2] + r1[2]], r3[3] + r1[3]);
        let top = Plane::new([r3[0] - r1[0], r3[1] - r1[1], r3[2] - r1[2]], r3[3] - r1[3]);
        let near = Plane::new([r3[0] + r2[0], r3[1] + r2[1], r3[2] + r2[2]], r3[3] + r2[3]);
        let far = Plane::new([r3[0] - r2[0], r3[1] - r2[1], r3[2] - r2[2]], r3[3] - r2[3]);

        Self::new(left, right, bottom, top, near, far).normalize()
    }

    pub fn intersects_aabb(&self, aabb: &Aabb3) -> bool {
        // If the AABB is entirely outside any plane, it doesn't intersect.
        // Use the p-vertex test: choose the vertex most in the direction of the plane normal.
        for plane in [
            self.left,
            self.right,
            self.bottom,
            self.top,
            self.near,
            self.far,
        ] {
            let px = if plane.n[0] >= 0.0 {
                aabb.max[0]
            } else {
                aabb.min[0]
            };
            let py = if plane.n[1] >= 0.0 {
                aabb.max[1]
            } else {
                aabb.min[1]
            };
            let pz = if plane.n[2] >= 0.0 {
                aabb.max[2]
            } else {
                aabb.min[2]
            };
            if plane.distance([px, py, pz]) < 0.0 {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct FrustumCullQuery {
    pub kind: Option<VectorGeometryKind>,
    pub limit: usize,
}

impl Default for FrustumCullQuery {
    fn default() -> Self {
        Self {
            kind: None,
            limit: 1000,
        }
    }
}

/// Cull vector entities against a world-space frustum.
///
/// Notes:
/// - This uses entity bounds (`World::bounds`) as a proxy for geometry.
/// - Entities without explicit bounds are ignored.
/// - Visibility gating is inherited from `World::vector_geometries_by_entity()`.
pub fn cull_vector_entities_in_frustum(
    world: &World,
    frustum: &Frustum,
    query: &FrustumCullQuery,
) -> SelectionSet {
    let mut out = SelectionSet::new();

    for (entity, _transform, component) in world.vector_geometries_by_entity() {
        if let Some(kind) = query.kind
            && component.kind != kind
        {
            continue;
        }

        let Some(b) = world.bounds(entity) else {
            continue;
        };

        let aabb = Aabb3::new([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]);
        if !frustum.intersects_aabb(&aabb) {
            continue;
        }

        out.insert(entity);
        if out.len() >= query.limit {
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{Frustum, FrustumCullQuery, Plane, cull_vector_entities_in_frustum};
    use crate::World;
    use crate::components::{
        ComponentBounds, ComponentVectorGeometry, Transform, VectorGeometry, VectorGeometryKind,
    };
    use foundation::math::Vec3;

    fn unit_cube_frustum() -> Frustum {
        // Cube: -1<=x<=1, -1<=y<=1, -1<=z<=1
        // Planes in the form n·p + d >= 0
        let left = Plane::new([1.0, 0.0, 0.0], 1.0); // x >= -1
        let right = Plane::new([-1.0, 0.0, 0.0], 1.0); // x <= 1
        let bottom = Plane::new([0.0, 1.0, 0.0], 1.0); // y >= -1
        let top = Plane::new([0.0, -1.0, 0.0], 1.0); // y <= 1
        let near = Plane::new([0.0, 0.0, 1.0], 1.0); // z >= -1
        let far = Plane::new([0.0, 0.0, -1.0], 1.0); // z <= 1
        Frustum::new(left, right, bottom, top, near, far)
    }

    #[test]
    fn intersects_aabb_basic() {
        let f = unit_cube_frustum();
        assert!(f.intersects_aabb(&foundation::bounds::Aabb3::new(
            [-0.5, -0.5, -0.5],
            [0.5, 0.5, 0.5]
        )));
        assert!(!f.intersects_aabb(&foundation::bounds::Aabb3::new(
            [2.0, 2.0, 2.0],
            [3.0, 3.0, 3.0]
        )));
    }

    #[test]
    fn cull_returns_sorted_entities_via_selection_set() {
        let mut world = World::new();

        let a = world.spawn();
        world.set_transform(a, Transform::identity());
        world.set_bounds(
            a,
            ComponentBounds::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(0.5, 0.5, 0.5)),
        );
        let ga = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(0.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            a,
            ComponentVectorGeometry::new(ga, VectorGeometryKind::Point),
        );

        let b = world.spawn();
        world.set_transform(b, Transform::identity());
        world.set_bounds(
            b,
            ComponentBounds::new(Vec3::new(10.0, 10.0, 10.0), Vec3::new(11.0, 11.0, 11.0)),
        );
        let gb = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(10.0, 10.0, 10.0),
        });
        world.set_vector_geometry(
            b,
            ComponentVectorGeometry::new(gb, VectorGeometryKind::Point),
        );

        let f = unit_cube_frustum();
        let q = FrustumCullQuery {
            kind: Some(VectorGeometryKind::Point),
            limit: 1000,
        };
        let hits = cull_vector_entities_in_frustum(&world, &f, &q);
        let got: Vec<u32> = hits.iter_indices().collect();

        assert_eq!(got, vec![a.index()]);
    }
}
