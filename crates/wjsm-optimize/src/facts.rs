//! 投机编译用的纯整数事实：owner 线程从反馈槽 / ShapeTable 拷贝，worker 只读本结构。

use wjsm_ir::{BasicBlockId, FunctionId};

pub const INLINE_MAX_CALLEE_INSTRUCTIONS: usize = 192;
pub const INLINE_MAX_NET_GROWTH: usize = 1024;
pub const INLINE_MAX_DEPTH: usize = 4;
pub const POLY_MAX: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeculativeFacts {
    pub function: FunctionId,
    pub param_tags: Box<[u8]>,
    pub extra_number_values: Vec<wjsm_ir::ValueId>,
    pub get_props: Vec<PropFact>,
    pub set_props: Vec<PropFact>,
    pub get_elems: Vec<ElemFact>,
    pub calls: Vec<CallFact>,
    pub binaries: Vec<BinaryFact>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropFact {
    pub block: BasicBlockId,
    pub instruction_index: u32,
    pub shape_id: u32,
    pub slot_index: u32,
    pub proto_generation: u32,
    pub expected_proto: u32,
    pub own_data: bool,
    pub poly_len: u8,
    pub poly_shapes: [u32; POLY_MAX],
    pub poly_slots: [u32; POLY_MAX],
    pub transition_shape: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElemFact {
    pub block: BasicBlockId,
    pub instruction_index: u32,
    pub elements_kind: u32,
    pub shape_id: u32,
    pub first_kind: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallFact {
    pub block: BasicBlockId,
    pub instruction_index: u32,
    pub target_function: u32,
    pub target_image_id: u64,
    pub this_tag: u8,
    pub this_shape: u32,
    pub construct: bool,
    pub poly_len: u8,
    pub poly_functions: [u32; POLY_MAX],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryFact {
    pub block: BasicBlockId,
    pub instruction_index: u32,
    pub lhs_tag: u8,
    pub rhs_tag: u8,
}

impl SpeculativeFacts {
    pub fn prop_at(&self, block: BasicBlockId, instruction_index: u32) -> Option<&PropFact> {
        self.get_props
            .iter()
            .find(|fact| fact.block == block && fact.instruction_index == instruction_index)
    }

    pub fn set_prop_at(&self, block: BasicBlockId, instruction_index: u32) -> Option<&PropFact> {
        self.set_props
            .iter()
            .find(|fact| fact.block == block && fact.instruction_index == instruction_index)
    }

    pub fn elem_at(&self, block: BasicBlockId, instruction_index: u32) -> Option<&ElemFact> {
        self.get_elems
            .iter()
            .find(|fact| fact.block == block && fact.instruction_index == instruction_index)
    }

    pub fn call_at(&self, block: BasicBlockId, instruction_index: u32) -> Option<&CallFact> {
        self.calls
            .iter()
            .find(|fact| fact.block == block && fact.instruction_index == instruction_index)
    }

    pub fn binary_at(&self, block: BasicBlockId, instruction_index: u32) -> Option<&BinaryFact> {
        self.binaries
            .iter()
            .find(|fact| fact.block == block && fact.instruction_index == instruction_index)
    }
}
