/// Deterministic frame budgeting for time-slicing work.
///
/// Budgets are expressed in abstract "work units" rather than wall-clock time.
/// This keeps scheduling deterministic and replayable.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FrameBudget {
    remaining_units: u32,
}

impl FrameBudget {
    pub fn new(units: u32) -> Self {
        Self {
            remaining_units: units,
        }
    }

    /// A practically-unbounded budget (still deterministic).
    pub fn unlimited() -> Self {
        Self {
            remaining_units: u32::MAX,
        }
    }

    pub fn remaining_units(&self) -> u32 {
        self.remaining_units
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining_units == 0
    }

    pub fn can_consume(&self, units: u32) -> bool {
        self.remaining_units >= units
    }

    /// Attempts to consume `units` from the budget.
    ///
    /// Returns `true` if the budget had enough remaining units.
    pub fn try_consume(&mut self, units: u32) -> bool {
        if self.remaining_units < units {
            return false;
        }
        self.remaining_units -= units;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::FrameBudget;

    #[test]
    fn consumes_units() {
        let mut b = FrameBudget::new(3);
        assert!(b.try_consume(2));
        assert_eq!(b.remaining_units(), 1);
        assert!(!b.try_consume(2));
        assert_eq!(b.remaining_units(), 1);
        assert!(b.try_consume(1));
        assert!(b.is_exhausted());
    }
}
