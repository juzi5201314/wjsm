use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DistributionSummary {
    pub count: usize,
    pub mean: Option<f64>,
    pub min: Option<u64>,
    pub p50: Option<u64>,
    pub p99: Option<u64>,
    pub max: Option<u64>,
    pub ci99_low: Option<f64>,
    pub ci99_high: Option<f64>,
}

pub fn summarize(samples: &[u64]) -> DistributionSummary {
    if samples.is_empty() {
        return DistributionSummary::default();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let sum: u128 = sorted.iter().map(|&value| u128::from(value)).sum();
    let mean = sum as f64 / sorted.len() as f64;
    let (ci99_low, ci99_high) = bootstrap_mean_ci_99(&sorted);
    DistributionSummary {
        count: sorted.len(),
        mean: Some(mean),
        min: sorted.first().copied(),
        p50: Some(quantile(&sorted, 0.50)),
        p99: Some(quantile(&sorted, 0.99)),
        max: sorted.last().copied(),
        ci99_low: Some(ci99_low),
        ci99_high: Some(ci99_high),
    }
}

fn quantile(samples: &[u64], quantile: f64) -> u64 {
    let index = ((samples.len() - 1) as f64 * quantile).ceil() as usize;
    samples[index]
}

/// 固定种子的 bootstrap 均值 99% CI，避免报告随进程随机源漂移。
fn bootstrap_mean_ci_99(samples: &[u64]) -> (f64, f64) {
    const RESAMPLES: usize = 20_000;
    let mut state = 0x2a4b_9c1d_e5f6_8071_u64;
    let mut means = Vec::with_capacity(RESAMPLES);
    for _ in 0..RESAMPLES {
        let mut sum = 0_u128;
        for _ in samples {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = ((state >> 32) as usize) % samples.len();
            sum += u128::from(samples[index]);
        }
        means.push(sum as f64 / samples.len() as f64);
    }
    means.sort_by(f64::total_cmp);
    let low = means[(RESAMPLES as f64 * 0.005).floor() as usize];
    let high = means[(RESAMPLES as f64 * 0.995).ceil() as usize - 1];
    (low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn summary_is_deterministic() {
        let samples = [1, 2, 3, 4, 5];
        assert_eq!(summarize(&samples).p99, Some(5));
        assert_eq!(summarize(&samples).count, 5);
    }
}
