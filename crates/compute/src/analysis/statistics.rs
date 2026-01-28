pub struct Statistics;

impl Statistics {
    pub fn mean(values: &[f64]) -> Option<f64> {
        if values.is_empty() {
            return None;
        }
        let mut sum = 0.0;
        for &v in values {
            sum += v;
        }
        Some(sum / values.len() as f64)
    }

    pub fn min_max(values: &[f64]) -> Option<(f64, f64)> {
        let first = *values.first()?;
        let mut min = first;
        let mut max = first;
        for &v in values.iter().skip(1) {
            min = min.min(v);
            max = max.max(v);
        }
        Some((min, max))
    }
}

#[cfg(test)]
mod tests {
    use super::Statistics;

    #[test]
    fn mean_works() {
        let m = Statistics::mean(&[1.0, 2.0, 3.0]).unwrap();
        assert!((m - 2.0).abs() < 1e-9);
    }
}
