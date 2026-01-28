use foundation::math::precision::stable_total_cmp_f64;
use foundation::time::{Time, TimeSpan};

use crate::entity::EntityId;

/// A deterministic interval tree for time-span membership queries.
///
/// Ordering contract:
/// - `query_at_time` and `query_overlaps` return entities in ascending `EntityId::index()` order.
///
/// This is MVP-focused: correctness + determinism first; performance later.
#[derive(Debug, Clone)]
pub struct IntervalTree {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
struct Node {
    center: f64,
    items: Vec<IntervalItem>,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct IntervalItem {
    pub entity: EntityId,
    pub span: TimeSpan,
}

impl IntervalTree {
    pub fn build(items: Vec<IntervalItem>) -> Self {
        let mut nodes = Vec::new();
        if !items.is_empty() {
            let _ = build_node(&mut nodes, items);
        }
        Self { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns all entities active at `time`.
    pub fn query_at_time(&self, time: Time) -> Vec<EntityId> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<EntityId> = Vec::new();
        query_time(&self.nodes, 0, time.0, &mut hits);

        hits.sort_by_key(|e| e.index());
        hits.dedup();
        hits
    }

    /// Returns all entities with spans overlapping `span`.
    pub fn query_overlaps(&self, span: TimeSpan) -> Vec<EntityId> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<EntityId> = Vec::new();
        query_overlaps(&self.nodes, 0, span, &mut hits);

        hits.sort_by_key(|e| e.index());
        hits.dedup();
        hits
    }
}

fn build_node(nodes: &mut Vec<Node>, items: Vec<IntervalItem>) -> usize {
    let center = choose_center(&items);

    let mut left_items: Vec<IntervalItem> = Vec::new();
    let mut right_items: Vec<IntervalItem> = Vec::new();
    let mut here: Vec<IntervalItem> = Vec::new();

    for item in items {
        if item.span.end.0 < center {
            left_items.push(item);
        } else if item.span.start.0 > center {
            right_items.push(item);
        } else {
            here.push(item);
        }
    }

    // Stable ordering for deterministic traversal.
    here.sort_by(|a, b| {
        stable_total_cmp_f64(a.span.start.0, b.span.start.0)
            .then_with(|| stable_total_cmp_f64(a.span.end.0, b.span.end.0))
            .then_with(|| a.entity.index().cmp(&b.entity.index()))
    });

    let idx = nodes.len();
    nodes.push(Node {
        center,
        items: here,
        left: None,
        right: None,
    });

    if !left_items.is_empty() {
        let child = build_node(nodes, left_items);
        nodes[idx].left = Some(child);
    }
    if !right_items.is_empty() {
        let child = build_node(nodes, right_items);
        nodes[idx].right = Some(child);
    }

    idx
}

fn choose_center(items: &[IntervalItem]) -> f64 {
    let mut endpoints: Vec<f64> = Vec::with_capacity(items.len() * 2);
    for item in items {
        endpoints.push(item.span.start.0);
        endpoints.push(item.span.end.0);
    }
    endpoints.sort_by(|a, b| stable_total_cmp_f64(*a, *b));
    endpoints[endpoints.len() / 2]
}

fn contains_time(span: TimeSpan, t: f64) -> bool {
    // Treat endpoints as inclusive.
    t >= span.start.0 && t <= span.end.0
}

fn overlaps(a: TimeSpan, b: TimeSpan) -> bool {
    !(a.end.0 < b.start.0 || a.start.0 > b.end.0)
}

fn query_time(nodes: &[Node], idx: usize, t: f64, out: &mut Vec<EntityId>) {
    let node = &nodes[idx];

    for item in &node.items {
        if contains_time(item.span, t) {
            out.push(item.entity);
        }
    }

    if t < node.center {
        if let Some(left) = node.left {
            query_time(nodes, left, t, out);
        }
    } else if let Some(right) = node.right {
        query_time(nodes, right, t, out);
    }
}

fn query_overlaps(nodes: &[Node], idx: usize, span: TimeSpan, out: &mut Vec<EntityId>) {
    let node = &nodes[idx];

    for item in &node.items {
        if overlaps(item.span, span) {
            out.push(item.entity);
        }
    }

    if span.start.0 < node.center
        && let Some(left) = node.left
    {
        query_overlaps(nodes, left, span, out);
    }
    if span.end.0 > node.center
        && let Some(right) = node.right
    {
        query_overlaps(nodes, right, span, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{IntervalItem, IntervalTree};
    use crate::entity::EntityId;
    use foundation::handles::Handle;
    use foundation::time::{Time, TimeSpan};

    fn e(idx: u32) -> EntityId {
        EntityId(Handle::new(idx, 0))
    }

    fn span(a: f64, b: f64) -> TimeSpan {
        TimeSpan {
            start: Time(a),
            end: Time(b),
        }
    }

    #[test]
    fn query_at_time_returns_sorted_entities() {
        let items = vec![
            IntervalItem {
                entity: e(3),
                span: span(0.0, 10.0),
            },
            IntervalItem {
                entity: e(1),
                span: span(5.0, 6.0),
            },
            IntervalItem {
                entity: e(2),
                span: span(-1.0, 1.0),
            },
        ];
        let tree = IntervalTree::build(items);

        let hits = tree.query_at_time(Time(5.5));
        assert_eq!(hits, vec![e(1), e(3)]);
    }

    #[test]
    fn build_is_input_order_independent_for_results() {
        let a = vec![
            IntervalItem {
                entity: e(1),
                span: span(0.0, 1.0),
            },
            IntervalItem {
                entity: e(2),
                span: span(2.0, 3.0),
            },
            IntervalItem {
                entity: e(3),
                span: span(4.0, 5.0),
            },
        ];
        let mut b = a.clone();
        b.reverse();

        let ha = IntervalTree::build(a).query_overlaps(span(2.5, 4.5));
        let hb = IntervalTree::build(b).query_overlaps(span(2.5, 4.5));
        assert_eq!(ha, hb);
        assert_eq!(ha, vec![e(2), e(3)]);
    }
}
