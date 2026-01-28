/// Deterministic work queue for streaming/compute style task scheduling.
///
/// Key properties:
/// - Total ordering on `(priority, id)`.
/// - Equal priorities are processed in insertion order.
/// - Cancellation does not perturb the order of remaining items.
/// - Optional backpressure via a deterministic maximum pending length.
/// - Optional frame budgeting via abstract work units.
///
/// This is intentionally simple (Vec-backed) because MVP correctness and
/// determinism are more important than asymptotic performance.

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub u64);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct WorkQueueFull {
    pub max_len: usize,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct Key {
    // Smaller values run earlier.
    priority: i32,
    id: WorkId,
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Total ordering: (priority, id)
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct Item<T> {
    key: Key,
    payload: T,
    cost_units: u32,
    canceled: bool,
}

#[derive(Debug)]
pub struct WorkQueue<T> {
    next_id: u64,
    items: Vec<Item<T>>,
    max_len: Option<usize>,
}

impl<T> Default for WorkQueue<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            items: Vec::new(),
            max_len: None,
        }
    }
}

impl<T> WorkQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_len(max_len: usize) -> Self {
        Self {
            max_len: Some(max_len),
            ..Self::default()
        }
    }

    pub fn max_len(&self) -> Option<usize> {
        self.max_len
    }

    pub fn len(&self) -> usize {
        self.items.iter().filter(|i| !i.canceled).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, priority: i32, payload: T) -> WorkId {
        self.push_unchecked_with_cost(priority, 1, payload)
    }

    pub fn try_push(&mut self, priority: i32, payload: T) -> Result<WorkId, WorkQueueFull> {
        self.try_push_with_cost(priority, 1, payload)
    }

    pub fn push_with_cost(&mut self, priority: i32, cost_units: u32, payload: T) -> WorkId {
        self.push_unchecked_with_cost(priority, cost_units, payload)
    }

    fn push_unchecked_with_cost(&mut self, priority: i32, cost_units: u32, payload: T) -> WorkId {
        let id = WorkId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.items.push(Item {
            key: Key { priority, id },
            payload,
            cost_units,
            canceled: false,
        });
        id
    }

    pub fn try_push_with_cost(
        &mut self,
        priority: i32,
        cost_units: u32,
        payload: T,
    ) -> Result<WorkId, WorkQueueFull> {
        if let Some(max_len) = self.max_len
            && self.len() >= max_len
        {
            return Err(WorkQueueFull { max_len });
        }

        Ok(self.push_unchecked_with_cost(priority, cost_units, payload))
    }

    pub fn cancel(&mut self, id: WorkId) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.key.id == id) {
            item.canceled = true;
            return true;
        }
        false
    }

    /// Pops the next (highest priority, then oldest) item.
    pub fn pop_next(&mut self) -> Option<(WorkId, i32, T)> {
        let mut best_idx: Option<usize> = None;
        for (idx, item) in self.items.iter().enumerate() {
            if item.canceled {
                continue;
            }
            match best_idx {
                None => best_idx = Some(idx),
                Some(best) => {
                    if item.key < self.items[best].key {
                        best_idx = Some(idx);
                    }
                }
            }
        }

        let idx = best_idx?;
        let item = self.items.swap_remove(idx);
        Some((item.key.id, item.key.priority, item.payload))
    }

    /// Pops the next item, but only if the budget can cover its cost.
    ///
    /// Budgeting uses deterministic abstract work units.
    ///
    /// Important: if the next item is too expensive, this returns `None` without
    /// searching for cheaper items. This keeps behavior predictable and aligned
    /// with priority ordering.
    pub fn pop_next_with_budget(
        &mut self,
        budget: &mut crate::budget::FrameBudget,
    ) -> Option<(WorkId, i32, T)> {
        let mut best_idx: Option<usize> = None;
        for (idx, item) in self.items.iter().enumerate() {
            if item.canceled {
                continue;
            }
            match best_idx {
                None => best_idx = Some(idx),
                Some(best) => {
                    if item.key < self.items[best].key {
                        best_idx = Some(idx);
                    }
                }
            }
        }

        let idx = best_idx?;
        let cost_units = self.items[idx].cost_units;
        if !budget.try_consume(cost_units) {
            return None;
        }

        let item = self.items.swap_remove(idx);
        Some((item.key.id, item.key.priority, item.payload))
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkQueue, WorkQueueFull};
    use crate::budget::FrameBudget;

    #[test]
    fn same_priority_is_insertion_order() {
        let mut q = WorkQueue::new();
        q.push(0, "a");
        q.push(0, "b");
        q.push(0, "c");

        let (_, _, a) = q.pop_next().unwrap();
        let (_, _, b) = q.pop_next().unwrap();
        let (_, _, c) = q.pop_next().unwrap();
        assert_eq!((a, b, c), ("a", "b", "c"));
    }

    #[test]
    fn lower_priority_value_runs_first() {
        let mut q = WorkQueue::new();
        q.push(10, "late");
        q.push(-1, "early");
        let (_, _, v) = q.pop_next().unwrap();
        assert_eq!(v, "early");
    }

    #[test]
    fn cancel_skips_item() {
        let mut q = WorkQueue::new();
        let a = q.push(0, "a");
        q.push(0, "b");
        assert!(q.cancel(a));

        let (_, _, v) = q.pop_next().unwrap();
        assert_eq!(v, "b");
        assert!(q.pop_next().is_none());
    }

    #[test]
    fn backpressure_rejects_when_full() {
        let mut q = WorkQueue::with_max_len(2);
        assert!(q.try_push(0, "a").is_ok());
        assert!(q.try_push(0, "b").is_ok());

        let err = q.try_push(0, "c").unwrap_err();
        assert_eq!(err, WorkQueueFull { max_len: 2 });
    }

    #[test]
    fn pop_respects_budget_units() {
        let mut q = WorkQueue::new();
        q.push_with_cost(0, 2, "expensive");

        let mut budget = FrameBudget::new(1);
        assert!(q.pop_next_with_budget(&mut budget).is_none());
        assert_eq!(q.len(), 1);

        let mut budget = FrameBudget::new(2);
        let (_, _, v) = q.pop_next_with_budget(&mut budget).unwrap();
        assert_eq!(v, "expensive");
        assert!(q.is_empty());
    }
}
