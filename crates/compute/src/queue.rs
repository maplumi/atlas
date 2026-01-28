use runtime::budget::FrameBudget;
use runtime::work_queue::{WorkId, WorkQueue, WorkQueueFull};

/// Compute work queue with deterministic ordering and backpressure.
///
/// For MVP, `T` is caller-defined; callers should pass a stable request payload.
#[derive(Debug)]
pub struct ComputeQueue<T> {
    inner: WorkQueue<T>,
}

impl<T> ComputeQueue<T> {
    pub fn new(max_pending: usize) -> Self {
        Self {
            inner: WorkQueue::with_max_len(max_pending),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn try_submit(
        &mut self,
        priority: i32,
        cost_units: u32,
        payload: T,
    ) -> Result<WorkId, WorkQueueFull> {
        self.inner.try_push_with_cost(priority, cost_units, payload)
    }

    pub fn cancel(&mut self, id: WorkId) -> bool {
        self.inner.cancel(id)
    }

    pub fn pop_next_with_budget(&mut self, budget: &mut FrameBudget) -> Option<(WorkId, T)> {
        let (id, _priority, payload) = self.inner.pop_next_with_budget(budget)?;
        Some((id, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::ComputeQueue;
    use runtime::budget::FrameBudget;

    #[test]
    fn compute_queue_backpressure_and_budgeting() {
        let mut q = ComputeQueue::new(1);
        assert!(q.try_submit(0, 2, "job").is_ok());
        assert!(q.try_submit(0, 1, "job2").is_err());

        let mut budget = FrameBudget::new(1);
        assert!(q.pop_next_with_budget(&mut budget).is_none());

        let mut budget = FrameBudget::new(2);
        let (_, v) = q.pop_next_with_budget(&mut budget).unwrap();
        assert_eq!(v, "job");
    }
}
