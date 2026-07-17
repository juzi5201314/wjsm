use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MIB: u64 = 1024 * 1024;
const BYTES_PER_LOGICAL_OBJECT: u64 = MIB / 8;
const MAX_LOGICAL_OBJECTS: u64 = 32_768;
pub const WORKLOAD_CONTRACT_VERSION: u32 = 1;

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
        let logical_graph_hash = hash_contract(self.kind, self.seed, allocations, retained);
        Scenario {
            manifest: ScenarioManifest {
                name: self.kind.as_str().into(),
                seed: self.seed,
                heap_cap_bytes: self.heap_cap_bytes,
                live_set_percent: self.live_set_percent,
                retained_objects: retained,
                workload_contract_version: WORKLOAD_CONTRACT_VERSION,
                logical_graph_hash,
            },
            denominators: Denominators {
                logical_objects: allocations,
                reference_edges: reference_edges(self.kind, allocations),
                planned_allocation_bytes: allocations.saturating_mul(32),
                physical_allocated_bytes: None,
            },
            source: source_for(self.kind, self.seed, allocations, retained),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ScenarioManifest {
    pub name: String,
    pub seed: u64,
    pub heap_cap_bytes: u64,
    pub live_set_percent: u8,
    pub retained_objects: u64,
    pub workload_contract_version: u32,
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

impl Scenario {
    /// Java driver 接收与 WJSM source 相同的 canonical workload contract 参数。
    pub fn java_args(&self) -> [String; 5] {
        [
            self.manifest.name.clone(),
            self.denominators.logical_objects.to_string(),
            self.manifest.retained_objects.to_string(),
            self.manifest.seed.to_string(),
            self.manifest.logical_graph_hash.clone(),
        ]
    }
}

fn allocation_count(heap_cap_bytes: u64) -> u64 {
    // 32 MiB ZGC 的实测边界：4,096 个根对象会让单样本超过 100 秒，1,024 个
    // 仍约 14 秒，而 256 个对象执行真实 full GC 约 5 秒。每个逻辑对象按 128 KiB
    // 预算，既保留 heap/metadata/relocation 余量，也让 30 样本基线受 180 秒绝对
    // 超时约束，避免 benchmark 自身成为不可控长跑。
    (heap_cap_bytes / BYTES_PER_LOGICAL_OBJECT).clamp(256, MAX_LOGICAL_OBJECTS)
}

fn reference_edges(kind: ScenarioKind, allocations: u64) -> u64 {
    match kind {
        ScenarioKind::Churn | ScenarioKind::Chain | ScenarioKind::IdleUncommit => allocations,
        ScenarioKind::Request => allocations.saturating_mul(3),
        ScenarioKind::Cycle => allocations.saturating_add(1),
        ScenarioKind::Wide => allocations.saturating_mul(5),
        ScenarioKind::Mutation | ScenarioKind::Humongous => allocations,
        ScenarioKind::Saturation => allocations.saturating_mul(2),
    }
}

fn source_for(kind: ScenarioKind, seed: u64, allocations: u64, retained: u64) -> String {
    let common = format!(
        "const total={allocations};const retained={retained};const seed={seed};const slots=retained===0?1:retained;let roots=[];"
    );
    let body = match kind {
        ScenarioKind::Churn => {
            "for(let i=0;i<total;i++){let node={id:i,next:roots[i%slots],payload:null};if(i<retained)roots[i%slots]=node;}gc();"
        }
        ScenarioKind::Request => {
            "for(let i=0;i<total;i++){let header={id:i,next:null,payload:null};let body=[i,seed];let node={id:i,next:roots[i%slots],payload:[header,body]};if(i<retained)roots[i%slots]=node;}"
        }
        ScenarioKind::Chain => {
            "let tail=null;for(let i=0;i<total;i++){tail={id:i,next:tail,payload:null};if(i<retained)roots[i%slots]=tail;}"
        }
        ScenarioKind::Cycle => {
            "let first={id:0,next:null,payload:null};let tail=first;for(let i=1;i<total;i++){let node={id:i,next:null,payload:null};tail.next=node;tail=node;if(i<retained)roots[i%slots]=node;}tail.next=first;"
        }
        ScenarioKind::Wide => {
            "for(let i=0;i<total;i++){let payload=[{id:i,next:null,payload:null},{id:i+1,next:null,payload:null},{id:i+2,next:null,payload:null},{id:i+3,next:null,payload:null}];let node={id:i,next:null,payload:payload};if(i<retained)roots[i%slots]=node;}"
        }
        ScenarioKind::Mutation => {
            "for(let i=0;i<total;i++){let node={id:i,next:roots[i%slots],payload:null};node.next={id:i+1,next:null,payload:null};if(i<retained)roots[i%slots]=node;}"
        }
        ScenarioKind::Humongous => {
            "for(let i=0;i<total;i++){let node={id:i,next:null,payload:new Array(64)};if(i<retained)roots[i%slots]=node;}"
        }
        ScenarioKind::IdleUncommit => {
            "for(let i=0;i<total;i++){let node={id:i,next:roots[i%slots],payload:null};if(i<retained)roots[i%slots]=node;}gc();"
        }
        ScenarioKind::Saturation => {
            "for(let i=0;i<total;i++){let left={id:i,next:null,payload:null};let right={id:i+1,next:null,payload:null};let node={id:i,next:left,payload:right};if(i<retained)roots[i%slots]=node;}"
        }
    };
    format!("{common}{body}console.log(roots.length);")
}

fn hash_contract(kind: ScenarioKind, seed: u64, allocations: u64, retained: u64) -> String {
    let contract = format!(
        "wjsm-gc-workload-v{}|{}|{seed}|{allocations}|{retained}",
        WORKLOAD_CONTRACT_VERSION,
        kind.as_str()
    );
    let digest = Sha256::digest(contract.as_bytes());
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
    fn java_and_wjsm_share_canonical_workload_identity() {
        let scenario = ScenarioSpec::new(ScenarioKind::Churn, 7, 32 * MIB, 50).build();
        let args = scenario.java_args();
        assert_eq!(args[0], "churn");
        assert_eq!(args[1], "256");
        assert_eq!(args[2], "128");
        assert_eq!(args[4], scenario.manifest.logical_graph_hash);
        assert!(scenario.source.ends_with("gc();console.log(roots.length);"));
    }
}
