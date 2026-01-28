use std::collections::BTreeMap;

use runtime::budget::FrameBudget;
use runtime::work_queue::{WorkId, WorkQueueFull};

use crate::cache::{Cache, CacheKey, MemoryBudget};
use crate::queue::StreamingQueue;
use crate::request::Request;

/// High-level streaming orchestration for requests + cache.
///
/// This is intentionally small and deterministic: queue ordering is handled by
/// `runtime::WorkQueue`, while `Cache` provides explicit residency + budgeting.
#[derive(Debug)]
pub struct Pipeline {
    cache: Cache,
    queue: StreamingQueue,
    pending: BTreeMap<Request, WorkId>,
}

impl Pipeline {
    pub fn new(cache_budget: MemoryBudget, max_pending: usize) -> Self {
        Self {
            cache: Cache::new(cache_budget),
            queue: StreamingQueue::new(max_pending),
            pending: BTreeMap::new(),
        }
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut Cache {
        &mut self.cache
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Submit a cache-backed request onto the deterministic streaming queue.
    pub fn submit(
        &mut self,
        key: CacheKey,
        priority: i32,
        cost_units: u32,
    ) -> Result<Request, WorkQueueFull> {
        let req = self.cache.request(key);
        let work_id = self.queue.try_submit(priority, cost_units, req)?;
        self.pending.insert(req, work_id);
        Ok(req)
    }

    /// Cancel a previously submitted request.
    ///
    /// Returns `true` if the request was still pending and was cancelled.
    pub fn cancel(&mut self, req: Request) -> bool {
        if let Some(work_id) = self.pending.remove(&req) {
            return self.queue.cancel(work_id);
        }
        false
    }

    pub fn pop_next_with_budget(
        &mut self,
        budget: &mut FrameBudget,
    ) -> Option<(Request, CacheKey)> {
        let (_work_id, req) = self.queue.pop_next_with_budget(budget)?;
        self.pending.remove(&req);
        let key = self.cache.key_for_request(req)?.clone();
        Some((req, key))
    }
}

#[cfg(test)]
mod tests {
    use super::Pipeline;
    use crate::cache::{CacheKey, MemoryBudget};
    use runtime::budget::FrameBudget;

    #[test]
    fn pipeline_cancel_removes_work() {
        let mut p = Pipeline::new(MemoryBudget::new(1024), 10);
        let req = p.submit(CacheKey::new("ds", "a"), 0, 1).expect("submit");
        assert_eq!(p.queue_len(), 1);
        assert!(p.cancel(req));
        assert_eq!(p.queue_len(), 0);

        let mut budget = FrameBudget::new(10);
        assert!(p.pop_next_with_budget(&mut budget).is_none());
    }

    #[test]
    fn pipeline_pop_returns_key() {
        let mut p = Pipeline::new(MemoryBudget::new(1024), 10);
        let _ = p
            .submit(CacheKey::new("ds", "cities"), 0, 1)
            .expect("submit");

        let mut budget = FrameBudget::new(10);
        let (_req, key) = p.pop_next_with_budget(&mut budget).expect("pop");
        assert_eq!(key.resource_id, "cities");
    }
}
