use std::collections::BTreeMap;

/// Deterministic metrics aggregation.
///
/// Metrics must not depend on wall-clock time or unordered iteration.
/// This type uses sorted maps so snapshots have stable ordering.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Metrics {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, i64>,
    histograms: BTreeMap<String, Histogram>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Histogram {
    pub count: u64,
    pub sum: i64,
    pub min: i64,
    pub max: i64,
}

impl Histogram {
    pub fn record(&mut self, value: i64) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
        self.count += 1;
        self.sum += value;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub counters: Vec<(String, u64)>,
    pub gauges: Vec<(String, i64)>,
    pub histograms: Vec<(String, Histogram)>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.counters.clear();
        self.gauges.clear();
        self.histograms.clear();
    }

    pub fn counter(&self, name: &str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }

    pub fn inc_counter(&mut self, name: impl Into<String>, by: u64) {
        let name = name.into();
        *self.counters.entry(name).or_insert(0) += by;
    }

    pub fn gauge(&self, name: &str) -> Option<i64> {
        self.gauges.get(name).copied()
    }

    pub fn set_gauge(&mut self, name: impl Into<String>, value: i64) {
        self.gauges.insert(name.into(), value);
    }

    pub fn record_histogram(&mut self, name: impl Into<String>, value: i64) {
        self.histograms
            .entry(name.into())
            .or_default()
            .record(value);
    }

    pub fn histogram(&self, name: &str) -> Option<Histogram> {
        self.histograms.get(name).copied()
    }

    /// Returns a stable, sorted snapshot suitable for logs/debug UI.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            counters: self.counters.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            gauges: self.gauges.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            histograms: self
                .histograms
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Histogram, Metrics};

    #[test]
    fn counters_accumulate() {
        let mut m = Metrics::new();
        m.inc_counter("a", 1);
        m.inc_counter("a", 2);
        assert_eq!(m.counter("a"), 3);
        assert_eq!(m.counter("missing"), 0);
    }

    #[test]
    fn gauges_overwrite() {
        let mut m = Metrics::new();
        assert_eq!(m.gauge("g"), None);
        m.set_gauge("g", 10);
        m.set_gauge("g", 11);
        assert_eq!(m.gauge("g"), Some(11));
    }

    #[test]
    fn histogram_tracks_min_max_sum_count() {
        let mut h = Histogram::default();
        h.record(5);
        h.record(-2);
        h.record(7);
        assert_eq!(h.count, 3);
        assert_eq!(h.sum, 10);
        assert_eq!(h.min, -2);
        assert_eq!(h.max, 7);
    }

    #[test]
    fn snapshot_is_stably_sorted() {
        let mut m = Metrics::new();
        m.inc_counter("b", 1);
        m.inc_counter("a", 1);
        m.set_gauge("z", 1);
        m.set_gauge("m", 2);
        m.record_histogram("h2", 10);
        m.record_histogram("h1", 5);

        let snap = m.snapshot();
        assert_eq!(
            snap.counters,
            vec![("a".to_string(), 1), ("b".to_string(), 1)]
        );
        assert_eq!(
            snap.gauges,
            vec![("m".to_string(), 2), ("z".to_string(), 1)]
        );
        assert_eq!(snap.histograms.len(), 2);
        assert_eq!(snap.histograms[0].0, "h1".to_string());
        assert_eq!(snap.histograms[1].0, "h2".to_string());
    }
}
