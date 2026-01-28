use crate::event_bus::EventBus;
use crate::frame::Frame;

/// A deterministic unit of work executed by the [`Scheduler`].
///
/// Jobs are run in a stable order based on their `(priority, id)`.
pub struct Job {
    pub id: &'static str,
    /// Smaller values run earlier.
    pub priority: i32,
    /// Abstract cost used by frame budgeting.
    pub cost_units: u32,
    pub run: fn(frame: Frame, bus: &mut EventBus),
}

impl Job {
    pub fn new(id: &'static str, run: fn(frame: Frame, bus: &mut EventBus)) -> Self {
        Self {
            id,
            priority: 0,
            cost_units: 1,
            run,
        }
    }

    pub fn with_priority(
        id: &'static str,
        priority: i32,
        run: fn(frame: Frame, bus: &mut EventBus),
    ) -> Self {
        Self {
            id,
            priority,
            cost_units: 1,
            run,
        }
    }

    pub fn with_cost(
        id: &'static str,
        cost_units: u32,
        run: fn(frame: Frame, bus: &mut EventBus),
    ) -> Self {
        Self {
            id,
            priority: 0,
            cost_units,
            run,
        }
    }

    pub fn with_priority_and_cost(
        id: &'static str,
        priority: i32,
        cost_units: u32,
        run: fn(frame: Frame, bus: &mut EventBus),
    ) -> Self {
        Self {
            id,
            priority,
            cost_units,
            run,
        }
    }
}
