use serde::{Deserialize, Serialize};

use crate::schema::GateStatus;

#[derive(Clone, Debug)]
pub struct NormalizedMetric {
    pub name: String,
    pub wjsm_numerator: Option<f64>,
    pub jdk_numerator: Option<f64>,
    pub denominator: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MetricGateResult {
    pub name: String,
    pub status: GateStatus,
    pub wjsm_value: Option<f64>,
    pub jdk_value: Option<f64>,
    pub ratio: Option<f64>,
    pub threshold: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GateReport {
    pub status: GateStatus,
    pub metrics: Vec<MetricGateResult>,
    pub leading_metrics: usize,
    pub reasons: Vec<String>,
}

pub fn evaluate_metric(metric: &NormalizedMetric) -> MetricGateResult {
    let Some(denominator) = metric.denominator.filter(|value| *value > 0.0) else {
        return unavailable(metric, "missing or zero denominator");
    };
    let Some(wjsm_numerator) = metric.wjsm_numerator else {
        return unavailable(metric, "missing WJSM numerator");
    };
    let Some(jdk_numerator) = metric.jdk_numerator else {
        return unavailable(metric, "missing JDK numerator");
    };
    let wjsm_value = wjsm_numerator / denominator;
    let jdk_value = jdk_numerator / denominator;
    if jdk_value == 0.0 {
        return unavailable(metric, "zero JDK normalized value");
    }
    let ratio = wjsm_value / jdk_value;
    MetricGateResult {
        name: metric.name.clone(),
        status: (ratio <= 1.10)
            .then_some(GateStatus::Passed)
            .unwrap_or(GateStatus::Failed),
        wjsm_value: Some(wjsm_value),
        jdk_value: Some(jdk_value),
        ratio: Some(ratio),
        threshold: 1.10,
    }
}

pub fn evaluate_gate(metrics: &[NormalizedMetric]) -> GateReport {
    let results: Vec<_> = metrics.iter().map(evaluate_metric).collect();
    let leading_metrics = results
        .iter()
        .filter(|result| result.ratio.is_some_and(|ratio| ratio <= 0.85))
        .count();
    let mut reasons = Vec::new();
    let status = if results
        .iter()
        .any(|result| result.status == GateStatus::NeedsVerification)
    {
        reasons.push("required normalized counters are missing".into());
        GateStatus::NeedsVerification
    } else if results
        .iter()
        .any(|result| result.status == GateStatus::Failed)
    {
        reasons.push("at least one metric exceeds the JDK × 1.10 hard gate".into());
        GateStatus::Failed
    } else if leading_metrics < 2 {
        reasons.push("fewer than two metrics beat JDK by at least 15%".into());
        GateStatus::Failed
    } else {
        GateStatus::Passed
    };
    GateReport {
        status,
        metrics: results,
        leading_metrics,
        reasons,
    }
}

fn unavailable(metric: &NormalizedMetric, _reason: &str) -> MetricGateResult {
    MetricGateResult {
        name: metric.name.clone(),
        status: GateStatus::NeedsVerification,
        wjsm_value: None,
        jdk_value: None,
        ratio: None,
        threshold: 1.10,
    }
}
