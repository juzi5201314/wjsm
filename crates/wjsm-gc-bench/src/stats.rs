use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DistributionSummary {
    pub count: usize,
    pub mean: Option<f64>,
    pub min: Option<u64>,
    pub p50: Option<u64>,
    pub p99: Option<u64>,
    pub max: Option<u64>,
}

pub fn summarize(samples: &[u64]) -> DistributionSummary {
    if samples.is_empty() {
        return DistributionSummary::default();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let sum: u128 = sorted.iter().map(|&value| u128::from(value)).sum();
    let mean = sum as f64 / sorted.len() as f64;
    DistributionSummary {
        count: sorted.len(),
        mean: Some(mean),
        min: sorted.first().copied(),
        p50: Some(quantile(&sorted, 0.50)),
        p99: Some(quantile(&sorted, 0.99)),
        max: sorted.last().copied(),
    }
}

fn quantile(samples: &[u64], quantile: f64) -> u64 {
    let index = ((samples.len() - 1) as f64 * quantile).ceil() as usize;
    samples[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_basic() {
        let samples = [10, 20, 30, 40, 50];
        let s = summarize(&samples);
        assert_eq!(s.count, 5);
        assert_eq!(s.min, Some(10));
        assert_eq!(s.max, Some(50));
        assert_eq!(s.p50, Some(30));
        assert_eq!(s.p99, Some(50));
        assert!((s.mean.unwrap() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn summary_empty() {
        let s = summarize(&[]);
        assert_eq!(s.count, 0);
        assert!(s.mean.is_none());
    }
}
