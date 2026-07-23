use serde::{Deserialize, Serialize};

/// 主机硬件与 OS 快照，嵌入每份报告供横向对比。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub cpu_brand: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub physical_memory_bytes: u64,
    pub available_memory_bytes: u64,
}

impl HostInfo {
    pub fn detect() -> Self {
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        let cpus = system.cpus();
        let cpu_brand = cpus
            .first()
            .map(|cpu| cpu.brand().to_owned())
            .unwrap_or_default();
        let logical_cores = cpus.len();
        let physical_cores = sysinfo::System::physical_core_count().unwrap_or(logical_cores);
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            cpu_brand,
            physical_cores,
            logical_cores,
            physical_memory_bytes: system.total_memory(),
            available_memory_bytes: system.available_memory(),
        }
    }
}
