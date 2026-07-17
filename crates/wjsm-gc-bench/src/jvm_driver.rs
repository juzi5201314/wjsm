use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use crate::jdk_probe::JdkProbeMetadata;
use crate::scenario::Scenario;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JdkCounters {
    pub gc_cpu_ns: Option<u64>,
    pub mark_cpu_ns: Option<u64>,
    pub relocation_cpu_ns: Option<u64>,
    pub relocated_bytes: Option<u64>,
    pub physical_allocated_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct JdkSample {
    pub steady_state_ns: u64,
    pub counters: Option<JdkCounters>,
}

pub struct JdkDriver {
    metadata: JdkProbeMetadata,
}

impl JdkDriver {
    pub fn new(metadata: JdkProbeMetadata) -> Self {
        Self { metadata }
    }

    pub fn run_sample(&self, scenario: &Scenario) -> Result<JdkSample> {
        let java = self.metadata.jdk_home.join("bin/java");
        let mut command = Command::new(&java);
        command
            .arg("-XX:+UseZGC")
            .arg(format!("-Xmx{}", scenario.manifest.heap_cap_bytes));
        if self.metadata.diagnostic_counters_available {
            command.arg("-XX:+UnlockDiagnosticVMOptions");
            command.arg("-XX:+WjsmGcBenchmarkCounters");
        }
        command
            .arg("-cp")
            .arg(&self.metadata.classes_dir)
            .arg("WjsmGcBench")
            .arg(&scenario.manifest.name)
            .arg(scenario.denominators.logical_objects.to_string())
            .arg(
                (scenario.denominators.logical_objects
                    * u64::from(scenario.manifest.live_set_percent)
                    / 100)
                    .to_string(),
            )
            .arg(scenario.manifest.seed.to_string());
        let output = command
            .output()
            .with_context(|| format!("spawn {}", java.display()))?;
        if !output.status.success() {
            anyhow::bail!(
                "JDK benchmark failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let stdout = String::from_utf8(output.stdout).context("decode JDK benchmark output")?;
        let steady_state_ns = parse_steady_state_ns(&stdout)?;
        Ok(JdkSample {
            steady_state_ns,
            // 收集器 patch 的 counters 文件协议在准备阶段不存在时不伪造；driver 将
            // `None` 交给 gate，明确生成 needs-verification。
            counters: None,
        })
    }

    pub fn classes_dir(&self) -> &PathBuf {
        &self.metadata.classes_dir
    }
}

fn parse_steady_state_ns(stdout: &str) -> Result<u64> {
    let line = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .context("JDK benchmark did not emit JSON")?;
    let value: serde_json::Value =
        serde_json::from_str(line).context("decode JDK benchmark JSON")?;
    value
        .get("steady_state_ns")
        .and_then(serde_json::Value::as_u64)
        .context("JDK benchmark JSON lacks steady_state_ns")
}
