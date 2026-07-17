use serde::{Deserialize, Serialize};

use crate::schema::GateStatus;

#[derive(Clone, Debug)]
pub struct NormalizedMetric {
    pub name: String,
    pub wjsm_numerator: Option<f64>,
    pub wjsm_denominator: Option<f64>,
    pub jdk_numerator: Option<f64>,
    pub jdk_denominator: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MetricGateResult {
    pub name: String,
    pub status: GateStatus,
    pub wjsm_value: Option<f64>,
    pub jdk_value: Option<f64>,
    pub ratio: Option<f64>,
    pub threshold: f64,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PauseGateResult {
    pub scenario_hash: String,
    pub status: GateStatus,
    pub wjsm_p999_ns: Option<u64>,
    pub jdk_p999_ns: Option<u64>,
    pub wjsm_max_ns: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GateReport {
    pub status: GateStatus,
    pub metrics: Vec<MetricGateResult>,
    pub pauses: Vec<PauseGateResult>,
    pub leading_metrics: usize,
    pub reasons: Vec<String>,
}

pub fn evaluate_metric(metric: &NormalizedMetric) -> MetricGateResult {
    let Some(wjsm_value) = normalized(metric.wjsm_numerator, metric.wjsm_denominator) else {
        return unavailable_metric(metric, "缺少 WJSM numerator 或 denominator");
    };
    let Some(jdk_value) = normalized(metric.jdk_numerator, metric.jdk_denominator) else {
        return unavailable_metric(metric, "缺少 JDK numerator 或 denominator");
    };
    if jdk_value == 0.0 {
        return unavailable_metric(metric, "JDK normalized value 为零");
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
        reason: None,
    }
}

pub fn evaluate_pause(
    scenario_hash: String,
    wjsm_p999_ns: Option<u64>,
    jdk_p999_ns: Option<u64>,
    wjsm_max_ns: Option<u64>,
) -> PauseGateResult {
    let Some(wjsm_p999_ns) = wjsm_p999_ns else {
        return unavailable_pause(scenario_hash, "缺少 WJSM p99.9 pause");
    };
    let Some(jdk_p999_ns) = jdk_p999_ns else {
        return unavailable_pause(scenario_hash, "缺少 JDK p99.9 pause");
    };
    let Some(wjsm_max_ns) = wjsm_max_ns else {
        return unavailable_pause(scenario_hash, "缺少 WJSM max pause");
    };
    let reason = if wjsm_p999_ns > jdk_p999_ns {
        Some("WJSM p99.9 pause 高于 JDK".into())
    } else if wjsm_max_ns >= 1_000_000 {
        Some("WJSM max pause 不满足 <1ms hard gate".into())
    } else {
        None
    };
    PauseGateResult {
        scenario_hash,
        status: reason
            .is_none()
            .then_some(GateStatus::Passed)
            .unwrap_or(GateStatus::Failed),
        wjsm_p999_ns: Some(wjsm_p999_ns),
        jdk_p999_ns: Some(jdk_p999_ns),
        wjsm_max_ns: Some(wjsm_max_ns),
        reason,
    }
}

pub fn evaluate_gate(metrics: &[NormalizedMetric], pauses: &[PauseGateResult]) -> GateReport {
    let results: Vec<_> = metrics.iter().map(evaluate_metric).collect();
    let leading_metrics = results
        .iter()
        .filter(|result| result.ratio.is_some_and(|ratio| ratio <= 0.85))
        .count();
    let mut reasons = Vec::new();
    let status = if results
        .iter()
        .any(|result| result.status == GateStatus::NeedsVerification)
        || pauses
            .iter()
            .any(|result| result.status == GateStatus::NeedsVerification)
    {
        reasons.push("缺少 required normalized counter 或 pause distribution".into());
        GateStatus::NeedsVerification
    } else if results
        .iter()
        .any(|result| result.status == GateStatus::Failed)
        || pauses
            .iter()
            .any(|result| result.status == GateStatus::Failed)
    {
        reasons.push("至少一项 metric 或 pause hard gate 失败".into());
        GateStatus::Failed
    } else if leading_metrics < 2 {
        reasons.push("少于两项 metric 比 JDK 至少领先 15%".into());
        GateStatus::Failed
    } else {
        GateStatus::Passed
    };
    GateReport {
        status,
        metrics: results,
        pauses: pauses.to_vec(),
        leading_metrics,
        reasons,
    }
}

fn normalized(numerator: Option<f64>, denominator: Option<f64>) -> Option<f64> {
    numerator
        .zip(denominator.filter(|value| *value > 0.0))
        .map(|(top, bottom)| top / bottom)
}

fn unavailable_metric(metric: &NormalizedMetric, reason: &str) -> MetricGateResult {
    MetricGateResult {
        name: metric.name.clone(),
        status: GateStatus::NeedsVerification,
        wjsm_value: None,
        jdk_value: None,
        ratio: None,
        threshold: 1.10,
        reason: Some(reason.into()),
    }
}

fn unavailable_pause(scenario_hash: String, reason: &str) -> PauseGateResult {
    PauseGateResult {
        scenario_hash,
        status: GateStatus::NeedsVerification,
        wjsm_p999_ns: None,
        jdk_p999_ns: None,
        wjsm_max_ns: None,
        reason: Some(reason.into()),
    }
}
