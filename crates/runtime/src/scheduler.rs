pub struct Scheduler;
impl Scheduler {
    pub fn new() -> Self {
        Scheduler
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
