//! overlay 守卫、槽访问、精确 deopt 与 generic resume 分发。

#![allow(unused_imports)]
use std::collections::HashMap;
use std::mem::offset_of;

use anyhow::{Context, Result};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use wjsm_ir::{BasicBlockId, FunctionId, Program, ValueId, constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

use crate::value_repr::{define_value_boxed, use_value_boxed};

use super::{
    BarrierThunks, LoweringCx, emit_feedback_tag_code, emit_guarded_slot_read,
    emit_is_boxed_handle, vmctx_offset,
};

fn define_boxed_bool(cx: &mut LoweringCx<'_, '_>, dest: ValueId, flag: ir::Value) -> Result<()> {
    let true_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(true));
    let false_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(false));
    let encoded = cx.builder.ins().select(flag, true_value, false_value);
    define_value_boxed(cx.builder, cx.variables, dest, encoded)
}

pub(crate) fn lower_guard_tag(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    value: ValueId,
    tag: u8,
) -> Result<()> {
    let encoded = use_value_boxed(cx.builder, cx.variables, value)?;
    let actual = emit_feedback_tag_code(cx.builder, encoded);
    let expected = cx.builder.ins().iconst(types::I64, i64::from(tag));
    let ok = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, actual, expected);
    define_boxed_bool(cx, dest, ok)
}

fn resolve_object_addr(
    cx: &mut LoweringCx<'_, '_>,
    object: ir::Value,
) -> Result<(ir::Value, ir::Value)> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let is_boxed = emit_is_boxed_handle(cx.builder, object);
    let tag = cx.builder.ins().ushr_imm_u(object, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    let tag_ok = cx.builder.ins().bor(is_obj, is_array);
    let tag_ok = cx.builder.ins().band(is_boxed, tag_ok);
    let handle_idx = cx.builder.ins().band_imm_u(object, i64::from(u32::MAX));
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(cx.ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    Ok((tag_ok, addr))
}

pub(crate) fn lower_guard_shape(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    object: ValueId,
    shape_id: u32,
) -> Result<()> {
    let encoded = use_value_boxed(cx.builder, cx.variables, object)?;
    let (tag_ok, addr) = resolve_object_addr(cx, encoded)?;
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let expected = cx.builder.ins().iconst(types::I64, i64::from(shape_id));
    let shape_ok = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, expected);
    let ok = cx.builder.ins().band(tag_ok, shape_ok);
    define_boxed_bool(cx, dest, ok)
}

pub(crate) fn lower_guard_elements_kind(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    array: ValueId,
    kind: u32,
) -> Result<()> {
    let encoded = use_value_boxed(cx.builder, cx.variables, array)?;
    let (tag_ok, addr) = resolve_object_addr(cx, encoded)?;
    let header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 0);
    let shift = i64::from(constants::HEAP_ARRAY_KIND_OFFSET) * 8;
    let shifted = cx.builder.ins().ushr_imm_u(header, shift);
    let actual = cx.builder.ins().band_imm_u(shifted, 0xFF);
    let expected = cx.builder.ins().iconst(types::I64, i64::from(kind));
    let kind_ok = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, actual, expected);
    let ok = cx.builder.ins().band(tag_ok, kind_ok);
    define_boxed_bool(cx, dest, ok)
}

pub(crate) fn lower_guard_call_target(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    callee: ValueId,
    function: FunctionId,
) -> Result<()> {
    let encoded = use_value_boxed(cx.builder, cx.variables, callee)?;
    let expected = cx.builder.ins().iconst(types::I64, i64::from(function.0));
    let result = cx.call(
        NativeRuntimeOp::GuardSameFunction.id(),
        &[encoded, expected],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)
}

pub(crate) fn lower_load_slot(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    index: u32,
) -> Result<()> {
    let offset = i32::try_from(
        constants::HEAP_OBJECT_HEADER_SIZE + index * constants::HEAP_OBJECT_VALUE_SLOT_SIZE,
    )
    .context("slot offset")?;
    let merge = cx.builder.create_block();
    let slow = cx.builder.create_block();
    emit_guarded_slot_read(cx, barrier_thunks, dest, object, offset, merge, slow)?;
    cx.builder.switch_to_block(slow);
    cx.builder.seal_block(slow);
    emit_deopt_to_generic(cx, cx.current_block, &[object])?;
    cx.builder.switch_to_block(merge);
    cx.builder.seal_block(merge);
    Ok(())
}

pub(crate) fn lower_store_slot(
    cx: &mut LoweringCx<'_, '_>,
    _barrier_thunks: &BarrierThunks,
    dest: Option<ValueId>,
    object: ValueId,
    index: u32,
    value: ValueId,
    transition_shape: Option<u32>,
) -> Result<()> {
    let encoded = use_value_boxed(cx.builder, cx.variables, object)?;
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let (_ok, addr) = resolve_object_addr(cx, encoded)?;
    let offset = i32::try_from(
        constants::HEAP_OBJECT_HEADER_SIZE + index * constants::HEAP_OBJECT_VALUE_SLOT_SIZE,
    )
    .context("slot offset")?;
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    if let Some(shape) = transition_shape {
        let word = cx
            .builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), addr, 8);
        let low = cx.builder.ins().band_imm_u(word, i64::from(u32::MAX));
        let new_high = cx.builder.ins().iconst(types::I64, i64::from(shape) << 32);
        let updated = cx.builder.ins().bor(low, new_high);
        cx.builder
            .ins()
            .store(MemFlagsData::trusted(), updated, addr, 8);
    }
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, encoded)?;
    }
    Ok(())
}

