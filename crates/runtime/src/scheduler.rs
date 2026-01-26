use crate::event_bus::EventBus;
use crate::frame::Frame;
use crate::job::Job;

#[derive(Default)]
pub struct Scheduler {
    jobs: Vec<Job>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self { jobs: Vec::new() }
    }

    pub fn add_job(&mut self, job: Job) {
        self.jobs.push(job);
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Run all jobs in a deterministic order for the given frame.
    pub fn run_frame(&mut self, frame: Frame, bus: &mut EventBus) {
        self.jobs.sort_by(|a, b| a.id.cmp(b.id));
        for job in &self.jobs {
            (job.run)(frame, bus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scheduler;
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
}
