/// Time primitives
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Time(pub f64); // seconds

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TimeSpan {
    pub start: Time,
    pub end: Time,
}

impl TimeSpan {
    pub fn forever() -> Self {
        Self {
            start: Time(f64::NEG_INFINITY),
            end: Time(f64::INFINITY),
        }
    }

    pub fn instant(t: Time) -> Self {
        Self { start: t, end: t }
    }

    pub fn duration(&self) -> f64 {
        (self.end.0 - self.start.0).max(0.0)
    }
}