pub(crate) fn emit_feedback_shape_store(
    cx: &mut LoweringCx<'_, '_>,
    shape: ir::Value,
    slot: ir::Value,
) {
    let Some(ptr) = cx.feedback_ptr else {
        return;
    };
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        shape,
        ptr,
        i32::try_from(constants::FEEDBACK_SLOT_SHAPE_ID_OFFSET).expect("shape offset"),
    );
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        slot,
        ptr,
        i32::try_from(constants::FEEDBACK_SLOT_SLOT_OR_KIND_OFFSET).expect("slot offset"),
    );
}

pub(crate) fn emit_deopt_to_generic(
    cx: &mut LoweringCx<'_, '_>,
    block: BasicBlockId,
    lives: &[ValueId],
) -> Result<()> {
    store_resume_lives(cx, lives)?;
    let function = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(cx.function_index));
    let block_id = cx.builder.ins().iconst(types::I64, i64::from(block.0));
    let instruction = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(cx.current_instruction));
    let inst_i32 = cx
        .builder
        .ins()
        .iconst(types::I32, i64::from(cx.current_instruction));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        inst_i32,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_instruction_index))?,
    );
    let count = cx
        .builder
        .ins()
        .iconst(types::I64, i64::try_from(lives.len()).unwrap_or(0));
    let env = cx.env;
    let this_value = cx.this_value;
    let result = cx.call(
        NativeRuntimeOp::DeoptToGeneric.id(),
        &[function, block_id, instruction, env, this_value, count],
        None,
    )?;
    cx.unlink_roots()?;
    cx.builder.ins().return_(&[result]);
    Ok(())
}

pub(crate) fn store_resume_lives(cx: &mut LoweringCx<'_, '_>, lives: &[ValueId]) -> Result<()> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let slots = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_slots))?,
    );
    for (index, live) in lives.iter().enumerate() {
        let value = use_value_boxed(cx.builder, cx.variables, *live)?;
        let offset = i32::try_from(index * 8).context("resume live offset")?;
        cx.builder
            .ins()
            .store(MemFlagsData::trusted(), value, slots, offset);
    }
    let count = cx
        .builder
        .ins()
        .iconst(types::I32, i64::try_from(lives.len()).unwrap_or(0));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        count,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_count))?,
    );
    Ok(())
}

pub(crate) fn emit_resume_dispatch(
    cx: &mut LoweringCx<'_, '_>,
    program: &Program,
    function: &wjsm_ir::Function,
    function_index: u32,
    blocks: &HashMap<BasicBlockId, ir::Block>,
    resume_pads: &HashMap<(BasicBlockId, u32), ir::Block>,
    headers: &[BasicBlockId],
    entry_body: ir::Block,
) -> Result<()> {
    let resume = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_block_plus_one))?,
    );
    let has_resume = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, resume, 0);
    let dispatch = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(has_resume, dispatch, &[], entry_body, &[]);
    cx.builder.switch_to_block(dispatch);
    let func = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_function_id))?,
    );
    let mine = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        func,
        i64::from(cx.function_index),
    );
    let take = cx.builder.create_block();
    cx.builder.ins().brif(mine, take, &[], entry_body, &[]);
    cx.builder.switch_to_block(take);
    let zero = cx.builder.ins().iconst(types::I32, 0);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        zero,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_block_plus_one))?,
    );
    let wanted = cx.builder.ins().iadd_imm_s(resume, -1);
    let wanted_inst = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_instruction_index))?,
    );
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let slots = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_slots))?,
    );
    let mut pads: Vec<((BasicBlockId, u32), ir::Block)> =
        resume_pads.iter().map(|(k, v)| (*k, *v)).collect();
    pads.sort_by_key(|(key, _)| (key.0.0, key.1));
    for ((block_id, inst), pad) in pads {
        let block_hit =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, wanted, i64::from(block_id.0));
        let inst_hit =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, wanted_inst, i64::from(inst));
        let hit = cx.builder.ins().band(block_hit, inst_hit);
        let restore = cx.builder.create_block();
        let skip = cx.builder.create_block();
        cx.builder.ins().brif(hit, restore, &[], skip, &[]);
        cx.builder.switch_to_block(restore);
        let lives = wjsm_optimize::live_values_at(
            program,
            FunctionId(function_index),
            block_id,
            inst as usize,
        );
        for (index, live) in lives.iter().enumerate() {
            let offset = i32::try_from(index * 8).context("resume live offset")?;
            let value = cx
                .builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), slots, offset);
            define_value_boxed(cx.builder, cx.variables, *live, value)?;
        }
        cx.builder.ins().jump(pad, &[]);
        cx.builder.switch_to_block(skip);
    }
    for header in headers {
        let hit =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, wanted, i64::from(header.0));
        let restore = cx.builder.create_block();
        let skip = cx.builder.create_block();
        cx.builder.ins().brif(hit, restore, &[], skip, &[]);
        cx.builder.switch_to_block(restore);
        let lives = wjsm_ir::typed_cfg::loop_header_live_phis(function, *header);
        for (index, live) in lives.iter().enumerate() {
            let offset = i32::try_from(index * 8).context("resume live offset")?;
            let value = cx
                .builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), slots, offset);
            define_value_boxed(cx.builder, cx.variables, *live, value)?;
        }
        let target = if *header == function.entry() {
            entry_body
        } else {
            blocks[header]
        };
        cx.builder.ins().jump(target, &[]);
        cx.builder.switch_to_block(skip);
    }
    cx.builder.ins().jump(entry_body, &[]);
    Ok(())
}
