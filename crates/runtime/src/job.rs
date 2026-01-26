use crate::event_bus::EventBus;
use crate::frame::Frame;

/// A deterministic unit of work executed by the [`Scheduler`].
///
/// Jobs are run in a stable order based on their `id`.
pub struct Job {
    pub id: &'static str,
    pub run: fn(frame: Frame, bus: &mut EventBus),
}

impl Job {
    pub fn new(id: &'static str, run: fn(frame: Frame, bus: &mut EventBus)) -> Self {
        Self { id, run }
    }
}
