use foundation::time::{Time, TimeSpan};

pub struct TemporalAnalysis;

impl TemporalAnalysis {
    pub fn contains(span: TimeSpan, t: Time) -> bool {
        t.0 >= span.start.0 && t.0 <= span.end.0
    }

    pub fn intersects(a: TimeSpan, b: TimeSpan) -> bool {
        !(a.end.0 < b.start.0 || b.end.0 < a.start.0)
    }
}

#[cfg(test)]
mod tests {
    use super::TemporalAnalysis;
    use foundation::time::{Time, TimeSpan};

    #[test]
    fn intersects_overlaps() {
        let a = TimeSpan {
            start: Time(0.0),
            end: Time(10.0),
        };
        let b = TimeSpan {
            start: Time(9.0),
            end: Time(11.0),
        };
        assert!(TemporalAnalysis::intersects(a, b));
    }
}
