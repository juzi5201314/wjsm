use serde::{Deserialize, Serialize};

/// 主机硬件、OS 与 GC 平台能力快照，嵌入每份报告供横向对比。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub cpu_brand: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub physical_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub isa: String,
    pub decommit: bool,
    pub hard_isolation: bool,
    pub numa_nodes: Vec<u32>,
    pub large_pages_hint: bool,
    pub needs_capability_runner: Vec<String>,
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
        let capabilities = wjsm_gc::heap::PlatformCapabilities::detect();
        Self {
            os: capabilities.os.into(),
            arch: capabilities.arch.into(),
            cpu_brand,
            physical_cores,
            logical_cores,
            physical_memory_bytes: system.total_memory(),
            available_memory_bytes: system.available_memory(),
            isa: capabilities.isa.as_str().into(),
            decommit: capabilities.decommit,
            hard_isolation: capabilities.hard_isolation,
            numa_nodes: capabilities
                .numa
                .nodes
                .into_iter()
                .map(|node| node.0)
                .collect(),
            large_pages_hint: capabilities.large_pages_hint,
            needs_capability_runner: capabilities
                .needs_capability_runner
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn evidence_status(&self) -> &'static str {
        if self.needs_capability_runner.is_empty() {
            "measured"
        } else {
            "needs-capability-runner"
        }
    }
}
