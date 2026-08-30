#![allow(unused_imports)]
use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem::{offset_of, size_of};

use anyhow::{Context, Result, anyhow, bail};
use cranelift_codegen::ir::{
    self, AbiParam, AtomicRmwOp, Function, InstBuilder, MemFlagsData, Signature, StackSlot,
    StackSlotData, StackSlotKind, UserFuncName, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{
    DataDescription, DataId, FuncId, Linkage, Module, ModuleDeclarations, ModuleReloc,
};
use cranelift_object::{ObjectBuilder, ObjectModule};
use wjsm_ir::{
    BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, EVAL_SCOPE_ENV_PARAM,
    FunctionId, Instruction, Program, Terminator, UnaryOp, ValueId, constants, value,
};
use wjsm_native_abi::{
    COOPERATIVE_POLL_LOOP_BACKEDGE_STEP_BYTES, COOPERATIVE_POLL_STEP_BYTES,
    NATIVE_BARRIER_MARKING_MASK, NativeBarrierState, NativeHostSymbol, NativeRootFrame,
    NativeRuntimeOp, NativeSignature, NativeVmContext, native_variable_names,
};

use rayon::prelude::*;

use crate::f64_analysis::infer_f64_values;
use crate::fast_call::{
    compile_slow_trampoline, fast_entry_signature, fast_js_arity, is_fast_body_eligible,
    is_fast_call_eligible, js_param_count,
};
use crate::root_plan::RootPlan;
use crate::safepoint_free::{function_uses_local_var, infer_safepoint_free_functions};
use crate::template_meta::{
    TemplateOriginMap, TrioField, build_template_origin_maps, plan_ic_slots,
    template_property_index_by_key_text, template_property_index_for_key, trio_field_for_access,
};
use crate::unwind::{UnwindPolicy, UnwindRecord, validate_unwind_info, write_object_unwind};
use crate::value_repr::{
    ValueRepr, box_f64_result, define_value_as, define_value_boxed, define_value_f64, unbox_f64,
    use_value_as, use_value_boxed, use_value_f64,
};
use crate::{NativeCompileError, NativeObject};

mod array_elem;
mod call_arith;
mod eq_binary;
mod feedback;
mod ic_get;
mod ic_set;
mod object_template;
mod overlay;
mod prop_template;
mod string_addr;
mod string_builder;
mod string_slice;

pub(crate) use array_elem::*;
pub(crate) use call_arith::*;
pub(crate) use eq_binary::*;
pub(crate) use feedback::*;
pub(crate) use ic_get::*;
pub(crate) use ic_set::*;
pub(crate) use object_template::*;
pub(crate) use overlay::*;
pub(crate) use prop_template::*;
pub(crate) use string_addr::*;
pub(crate) use string_builder::*;
pub(crate) use string_slice::*;
mod compile_decl;
mod env_slot;
mod lower_fn;
mod phi_locals;
mod term_dispatch;
pub(crate) use compile_decl::*;
pub(crate) use env_slot::*;
pub(crate) use lower_fn::*;
pub(crate) use phi_locals::*;
pub(crate) use term_dispatch::*;

const HOST_OPERATION_SYMBOL: NativeHostSymbol = NativeHostSymbol::HostOperationDispatcher;
const STRING_ADD_SYMBOL: NativeHostSymbol = NativeHostSymbol::StringAdd;
const STRING_BUILDER_FINISH_SYMBOL: NativeHostSymbol = NativeHostSymbol::StringBuilderFinish;
const DYNAMIC_BINARY_BASE: u32 = 0x1_0000;
const DYNAMIC_UNARY_BASE: u32 = 0x1_0100;
const DYNAMIC_COMPARE_BASE: u32 = 0x1_0200;
/// 共享 host 参数区的下限尺寸；无 host 调用的函数也保留一个合法槽。
const ARENA_MIN_BYTES: u32 = 8;

mod cx;
pub(crate) use cx::*;

mod instr;
pub(crate) use instr::*;

/// 把 object 侧 endianness 转成 gimli 的 writer endian；本后端只支持小端目标。
pub(crate) fn gimli_endian(triple: &target_lexicon::Triple) -> gimli::RunTimeEndian {
    match triple.endianness().unwrap() {
        target_lexicon::Endianness::Little => gimli::RunTimeEndian::Little,
        target_lexicon::Endianness::Big => gimli::RunTimeEndian::Big,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, Constant, Function, Instruction, ValueId};

    #[test]
    fn infer_f64_values_requires_exact_math_arity() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(0.5));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(2)),
            builtin: Builtin::MathSin,
            args: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(3)),
            builtin: Builtin::MathSin,
            args: vec![],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(4)),
            builtin: Builtin::MathSin,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(5)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(6)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(7)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0), ValueId(100)],
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        program.push_function(function);

        let inferred = infer_f64_values(&program);
        let f64_values = &inferred[&FunctionId(0)];
        assert!(f64_values.contains(&ValueId(0)));
        assert!(f64_values.contains(&ValueId(1)));
        assert!(f64_values.contains(&ValueId(2)));
        assert!(f64_values.contains(&ValueId(5)));
        assert!(!f64_values.contains(&ValueId(3)));
        assert!(!f64_values.contains(&ValueId(4)));
        assert!(!f64_values.contains(&ValueId(6)));
        assert!(!f64_values.contains(&ValueId(7)));
    }

    #[test]
    fn infer_f64_values_propagates_through_direct_function_calls() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let function_ref = program.add_constant(Constant::FunctionRef(FunctionId(1)));

        let mut caller = Function::new("main", BasicBlockId(0));
        let mut caller_block = BasicBlock::new(BasicBlockId(0));
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: function_ref,
        });
        caller_block.push_instruction(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(0),
            args: vec![ValueId(0)],
            callsite: None,
        });
        caller_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee = Function::new("add1", BasicBlockId(0));
        callee.set_params(vec!["$env".into(), "$this".into(), "x".into()]);
        let mut callee_block = BasicBlock::new(BasicBlockId(0));
        callee_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        callee_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        callee_block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        callee_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        callee.push_block(callee_block);
        program.push_function(callee);

        let inferred = infer_f64_values(&program);
        assert!(inferred[&FunctionId(0)].contains(&ValueId(2)));
        assert!(inferred[&FunctionId(1)].contains(&ValueId(0)));
        assert!(inferred[&FunctionId(1)].contains(&ValueId(2)));
    }

    #[test]
    fn infer_f64_values_rejects_escaped_function_references() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let function_ref = program.add_constant(Constant::FunctionRef(FunctionId(1)));

        let mut caller = Function::new("main", BasicBlockId(0));
        let mut caller_block = BasicBlock::new(BasicBlockId(0));
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: function_ref,
        });
        caller_block.push_instruction(Instruction::StoreVar {
            name: "escaped".into(),
            value: ValueId(1),
        });
        caller_block.push_instruction(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(0),
            args: vec![ValueId(0)],
            callsite: None,
        });
        caller_block.set_terminator(Terminator::Return { value: None });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee = Function::new("add1", BasicBlockId(0));
        callee.set_params(vec!["$env".into(), "$this".into(), "x".into()]);
        let mut callee_block = BasicBlock::new(BasicBlockId(0));
        callee_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        callee_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        callee_block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        callee_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        callee.push_block(callee_block);
        program.push_function(callee);

        let inferred = infer_f64_values(&program);
        assert!(!inferred[&FunctionId(0)].contains(&ValueId(2)));
        assert!(!inferred[&FunctionId(1)].contains(&ValueId(0)));
        assert!(!inferred[&FunctionId(1)].contains(&ValueId(2)));
    }
}
