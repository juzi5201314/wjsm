use clap::ValueEnum;
use serde::{Deserialize, Serialize};

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

    pub fn all() -> &'static [ScenarioKind] {
        &[
            Self::Churn,
            Self::Request,
            Self::Chain,
            Self::Cycle,
            Self::Wide,
            Self::Mutation,
            Self::Humongous,
            Self::IdleUncommit,
            Self::Saturation,
        ]
    }
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: &'static str,
    pub heap_cap_bytes: u64,
    pub live_set_percent: u8,
    pub seed: u64,
    pub allocations: u64,
    pub retained: u64,
    pub source: String,
}

impl Scenario {
    pub fn build(
        kind: ScenarioKind,
        seed: u64,
        heap_cap_bytes: u64,
        live_set_percent: u8,
        objects_override: Option<u64>,
    ) -> Self {
        assert!(live_set_percent <= 100, "live set must be a percentage");
        let allocations = objects_override.unwrap_or_else(|| allocation_count(heap_cap_bytes));
        let retained = allocations.saturating_mul(u64::from(live_set_percent)) / 100;
        let source = source_for(kind, seed, allocations, retained);
        Self {
            name: kind.as_str(),
            heap_cap_bytes,
            live_set_percent,
            seed,
            allocations,
            retained,
            source,
        }
    }
}

fn allocation_count(heap_cap_bytes: u64) -> u64 {
    (heap_cap_bytes / BYTES_PER_LOGICAL_OBJECT).clamp(256, MAX_LOGICAL_OBJECTS)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_source_is_deterministic() {
        let a = Scenario::build(ScenarioKind::Churn, 42, 32 * MIB, 50, None);
        let b = Scenario::build(ScenarioKind::Churn, 42, 32 * MIB, 50, None);
        assert_eq!(a.source, b.source);
        assert_eq!(a.allocations, b.allocations);
    }

    #[test]
    fn allocation_count_scales_with_heap() {
        let small = allocation_count(32 * MIB);
        let large = allocation_count(1024 * MIB);
        assert!(large > small);
        assert!(small >= 256);
        assert!(large <= MAX_LOGICAL_OBJECTS);
    }
}
