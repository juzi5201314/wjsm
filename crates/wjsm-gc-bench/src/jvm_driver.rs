use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use crate::cli::GcKind;
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

    pub fn run_sample(&self, scenario: &Scenario, gc: GcKind) -> Result<JdkSample> {
        let java = self.metadata.jdk_home.join("bin/java");
        let mut command = Command::new(&java);
        command.arg(jdk_gc_flag(gc)?);
        command.arg(format!("-Xmx{}", scenario.manifest.heap_cap_bytes));
        if self.metadata.diagnostic_counters_available {
            command.arg("-XX:+UnlockDiagnosticVMOptions");
            command.arg("-XX:+WjsmGcBenchmarkCounters");
        }
        let args = scenario.java_args();
        command
            .arg("-cp")
            .arg(&self.metadata.classes_dir)
            .arg("WjsmGcBench")
            .args(args);
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
        let steady_state_ns =
            parse_steady_state_ns(&stdout, &scenario.manifest.logical_graph_hash)?;
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

fn jdk_gc_flag(gc: GcKind) -> Result<&'static str> {
    match gc {
        GcKind::Zgc => Ok("-XX:+UseZGC"),
        GcKind::G1 => Ok("-XX:+UseG1GC"),
        GcKind::MarkSweep => anyhow::bail!(
            "JDK 没有与 WJSM mark-sweep 对应的 collector；此比较需要 JDK reference runner"
        ),
    }
}

fn parse_steady_state_ns(stdout: &str, expected_workload_hash: &str) -> Result<u64> {
    let line = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .context("JDK benchmark did not emit JSON")?;
    let value: serde_json::Value =
        serde_json::from_str(line).context("decode JDK benchmark JSON")?;
    let workload_hash = value
        .get("workload_hash")
        .and_then(serde_json::Value::as_str)
        .context("JDK benchmark JSON lacks workload_hash")?;
    if workload_hash != expected_workload_hash {
        anyhow::bail!(
            "JDK workload hash mismatch: expected={expected_workload_hash} actual={workload_hash}"
        );
    }
    value
        .get("steady_state_ns")
        .and_then(serde_json::Value::as_u64)
        .context("JDK benchmark JSON lacks steady_state_ns")
}
