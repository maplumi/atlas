use foundation::time::Time;

/// Deterministic frame metadata.
///
/// This is the primary timebase for the engine runtime. It is intentionally
/// small and pure so it can be recorded and replayed.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Frame {
    /// 0-based frame index.
    pub index: u64,
    /// Fixed delta time (seconds).
    pub dt_s: f64,
    /// Engine time at the start of the frame (seconds).
    pub time: Time,
}

impl Frame {
    pub fn new(index: u64, dt_s: f64) -> Self {
        Self {
            index,
            dt_s,
            time: Time(index as f64 * dt_s),
        }
    }

    pub fn next(self) -> Self {
        Self::new(self.index + 1, self.dt_s)
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;
    use foundation::time::Time;

    #[test]
    fn frame_time_is_deterministic() {
        let a = Frame::new(10, 1.0 / 60.0);
        let b = Frame::new(10, 1.0 / 60.0);
        assert_eq!(a, b);
        assert_eq!(a.time, Time(10.0 / 60.0));
    }

    #[test]
    fn next_advances_index_and_time() {
        let f0 = Frame::new(0, 0.5);
        let f1 = f0.next();
        assert_eq!(f1.index, 1);
        assert_eq!(f1.time, Time(0.5));
    }
}
