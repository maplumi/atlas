use foundation::bounds::Aabb3;
use foundation::time::{Time, TimeSpan};
use std::collections::BTreeMap;

use crate::World;
use crate::components::VectorGeometryKind;
use crate::entity::EntityId;
use crate::selection::SelectionSet;
use crate::spatial::{Bvh, Item as BvhItem};

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

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TimeFilter {
    At(Time),
    Overlaps(TimeSpan),
}

/// Unified core query for vector entities.
///
/// Atlas queries are intersections over:
/// - spatial (bbox)
/// - temporal (`TimeFilter`)
/// - visibility (handled by `World::vector_geometries_by_entity`)
/// - attributes (`PropertyFilter`)
///
/// Ordering contract:
/// - `query_vector_entities` returns a `SelectionSet` whose iteration is in ascending `EntityId::index()` order.
#[derive(Debug, Clone)]
pub struct VectorEntityQuery {
    pub kind: Option<VectorGeometryKind>,
    pub time: Option<TimeFilter>,
    pub bbox_world_ecef: Option<Aabb3>,
    pub properties: Vec<PropertyFilter>,
    pub limit: usize,
}

impl Default for VectorEntityQuery {
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

fn time_allows(span: Option<TimeSpan>, filter: Option<TimeFilter>) -> bool {
    let Some(filter) = filter else {
        return true;
    };

    let Some(span) = span else {
        // If unset, treat as always visible.
        return true;
    };

    match filter {
        TimeFilter::At(t) => !(t.0 < span.start.0 || t.0 > span.end.0),
        TimeFilter::Overlaps(q) => !(span.end.0 < q.start.0 || span.start.0 > q.end.0),
    }
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

pub fn query_vector_entities(world: &World, query: &VectorEntityQuery) -> SelectionSet {
    let mut out = SelectionSet::new();

    let geoms = world.vector_geometries_by_entity();

    if let Some(aabb) = query.bbox_world_ecef {
        // Build deterministic lookup for candidate retrieval.
        let mut by_index: BTreeMap<u32, VectorGeometryKind> = BTreeMap::new();
        let mut bvh_items: Vec<BvhItem> = Vec::new();

        for (entity, _transform, component) in &geoms {
            by_index.insert(entity.index(), component.kind);

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
            let Some(kind) = by_index.get(&entity.index()).copied() else {
                continue;
            };

            if let Some(k) = query.kind
                && kind != k
            {
                continue;
            }

            if !time_allows(world.time_span(entity), query.time) {
                continue;
            }

            if !properties_match(world, entity, &query.properties) {
                continue;
            }

            out.insert(entity);
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

        out.insert(entity);
        if out.len() >= query.limit {
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{PropertyFilter, PropertyOp, TimeFilter, VectorEntityQuery, query_vector_entities};
    use foundation::bounds::Aabb3;
    use foundation::math::Vec3;
    use foundation::time::{Time, TimeSpan};

    use crate::World;
    use crate::components::{
        ComponentBounds, ComponentProperties, ComponentTimeSpan, ComponentVectorGeometry,
        Transform, VectorGeometry, VectorGeometryKind,
    };

    fn span(a: f64, b: f64) -> TimeSpan {
        TimeSpan {
            start: Time(a),
            end: Time(b),
        }
    }

    #[test]
    fn results_are_deterministic_and_sorted() {
        let mut world = World::new();

        let e1 = world.spawn();
        world.set_transform(e1, Transform::translate(Vec3::new(0.0, 0.0, 0.0)));
        world.set_bounds(
            e1,
            ComponentBounds::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
        );
        world.set_time_span(e1, ComponentTimeSpan::new(span(0.0, 10.0)));
        world.set_properties(
            e1,
            ComponentProperties::new(vec![("name".into(), "alpha".into())]),
        );
        let g1 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(0.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e1,
            ComponentVectorGeometry::new(g1, VectorGeometryKind::Point),
        );

        let e2 = world.spawn();
        world.set_transform(e2, Transform::translate(Vec3::new(5.0, 0.0, 0.0)));
        world.set_bounds(
            e2,
            ComponentBounds::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
        );
        world.set_time_span(e2, ComponentTimeSpan::new(span(-5.0, -1.0)));
        world.set_properties(
            e2,
            ComponentProperties::new(vec![("name".into(), "beta".into())]),
        );
        let g2 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(5.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e2,
            ComponentVectorGeometry::new(g2, VectorGeometryKind::Point),
        );

        let e3 = world.spawn();
        world.set_transform(e3, Transform::translate(Vec3::new(2.0, 0.0, 0.0)));
        // Intentionally no bounds for e3.
        // Intentionally no time span for e3 (treated as always visible).
        world.set_properties(
            e3,
            ComponentProperties::new(vec![("name".into(), "alpha-2".into())]),
        );
        let g3 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(2.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e3,
            ComponentVectorGeometry::new(g3, VectorGeometryKind::Point),
        );

        let q = VectorEntityQuery {
            kind: Some(VectorGeometryKind::Point),
            time: Some(TimeFilter::At(Time(1.0))),
            bbox_world_ecef: Some(Aabb3::new([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0])),
            properties: vec![PropertyFilter {
                key: "name".into(),
                op: PropertyOp::Contains,
                value: "alpha".into(),
            }],
            limit: 1000,
        };

        // With bbox set, entities without explicit bounds are excluded.
        let hits = query_vector_entities(&world, &q);
        let got: Vec<u32> = hits.iter_indices().collect();
        assert_eq!(got, vec![e1.index()]);
    }

    #[test]
    fn time_overlaps_includes_entities_without_spans() {
        let mut world = World::new();

        let e1 = world.spawn();
        world.set_transform(e1, Transform::identity());
        world.set_time_span(e1, ComponentTimeSpan::new(span(0.0, 1.0)));
        let g1 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(0.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e1,
            ComponentVectorGeometry::new(g1, VectorGeometryKind::Point),
        );

        let e2 = world.spawn();
        world.set_transform(e2, Transform::identity());
        // no time span -> always visible
        let g2 = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(0.0, 0.0, 0.0),
        });
        world.set_vector_geometry(
            e2,
            ComponentVectorGeometry::new(g2, VectorGeometryKind::Point),
        );

        let q = VectorEntityQuery {
            time: Some(TimeFilter::Overlaps(span(0.5, 2.0))),
            ..Default::default()
        };

        let hits = query_vector_entities(&world, &q);
        let got: Vec<u32> = hits.iter_indices().collect();
        assert_eq!(got, vec![e1.index(), e2.index()]);
    }
}
