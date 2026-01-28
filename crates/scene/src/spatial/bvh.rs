use foundation::bounds::Aabb3;
use foundation::math::precision::stable_total_cmp_f64;

use crate::entity::EntityId;

/// A deterministic bounding volume hierarchy (BVH) over `Aabb3` items.
///
/// Ordering contract:
/// - `query_aabb` returns entities in ascending `EntityId::index()` order.
///
/// This is MVP-focused: correctness + determinism first; performance later.
#[derive(Debug, Clone)]
pub struct Bvh {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf {
        bounds: Aabb3,
        items: Vec<Item>,
    },
    Internal {
        bounds: Aabb3,
        left: usize,
        right: usize,
    },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Item {
    pub entity: EntityId,
    pub bounds: Aabb3,
}

impl Bvh {
    pub fn build(items: Vec<Item>) -> Self {
        let mut nodes = Vec::new();
        let mut items = items;
        if !items.is_empty() {
            let _root = build_node(&mut nodes, &mut items);
        }
        Self { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Query the BVH for items that intersect `query`.
    ///
    /// Returns entities in ascending `EntityId::index()` order.
    pub fn query_aabb(&self, query: &Aabb3) -> Vec<EntityId> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<EntityId> = Vec::new();
        let mut stack: Vec<usize> = vec![0];

        while let Some(idx) = stack.pop() {
            match &self.nodes[idx] {
                Node::Leaf { bounds, items } => {
                    if !bounds.intersects(query) {
                        continue;
                    }
                    for item in items {
                        if item.bounds.intersects(query) {
                            hits.push(item.entity);
                        }
                    }
                }
                Node::Internal {
                    bounds,
                    left,
                    right,
                } => {
                    if !bounds.intersects(query) {
                        continue;
                    }
                    // Stack order doesn't matter because we sort output, but keep it stable.
                    stack.push(*right);
                    stack.push(*left);
                }
            }
        }

        hits.sort_by_key(|e| e.index());
        hits.dedup();
        hits
    }

    /// Query the BVH for items whose bounds intersect a ray.
    ///
    /// `origin` and `dir` are in the same world coordinate system as the stored bounds.
    ///
    /// Returns entities in ascending `EntityId::index()` order.
    pub fn query_ray(
        &self,
        origin: [f64; 3],
        dir: [f64; 3],
        t_min: f64,
        t_max: f64,
    ) -> Vec<EntityId> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<EntityId> = Vec::new();
        let mut stack: Vec<usize> = vec![0];

        while let Some(idx) = stack.pop() {
            match &self.nodes[idx] {
                Node::Leaf { bounds, items } => {
                    if !ray_intersects_aabb(origin, dir, bounds, t_min, t_max) {
                        continue;
                    }
                    for item in items {
                        if ray_intersects_aabb(origin, dir, &item.bounds, t_min, t_max) {
                            hits.push(item.entity);
                        }
                    }
                }
                Node::Internal {
                    bounds,
                    left,
                    right,
                } => {
                    if !ray_intersects_aabb(origin, dir, bounds, t_min, t_max) {
                        continue;
                    }
                    // Stack order doesn't matter because we sort output, but keep it stable.
                    stack.push(*right);
                    stack.push(*left);
                }
            }
        }

        hits.sort_by_key(|e| e.index());
        hits.dedup();
        hits
    }
}

const LEAF_MAX: usize = 8;

fn build_node(nodes: &mut Vec<Node>, items: &mut [Item]) -> usize {
    if items.len() <= LEAF_MAX {
        let bounds = bounds_for_items(items);
        let leaf_items = items.to_vec();
        let idx = nodes.len();
        nodes.push(Node::Leaf {
            bounds,
            items: leaf_items,
        });
        return idx;
    }

    let bounds = bounds_for_items(items);
    let axis = split_axis(&bounds);

    items.sort_by(|a, b| {
        let ca = centroid_axis(&a.bounds, axis);
        let cb = centroid_axis(&b.bounds, axis);
        stable_total_cmp_f64(ca, cb).then_with(|| a.entity.index().cmp(&b.entity.index()))
    });

    let mid = items.len() / 2;
    let (left_items, right_items) = items.split_at_mut(mid);

    let idx = nodes.len();
    // Placeholder; will patch after children are built.
    nodes.push(Node::Leaf {
        bounds,
        items: Vec::new(),
    });

    let left = build_node(nodes, left_items);
    let right = build_node(nodes, right_items);

    nodes[idx] = Node::Internal {
        bounds,
        left,
        right,
    };
    idx
}

fn centroid_axis(aabb: &Aabb3, axis: usize) -> f64 {
    (aabb.min[axis] + aabb.max[axis]) * 0.5
}

fn split_axis(bounds: &Aabb3) -> usize {
    let ex = bounds.max[0] - bounds.min[0];
    let ey = bounds.max[1] - bounds.min[1];
    let ez = bounds.max[2] - bounds.min[2];

    // Deterministic tie-break: prefer X, then Y, then Z.
    if ex >= ey && ex >= ez {
        0
    } else if ey >= ez {
        1
    } else {
        2
    }
}

fn bounds_for_items(items: &[Item]) -> Aabb3 {
    let mut b = items[0].bounds;
    for item in &items[1..] {
        b = union_aabb3(&b, &item.bounds);
    }
    b
}

fn union_aabb3(a: &Aabb3, b: &Aabb3) -> Aabb3 {
    Aabb3::new(
        [
            a.min[0].min(b.min[0]),
            a.min[1].min(b.min[1]),
            a.min[2].min(b.min[2]),
        ],
        [
            a.max[0].max(b.max[0]),
            a.max[1].max(b.max[1]),
            a.max[2].max(b.max[2]),
        ],
    )
}

fn ray_intersects_aabb(
    origin: [f64; 3],
    dir: [f64; 3],
    aabb: &Aabb3,
    mut t_min: f64,
    mut t_max: f64,
) -> bool {
    // Slabs intersection; deterministic math only.
    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        let min = aabb.min[axis];
        let max = aabb.max[axis];

        if d.abs() < 1e-12 {
            if o < min || o > max {
                return false;
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
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{Bvh, Item};
    use crate::entity::EntityId;
    use foundation::bounds::Aabb3;
    use foundation::handles::Handle;

    fn e(idx: u32) -> EntityId {
        EntityId(Handle::new(idx, 0))
    }

    #[test]
    fn query_returns_entities_in_index_order() {
        let items = vec![
            Item {
                entity: e(2),
                bounds: Aabb3::new([10.0, 0.0, 0.0], [11.0, 1.0, 1.0]),
            },
            Item {
                entity: e(1),
                bounds: Aabb3::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            },
            Item {
                entity: e(3),
                bounds: Aabb3::new([0.5, 0.5, 0.5], [2.0, 2.0, 2.0]),
            },
        ];
        let bvh = Bvh::build(items);

        let hits = bvh.query_aabb(&Aabb3::new([0.25, 0.25, 0.25], [1.5, 1.5, 1.5]));
        assert_eq!(hits, vec![e(1), e(3)]);
    }

    #[test]
    fn build_is_input_order_independent_for_results() {
        let a = vec![
            Item {
                entity: e(1),
                bounds: Aabb3::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            },
            Item {
                entity: e(2),
                bounds: Aabb3::new([2.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
            },
            Item {
                entity: e(3),
                bounds: Aabb3::new([4.0, 0.0, 0.0], [5.0, 1.0, 1.0]),
            },
        ];
        let mut b = a.clone();
        b.reverse();

        let q = Aabb3::new([1.5, 0.0, 0.0], [4.5, 1.0, 1.0]);
        let ha = Bvh::build(a).query_aabb(&q);
        let hb = Bvh::build(b).query_aabb(&q);
        assert_eq!(ha, hb);
        assert_eq!(ha, vec![e(2), e(3)]);
    }
}
