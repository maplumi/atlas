use crate::frame::Frame;

/// Minimal event type for traceability.
///
/// For now this is just structured text; as the engine evolves this can become
/// a stable, serializable event enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub frame_index: u64,
    pub kind: &'static str,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct EventBus {
    events: Vec<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn emit(&mut self, frame: Frame, kind: &'static str, message: impl Into<String>) {
        self.events.push(Event {
            frame_index: frame.index,
            kind,
            message: message.into(),
        });
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn drain(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::EventBus;
    use crate::frame::Frame;

    #[test]
    fn records_events_with_frame_index() {
        let mut bus = EventBus::new();
        let f = Frame::new(2, 0.1);
        bus.emit(f, "test", "hello");
        assert_eq!(bus.events().len(), 1);
        assert_eq!(bus.events()[0].frame_index, 2);
    }

    #[test]
    fn drain_clears_events() {
        let mut bus = EventBus::new();
        bus.emit(Frame::new(0, 1.0), "k", "m");
        let drained = bus.drain();
        assert_eq!(drained.len(), 1);
        assert!(bus.events().is_empty());
    }
}
