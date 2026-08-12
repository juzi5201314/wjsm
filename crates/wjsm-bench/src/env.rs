//! 环境快照：主机硬件、OS、工具链版本，嵌入每份报告供横向对比。

use serde::Serialize;
use std::process::Command;

use crate::work_dir::repo_root;

/// 当前 native image compiler 使用的 Cranelift 版本。
pub const CRANELIFT_VERSION: &str = wjsm_backend_native::CRANELIFT_VERSION;

/// 主机与工具链快照。
#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentSnapshot {
    pub node_version: Option<String>,
    pub wjsm_version: Option<String>,
    /// Cranelift 版本，用于解释 native image/cache 的可比性。
    pub cranelift_version: String,
    pub hyperfine_version: Option<String>,
    pub os: String,
    pub arch: String,
    pub cpu_brand: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub physical_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub wjsm_gc: String,
    pub wjsm_backend: String,
    pub git_rev: Option<String>,
}

/// 运行 `<bin> --version` 取首行；失败返回 None（不硬失败）。
fn first_line(cmd: &mut Command) -> Option<String> {
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn git_rev() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_root())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rev = String::from_utf8_lossy(&output.stdout);
    let rev = rev.trim();
    (!rev.is_empty()).then(|| rev.to_owned())
}

/// 采集当前主机与工具链快照。所有外部命令失败均降级为 None。
pub fn detect(node_bin: &str, wjsm_bin: &str) -> EnvironmentSnapshot {
    let mut system = sysinfo::System::new_all();
    system.refresh_all();
    let cpus = system.cpus();
    let cpu_brand = cpus
        .first()
        .map(|cpu| cpu.brand().to_owned())
        .unwrap_or_default();
    let logical_cores = cpus.len();
    let physical_cores = sysinfo::System::physical_core_count().unwrap_or(logical_cores);

    EnvironmentSnapshot {
        node_version: first_line(Command::new(node_bin).arg("--version")),
        wjsm_version: first_line(Command::new(wjsm_bin).arg("--version")),
        cranelift_version: CRANELIFT_VERSION.to_owned(),
        hyperfine_version: first_line(Command::new("hyperfine").arg("--version")),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        cpu_brand,
        physical_cores,
        logical_cores,
        physical_memory_bytes: system.total_memory(),
        available_memory_bytes: system.available_memory(),
        wjsm_gc: std::env::var("WJSM_GC").unwrap_or_else(|_| "zgc".into()),
        wjsm_backend: "cranelift-native".to_owned(),
        git_rev: git_rev(),
    }
}
