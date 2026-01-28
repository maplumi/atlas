use runtime::budget::FrameBudget;
use runtime::work_queue::{WorkId, WorkQueue, WorkQueueFull};

use crate::request::Request;

/// Streaming work queue with deterministic ordering and backpressure.
///
/// This is a thin wrapper over `runtime::WorkQueue` so streaming can own its
/// scheduling policy without duplicating queue logic.
#[derive(Debug)]
pub struct StreamingQueue {
    inner: WorkQueue<Request>,
}

impl StreamingQueue {
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
        req: Request,
    ) -> Result<WorkId, WorkQueueFull> {
        self.inner.try_push_with_cost(priority, cost_units, req)
    }

    pub fn cancel(&mut self, id: WorkId) -> bool {
        self.inner.cancel(id)
    }

    pub fn pop_next_with_budget(&mut self, budget: &mut FrameBudget) -> Option<(WorkId, Request)> {
        let (id, _priority, req) = self.inner.pop_next_with_budget(budget)?;
        Some((id, req))
    }
}

#[cfg(test)]
mod tests {
    use super::StreamingQueue;
    use runtime::budget::FrameBudget;

    #[test]
    fn enforces_backpressure() {
        let mut q = StreamingQueue::new(1);
        assert!(q.try_submit(0, 1, super::Request(1)).is_ok());
        assert!(q.try_submit(0, 1, super::Request(2)).is_err());
    }

    #[test]
    fn respects_budget() {
        let mut q = StreamingQueue::new(10);
        q.try_submit(0, 2, super::Request(1)).unwrap();

        let mut budget = FrameBudget::new(1);
        assert!(q.pop_next_with_budget(&mut budget).is_none());
        assert_eq!(q.len(), 1);

        let mut budget = FrameBudget::new(2);
        assert!(q.pop_next_with_budget(&mut budget).is_some());
        assert_eq!(q.len(), 0);
    }
}
