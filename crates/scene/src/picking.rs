use foundation::math::Vec3;
use foundation::math::precision::stable_total_cmp_f64;

use crate::World;
use crate::components::VectorGeometryKind;
use crate::entity::EntityId;
use crate::spatial::{Bvh, Item as BvhItem};

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self { origin, dir }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PickHit {
    pub entity: EntityId,
    pub kind: VectorGeometryKind,
    pub distance: f64,
    pub point: Vec3,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PickOptions {
    pub max_distance: f64,
}

impl Default for PickOptions {
    fn default() -> Self {
        Self {
            max_distance: 1.0e30,
        }
    }
}

/// Deterministic ray picking for vector entities.
///
/// Ordering contract:
/// - If multiple entities are hit at the same distance, the lower `EntityId::index()` wins.
/// - Otherwise, the closest hit along the (normalized) ray wins.
///
/// Notes:
/// - This uses entity bounds (`World::bounds`) for intersection.
/// - Entities without explicit bounds are ignored.
/// - Visibility gating is inherited from `World::vector_geometries_by_entity()`.
pub fn pick_ray(world: &World, ray: Ray, opts: PickOptions) -> Option<PickHit> {
    let dir = normalize(ray.dir)?;

    // Collect visible vector entities and build a BVH over those with explicit bounds.
    let geoms = world.vector_geometries_by_entity();
    let mut kinds_by_index: Vec<Option<VectorGeometryKind>> = Vec::new();
    let mut bvh_items: Vec<BvhItem> = Vec::new();

    for (entity, _transform, component) in &geoms {
        let idx = entity.index() as usize;
        if kinds_by_index.len() <= idx {
            kinds_by_index.resize(idx + 1, None);
        }
        kinds_by_index[idx] = Some(component.kind);

        let Some(b) = world.bounds(*entity) else {
            continue;
        };
        bvh_items.push(BvhItem {
            entity: *entity,
            bounds: foundation::bounds::Aabb3::new(
                [b.min.x, b.min.y, b.min.z],
                [b.max.x, b.max.y, b.max.z],
            ),
        });
    }

    if bvh_items.is_empty() {
        return None;
    }

    let bvh = Bvh::build(bvh_items);
    let origin = [ray.origin.x, ray.origin.y, ray.origin.z];
    let dir_a = [dir.x, dir.y, dir.z];

    let mut best: Option<(f64, EntityId, VectorGeometryKind)> = None;

    for entity in bvh.query_ray(origin, dir_a, 0.0, opts.max_distance) {
        let Some(b) = world.bounds(entity) else {
            continue;
        };
        let Some(kind) = kinds_by_index.get(entity.index() as usize).and_then(|k| *k) else {
            continue;
        };

        let Some(t) = ray_aabb_hit_t(origin, dir_a, b, 0.0, opts.max_distance) else {
            continue;
        };

        best = match best {
            None => Some((t, entity, kind)),
            Some((bt, be, bk)) => {
                let ord = stable_total_cmp_f64(t, bt).then_with(|| entity.index().cmp(&be.index()));
                if ord.is_lt() {
                    Some((t, entity, kind))
                } else {
                    Some((bt, be, bk))
                }
            }
        };
    }

    let (t, entity, kind) = best?;
    let point = Vec3::new(
        ray.origin.x + dir.x * t,
        ray.origin.y + dir.y * t,
        ray.origin.z + dir.z * t,
    );

    Some(PickHit {
        entity,
        kind,
        distance: t,
        point,
    })
}

/// Screen picking wrapper.
///
/// The caller supplies a deterministic screen->ray mapping via `make_ray`.
pub fn pick_screen<F>(
    world: &World,
    x_px: f64,
    y_px: f64,
    mut make_ray: F,
    opts: PickOptions,
) -> Option<PickHit>
where
    F: FnMut(f64, f64) -> Option<Ray>,
{
    let ray = make_ray(x_px, y_px)?;
    pick_ray(world, ray, opts)
}

fn normalize(v: Vec3) -> Option<Vec3> {
    let l2 = v.dot(v);
    if l2 <= 0.0 {
        return None;
    }
    let inv = 1.0 / l2.sqrt();
    Some(Vec3::new(v.x * inv, v.y * inv, v.z * inv))
}

fn ray_aabb_hit_t(
    origin: [f64; 3],
    dir: [f64; 3],
    bounds: crate::components::ComponentBounds,
    mut t_min: f64,
    mut t_max: f64,
) -> Option<f64> {
    // Slabs intersection; returns entry distance.
    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        let (min, max) = match axis {
            0 => (bounds.min.x, bounds.max.x),
            1 => (bounds.min.y, bounds.max.y),
            _ => (bounds.min.z, bounds.max.z),
        };

        if d.abs() < 1e-12 {
            if o < min || o > max {
                return None;
            }
            continue;
        }

        let inv = 1.0 / d;
        let mut t1 = (min - o) * inv;
        let mut t2 = (max - o) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }

        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_max < t_min {
            return None;
        }
    }

    Some(t_min.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::{PickOptions, Ray, pick_ray};
    use crate::World;
    use crate::components::{
        ComponentBounds, ComponentVectorGeometry, Transform, VectorGeometry, VectorGeometryKind,
    };
    use foundation::math::Vec3;

    #[test]
    fn ray_picks_nearest_hit() {
        let mut world = World::new();

        let a = world.spawn();
        world.set_transform(a, Transform::identity());
        world.set_bounds(
            a,
            ComponentBounds::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
        );
        let ga = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(5.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            a,
            ComponentVectorGeometry::new(ga, VectorGeometryKind::Point),
        );

        let b = world.spawn();
        world.set_transform(b, Transform::identity());
        world.set_bounds(
            b,
            ComponentBounds::new(Vec3::new(9.0, -1.0, -1.0), Vec3::new(11.0, 1.0, 1.0)),
        );
        let gb = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(10.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            b,
            ComponentVectorGeometry::new(gb, VectorGeometryKind::Point),
        );

        let ray = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hit = pick_ray(&world, ray, PickOptions::default()).expect("hit");
        assert_eq!(hit.entity, a);
        assert!(hit.distance >= 4.0 && hit.distance <= 6.0);
    }

    #[test]
    fn tie_breaks_by_entity_index() {
        let mut world = World::new();

        let e2 = world.spawn(); // index 0
        world.set_transform(e2, Transform::identity());
        world.set_bounds(
            e2,
            ComponentBounds::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
        );
        let g2 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(5.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e2,
            ComponentVectorGeometry::new(g2, VectorGeometryKind::Point),
        );

        let e1 = world.spawn(); // index 1
        world.set_transform(e1, Transform::identity());
        world.set_bounds(
            e1,
            ComponentBounds::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
        );
        let g1 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(5.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e1,
            ComponentVectorGeometry::new(g1, VectorGeometryKind::Point),
        );

        let ray = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hit = pick_ray(&world, ray, PickOptions::default()).expect("hit");
        assert_eq!(hit.entity, e2);
    }
}
