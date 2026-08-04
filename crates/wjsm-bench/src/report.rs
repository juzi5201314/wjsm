//! 报告 schema：serde 结构 + JSON 写盘。

use serde::Serialize;
use std::collections::BTreeMap;

use crate::env::EnvironmentSnapshot;

pub const BENCHMARK_SCHEMA_VERSION: u32 = 1;

/// 完整基准报告。BTreeMap 保证 JSON 键序稳定（default 在 wjsm_cold 前）。
#[derive(Clone, Debug, Serialize)]
pub struct BenchReport {
    pub schema_version: u32,
    /// 手写 RFC 3339（UTC），不引入 chrono。
    pub created_at: String,
    pub git_rev: Option<String>,
    pub config: BenchConfig,
    pub environment: EnvironmentSnapshot,
    /// "default" / "wjsm_cold"。
    pub regimes: BTreeMap<String, RegimeReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchConfig {
    pub scenarios: Vec<String>,
    pub runtimes: Vec<String>,
    pub cold: bool,
    pub runs: usize,
    pub warmup: usize,
    pub window_ms: u64,
    pub warmup_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RegimeReport {
    pub scenarios: BTreeMap<String, ScenarioReport>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScenarioReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<RuntimeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wjsm: Option<RuntimeReport>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RuntimeReport {
    pub wall: Option<WallStats>,
    /// 仅 default 档有值。
    pub ns_per_op: Option<f64>,
    pub max_rss_kb: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WallStats {
    pub mean_s: f64,
    pub stddev_s: f64,
    pub median_s: f64,
    pub min_s: f64,
    pub max_s: f64,
    pub runs: usize,
}

/// 当前 UTC 时间的 RFC 3339 字符串（手写，秒精度）。
pub fn rfc3339_now() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = i64::try_from(secs / 86_400).expect("unix 秒数除以 86400 必然落在 i64 范围内");
    let secs_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的 civil_from_days 算法：天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

/// JSON 写盘：建父目录 + pretty print。
pub fn write_json(path: &std::path::Path, value: &impl Serialize) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow::anyhow!("创建 {} 失败: {error}", parent.display()))?;
    let payload = serde_json::to_vec_pretty(value)
        .map_err(|error| anyhow::anyhow!("序列化报告失败: {error}"))?;
    std::fs::write(path, payload)
        .map_err(|error| anyhow::anyhow!("写入 {} 失败: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_now_formats_utc() {
        let value = rfc3339_now();
        // 形如 2026-07-31T14:00:00Z
        assert_eq!(value.len(), 20);
        assert!(value.ends_with('Z'));
        assert_eq!(&value[4..5], "-");
        assert_eq!(&value[7..8], "-");
        assert_eq!(&value[10..11], "T");
        assert_eq!(&value[13..14], ":");
        assert_eq!(&value[16..17], ":");
    }
}
