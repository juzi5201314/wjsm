//! Sound / Speculative IR→IR 优化入口。

mod cfg_fold;
mod dce;
mod escape_scalar;
mod escape_scalar_record;
pub mod facts;
mod gvn;
mod inline_for_ea;
pub mod ir_walk;
mod licm;
mod licm_apply;
mod licm_elem_guard;
mod licm_facts;
mod liveness;
mod object_literal_read_fold;
mod speculative;
mod speculative_poly;

use wjsm_ir::{BasicBlockId, FunctionId, Program};

pub use facts::{
    BinaryFact, CallFact, ElemFact, INLINE_MAX_CALLEE_INSTRUCTIONS, INLINE_MAX_DEPTH,
    INLINE_MAX_NET_GROWTH, POLY_MAX, PropFact, SpeculativeFacts,
};
pub use inline_for_ea::{find_exception_path, max_value_id_in_function, undefined_const_id};
pub use ir_walk::{collect_uses, instr_uses, instruction_dest, terminator_uses};
pub use licm::licm_disabled_by_env;
pub use liveness::live_values_at;
pub use speculative::rewrite_speculative;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptimizeMode {
    Sound,
    Speculative,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeoptMap {
    pub points: Vec<DeoptMapEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeoptMapEntry {
    pub overlay_block: BasicBlockId,
    pub overlay_instruction: u32,
    pub generic_function: FunctionId,
    pub generic_block: BasicBlockId,
    pub generic_instruction: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OsrMap {
    pub headers: Vec<(BasicBlockId, BasicBlockId)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SlotMap {
    pub sites: Vec<SlotMapEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotMapEntry {
    pub overlay_block: BasicBlockId,
    pub overlay_instruction: u32,
    pub generic_block: BasicBlockId,
    pub generic_instruction: u32,
}

#[derive(Clone, Debug)]
pub struct OptimizedUnit {
    pub program: Program,
    pub deopt_map: DeoptMap,
    pub osr_map: OsrMap,
    pub slot_map: SlotMap,
}

pub fn optimize_sound(program: &mut Program) {
    inline_for_ea::run(program);
    cfg_fold::run(program);
    object_literal_read_fold::run(program);
    escape_scalar::run(program);
    wjsm_ir::typed_cfg::rewrite_program(program, &std::collections::HashMap::new());
    cfg_fold::run_after_value_class(program);
    licm::run(program);
    gvn::run(program);
    dce::run(program);
    dce::prune_unreachable(program);
}

pub fn optimize_speculative(program: &mut Program, facts: &SpeculativeFacts) -> OptimizedUnit {
    let mut deopt_map = DeoptMap::default();
    let mut slot_map = SlotMap::default();
    speculative::rewrite_speculative(program, facts, &mut deopt_map, &mut slot_map);
    inline_for_ea::run(program);
    let class_seeds = wjsm_ir::value_class::FunctionSeeds {
        param_is_number: facts.param_tags.iter().map(|tag| *tag == 0x1f).collect(),
        extra_numbers: facts.extra_number_values.iter().copied().collect(),
    };
    let mut seeds = std::collections::HashMap::new();
    seeds.insert(facts.function.0, class_seeds);
    escape_scalar::run(program);
    wjsm_ir::typed_cfg::rewrite_program(program, &seeds);
    cfg_fold::run_after_value_class(program);
    licm::run(program);
    gvn::run(program);
    dce::run(program);
    dce::prune_unreachable(program);
    let osr_map = osr_headers(program, facts.function);
    OptimizedUnit {
        program: program.clone(),
        deopt_map,
        osr_map,
        slot_map,
    }
}

fn osr_headers(program: &Program, function_id: FunctionId) -> OsrMap {
    let Some(function) = program.functions().get(function_id.0 as usize) else {
        return OsrMap::default();
    };
    let headers = wjsm_ir::typed_cfg::loop_headers(function);
    OsrMap {
        headers: headers.into_iter().map(|block| (block, block)).collect(),
    }
}

pub fn optimize(program: &mut Program, mode: OptimizeMode, facts: Option<&SpeculativeFacts>) {
    match mode {
        OptimizeMode::Sound => optimize_sound(program),
        OptimizeMode::Speculative => {
            let facts = facts.expect("speculative optimize requires facts");
            let _unit = optimize_speculative(program, facts);
        }
    }
}
