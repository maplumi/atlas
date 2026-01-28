use foundation::bounds::Aabb3;
use foundation::time::{Time, TimeSpan};
use scene::components::VectorGeometryKind;
use scene::spatial::{Bvh, Item as BvhItem};
use scene::{World, entity::EntityId};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyOp {
    Eq,
    Contains,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFilter {
    pub key: String,
    pub op: PropertyOp,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct VectorQuery {
    pub kind: Option<VectorGeometryKind>,
    pub time: Option<Time>,
    pub bbox_world_ecef: Option<Aabb3>,
    pub properties: Vec<PropertyFilter>,
    pub limit: usize,
}

impl Default for VectorQuery {
    fn default() -> Self {
        Self {
            kind: None,
            time: None,
            bbox_world_ecef: None,
            properties: Vec::new(),
            limit: 1000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VectorQueryHit {
    pub entity: EntityId,
    pub kind: VectorGeometryKind,
}

fn time_allows(span: Option<TimeSpan>, time: Option<Time>) -> bool {
    let Some(time) = time else {
        return true;
    };
    let Some(span) = span else {
        // If unset, treat as always visible.
        return true;
    };
    !(time.0 < span.start.0 || time.0 > span.end.0)
}

fn properties_match(world: &World, entity: EntityId, filters: &[PropertyFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let Some(props) = world.properties(entity) else {
        return false;
    };

    'filters: for f in filters {
        let mut found = false;
        for (k, v) in &props.pairs {
            if k != &f.key {
                continue;
            }
            found = match f.op {
                PropertyOp::Eq => v == &f.value,
                PropertyOp::Contains => v.contains(&f.value),
            };
            if found {
                continue 'filters;
            }
        }
        if !found {
            return false;
        }
    }

    true
}

pub fn query_vector(world: &World, query: &VectorQuery) -> Vec<VectorQueryHit> {
    let mut out: Vec<VectorQueryHit> = Vec::new();

    let geoms = world.vector_geometries_by_entity();

    if let Some(aabb) = query.bbox_world_ecef {
        // Build deterministic lookup for candidate retrieval.
        let mut by_index: BTreeMap<u32, (EntityId, VectorGeometryKind)> = BTreeMap::new();
        let mut bvh_items: Vec<BvhItem> = Vec::new();

        for (entity, _transform, component) in &geoms {
            by_index.insert(entity.index(), (*entity, component.kind));

            let Some(b) = world.bounds(*entity) else {
                continue;
            };
            bvh_items.push(BvhItem {
                entity: *entity,
                bounds: Aabb3::new([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]),
            });
        }

        let bvh = Bvh::build(bvh_items);
        for entity in bvh.query_aabb(&aabb) {
            let Some((_e, kind)) = by_index.get(&entity.index()) else {
                continue;
            };

            if let Some(k) = query.kind
                && *kind != k
            {
                continue;
            }

            if !time_allows(world.time_span(entity), query.time) {
                continue;
            }

            if !properties_match(world, entity, &query.properties) {
                continue;
            }

            out.push(VectorQueryHit {
                entity,
                kind: *kind,
            });
            if out.len() >= query.limit {
                break;
            }
        }

        return out;
    }

    for (entity, _transform, component) in geoms {
        if let Some(kind) = query.kind
            && component.kind != kind
        {
            continue;
        }

        if !time_allows(world.time_span(entity), query.time) {
            continue;
        }

        if !properties_match(world, entity, &query.properties) {
            continue;
        }

        out.push(VectorQueryHit {
            entity,
            kind: component.kind,
        });
        if out.len() >= query.limit {
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{VectorQuery, query_vector};
    use foundation::bounds::Aabb3;
    use foundation::math::Vec3;
    use scene::World;
    use scene::components::{
        ComponentBounds, ComponentVectorGeometry, Transform, VectorGeometry, VectorGeometryKind,
    };

    #[test]
    fn bbox_query_requires_bounds() {
        let mut world = World::new();

        // Entity A: has transform + vector geometry, but NO bounds.
        let a = world.spawn();
        world.set_transform(a, Transform::translate(Vec3::new(1.0, 2.0, 3.0)));
        let a_geom = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(1.0, 2.0, 3.0),
        });
        world.set_vector_geometry(
            a,
            ComponentVectorGeometry::new(a_geom, VectorGeometryKind::Point),
        );

        // Entity B: same, but with explicit bounds.
        let b = world.spawn();
        world.set_transform(b, Transform::translate(Vec3::new(5.0, 0.0, 0.0)));
        world.set_bounds(
            b,
            ComponentBounds::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
        );
        let b_geom = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(5.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            b,
            ComponentVectorGeometry::new(b_geom, VectorGeometryKind::Point),
        );

        let q = VectorQuery {
            bbox_world_ecef: Some(Aabb3::new([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            ..Default::default()
        };

        let hits = query_vector(&world, &q);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity, b);
    }
}
