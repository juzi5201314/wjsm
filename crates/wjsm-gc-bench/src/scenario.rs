use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MIB: u64 = 1024 * 1024;
const BYTES_PER_LOGICAL_OBJECT: u64 = MIB / 8;
const MAX_LOGICAL_OBJECTS: u64 = 32_768;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioKind {
    #[default]
    Churn,
    Request,
    Chain,
    Cycle,
    Wide,
    Mutation,
    Humongous,
    IdleUncommit,
    Saturation,
}

impl ScenarioKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Churn => "churn",
            Self::Request => "request",
            Self::Chain => "chain",
            Self::Cycle => "cycle",
            Self::Wide => "wide",
            Self::Mutation => "mutation",
            Self::Humongous => "humongous",
            Self::IdleUncommit => "idle-uncommit",
            Self::Saturation => "saturation",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScenarioSpec {
    kind: ScenarioKind,
    seed: u64,
    heap_cap_bytes: u64,
    live_set_percent: u8,
}

impl ScenarioSpec {
    pub fn new(kind: ScenarioKind, seed: u64, heap_cap_bytes: u64, live_set_percent: u8) -> Self {
        assert!(live_set_percent <= 100, "live set must be a percentage");
        Self {
            kind,
            seed,
            heap_cap_bytes,
            live_set_percent,
        }
    }

    pub fn build(&self) -> Scenario {
        let allocations = allocation_count(self.heap_cap_bytes);
        let retained = allocations.saturating_mul(u64::from(self.live_set_percent)) / 100;
        let source = source_for(self.kind, self.seed, allocations, retained);
        let logical_graph_hash = hash_source(&source);
        Scenario {
            manifest: ScenarioManifest {
                name: self.kind.as_str().into(),
                seed: self.seed,
                heap_cap_bytes: self.heap_cap_bytes,
                live_set_percent: self.live_set_percent,
                logical_graph_hash,
            },
            denominators: Denominators {
                logical_objects: allocations,
                reference_edges: reference_edges(self.kind, allocations, retained),
                planned_allocation_bytes: allocations.saturating_mul(32),
                physical_allocated_bytes: None,
            },
            source,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ScenarioManifest {
    pub name: String,
    pub seed: u64,
    pub heap_cap_bytes: u64,
    pub live_set_percent: u8,
    pub logical_graph_hash: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Denominators {
    pub logical_objects: u64,
    pub reference_edges: u64,
    pub planned_allocation_bytes: u64,
    /// 当前 memory32 heap 尚不公开累计物理分配计数；缺失值必须使性能 gate
    /// 进入 `needs-verification`，不可用逻辑对象估算替代。
    pub physical_allocated_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub manifest: ScenarioManifest,
    pub denominators: Denominators,
    pub source: String,
}

fn allocation_count(heap_cap_bytes: u64) -> u64 {
    // 32 MiB ZGC 的实测边界：4,096 个根对象会让单样本超过 100 秒，1,024 个
    // 仍约 14 秒，而 256 个对象执行真实 full GC 约 5 秒。每个逻辑对象按 128 KiB
    // 预算，既保留 heap/metadata/relocation 余量，也让 30 样本基线受 180 秒绝对
    // 超时约束，避免 benchmark 自身成为不可控长跑。
    (heap_cap_bytes / BYTES_PER_LOGICAL_OBJECT).clamp(256, MAX_LOGICAL_OBJECTS)
}

fn reference_edges(kind: ScenarioKind, allocations: u64, retained: u64) -> u64 {
    match kind {
        ScenarioKind::Chain | ScenarioKind::Cycle => allocations,
        ScenarioKind::Wide => allocations.saturating_mul(4),
        ScenarioKind::Mutation => allocations.saturating_mul(2),
        ScenarioKind::Request | ScenarioKind::Saturation => retained.saturating_mul(2),
        ScenarioKind::Humongous => allocations / 16,
        ScenarioKind::Churn | ScenarioKind::IdleUncommit => retained,
    }
}

fn source_for(kind: ScenarioKind, seed: u64, allocations: u64, retained: u64) -> String {
    let common = format!(
        "const total={allocations}; const retained={retained}; const seed={seed}; let roots=[];"
    );
    let body = match kind {
        ScenarioKind::Churn => {
            "for(let i=0;i<total;i++){let o={i,seed,next:roots[i&255]};if(i<retained)roots[i%retained]=o;}gc();"
        }
        ScenarioKind::Request => {
            "for(let i=0;i<total;i++){let request={i,headers:{seed},body:[i,seed]};if(i<retained)roots[i%retained]=request;}"
        }
        ScenarioKind::Chain => {
            "let tail=null;for(let i=0;i<total;i++){tail={i,next:tail};if(i<retained)roots[i%retained]=tail;}"
        }
        ScenarioKind::Cycle => {
            "let first={i:0};let tail=first;for(let i=1;i<total;i++){let o={i,next:first};tail.next=o;tail=o;if(i<retained)roots[i%retained]=o;}"
        }
        ScenarioKind::Wide => {
            "for(let i=0;i<total;i++){let o={i,a:{i},b:{i},c:{i},d:{i}};if(i<retained)roots[i%retained]=o;}"
        }
        ScenarioKind::Mutation => {
            "for(let i=0;i<total;i++){let o={i,next:roots[i&255]};o.next={i:i+1};if(i<retained)roots[i%retained]=o;}"
        }
        ScenarioKind::Humongous => {
            "for(let i=0;i<total;i++){let o={i,data:new Array(64)};if(i<retained)roots[i%retained]=o;}"
        }
        ScenarioKind::IdleUncommit => {
            "for(let i=0;i<total;i++){let o={i,data:[i,seed]};if(i<retained)roots[i%retained]=o;}gc();"
        }
        ScenarioKind::Saturation => {
            "for(let i=0;i<total;i++){let o={i,left:{i},right:{i}};if(i<retained)roots[i%retained]=o;}"
        }
    };
    format!("{common}{body} console.log(roots.length);")
}

fn hash_source(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn scenario_names_are_clap_values() {
        assert_eq!(ScenarioKind::IdleUncommit.as_str(), "idle-uncommit");
    }

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn allocation_count_stays_bounded() {
        assert_eq!(allocation_count(1), 256);
        assert_eq!(allocation_count(64 * MIB), 512);
    }

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn churn_scenario_forces_a_cycle_within_32m_capacity_model() {
        let scenario = ScenarioSpec::new(ScenarioKind::Churn, 7, 32 * MIB, 50).build();
        assert_eq!(scenario.denominators.logical_objects, 256);
        assert!(
            scenario
                .source
                .ends_with("gc(); console.log(roots.length);")
        );
    }
}
