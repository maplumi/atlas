/// Time primitives
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Time(pub f64); // seconds

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TimeSpan {
    pub start: Time,
    pub end: Time,
}

impl TimeSpan {
    pub fn duration(&self) -> f64 {
        (self.end.0 - self.start.0).max(0.0)
    }
}
