use crate::budget::FrameBudget;
use crate::event_bus::EventBus;
use crate::frame::Frame;
use crate::job::Job;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BudgetRunSummary {
    pub ran_jobs: usize,
    pub skipped_jobs: usize,
}

#[derive(Default)]
pub struct Scheduler {
    next_order: u64,
    jobs: Vec<(u64, Job)>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            next_order: 0,
            jobs: Vec::new(),
        }
    }

    pub fn add_job(&mut self, job: Job) {
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.jobs.push((order, job));
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Run all jobs in a deterministic order for the given frame.
    pub fn run_frame(&mut self, frame: Frame, bus: &mut EventBus) {
        let mut budget = FrameBudget::unlimited();
        let _ = self.run_frame_with_budget(frame, bus, &mut budget);
    }

    /// Run jobs for the given frame, stopping when the budget is exhausted.
    ///
    /// Ordering is deterministic and prioritization is respected:
    /// `(priority, id, insertion_order)`.
    pub fn run_frame_with_budget(
        &mut self,
        frame: Frame,
        bus: &mut EventBus,
        budget: &mut FrameBudget,
    ) -> BudgetRunSummary {
        // Total ordering: (priority, id, insertion_order). This stays deterministic even if
        // callers accidentally register duplicate job ids.
        self.jobs.sort_by(|(oa, a), (ob, b)| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.id.cmp(b.id))
                .then_with(|| oa.cmp(ob))
        });

        let mut ran = 0usize;
        for (_order, job) in &self.jobs {
            if !budget.try_consume(job.cost_units) {
                break;
            }
            ran += 1;
            (job.run)(frame, bus);
        }

        BudgetRunSummary {
            ran_jobs: ran,
            skipped_jobs: self.jobs.len().saturating_sub(ran),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scheduler;
    use crate::budget::FrameBudget;
    use crate::event_bus::EventBus;
    use crate::frame::Frame;
    use crate::job::Job;

    fn job_a(frame: Frame, bus: &mut EventBus) {
        bus.emit(frame, "job", "a");
    }

    fn job_b(frame: Frame, bus: &mut EventBus) {
        bus.emit(frame, "job", "b");
    }

    #[test]
    fn runs_jobs_in_stable_id_order() {
        let mut sched = Scheduler::new();
        sched.add_job(Job::new("b", job_b));
        sched.add_job(Job::new("a", job_a));

        let mut bus = EventBus::new();
        sched.run_frame(Frame::new(0, 1.0), &mut bus);
        let msgs: Vec<_> = bus.events().iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a", "b"]);
    }

    #[test]
    fn runs_duplicate_ids_in_insertion_order() {
        let mut sched = Scheduler::new();
        sched.add_job(Job::new("a", job_a));
        sched.add_job(Job::new("a", job_b));

        let mut bus = EventBus::new();
        sched.run_frame(Frame::new(0, 1.0), &mut bus);
        let msgs: Vec<_> = bus.events().iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a", "b"]);
    }

    #[test]
    fn runs_higher_priority_first() {
        let mut sched = Scheduler::new();
        sched.add_job(Job::with_priority("a", 10, job_a));
        sched.add_job(Job::with_priority("b", -1, job_b));

        let mut bus = EventBus::new();
        sched.run_frame(Frame::new(0, 1.0), &mut bus);
        let msgs: Vec<_> = bus.events().iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["b", "a"]);
    }

    #[test]
    fn respects_frame_budget() {
        let mut sched = Scheduler::new();
        sched.add_job(Job::with_cost("a", 1, job_a));
        sched.add_job(Job::with_cost("b", 1, job_b));

        let mut bus = EventBus::new();
        let mut budget = FrameBudget::new(1);
        let summary = sched.run_frame_with_budget(Frame::new(0, 1.0), &mut bus, &mut budget);
        let msgs: Vec<_> = bus.events().iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a"]);
        assert_eq!(summary.ran_jobs, 1);
        assert_eq!(summary.skipped_jobs, 1);
    }
}
