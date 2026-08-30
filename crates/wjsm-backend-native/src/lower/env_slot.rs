//! 闭包 `$env` 固定槽读写：Cranelift 内联 shape 守卫 + 直读/写槽，miss 回退 GetProp/SetProp。

use super::*;
use anyhow::{Context, Result};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use std::mem::offset_of;
use wjsm_ir::{ValueId, constants};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

use crate::ENV_LAYOUT_META_WORDS;

fn env_value_slot_offset(slot: u32) -> Result<i32> {
    let scaled = u64::from(slot)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .context("env slot scale overflows")?;
    let offset = u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
        .checked_add(scaled)
        .context("env slot offset overflows")?;
    i32::try_from(offset).context("env slot offset exceeds i32")
}

fn emit_load_env_layout_meta_word(
    cx: &mut LoweringCx<'_, '_>,
    word_index: u32,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let meta_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, env_layout_meta_base))?,
    );
    let entry_offset = (u64::from(cx.function_index) * u64::try_from(ENV_LAYOUT_META_WORDS).unwrap()
        + u64::from(word_index))
    .checked_mul(4)
    .context("env layout meta offset overflows")?;
    let entry_offset =
        i64::try_from(entry_offset).context("env layout meta offset exceeds i64")?;
    let address = cx.builder.ins().iadd_imm_s(meta_base, entry_offset);
    let word = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), address, 0);
    Ok(cx.builder.ins().uextend(types::I64, word))
}

fn emit_env_layout_fast_gate(
    cx: &mut LoweringCx<'_, '_>,
    slot: u32,
) -> Result<(ir::Value, ir::Block, ir::Block)> {
    let expected_shape = emit_load_env_layout_meta_word(cx, 0)?;
    let slot_count = emit_load_env_layout_meta_word(cx, 1)?;
    let slot_val = cx.builder.ins().iconst(types::I64, i64::from(slot));
    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let zero = cx.builder.ins().iconst(types::I64, 0);
    let layout_ok = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::NotEqual, expected_shape, zero);
    let slot_ok = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThan,
        slot_val,
        slot_count,
    );
    let can_fast = cx.builder.ins().band(layout_ok, slot_ok);
    cx.builder
        .ins()
        .brif(can_fast, fast_block, &[], slow_block, &[]);
    Ok((expected_shape, slow_block, fast_block))
}

fn emit_env_shape_guard(
    cx: &mut LoweringCx<'_, '_>,
    env: ValueId,
    expected_shape: ir::Value,
) -> Result<(ir::Block, ir::Block)> {
    let hit = cx.builder.create_block();
    let miss = cx.builder.create_block();
    let encoded = use_value_boxed(cx.builder, cx.variables, env)?;
    let (tag_ok, addr) = resolve_object_addr(cx, encoded)?;
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_ok = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, expected_shape);
    let guard_ok = cx.builder.ins().band(tag_ok, shape_ok);
    cx.builder.ins().brif(guard_ok, hit, &[], miss, &[]);
    Ok((hit, miss))
}

pub(crate) fn lower_load_env_slot(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    has_env_layout: bool,
    dest: ValueId,
    env: ValueId,
    slot: u32,
    key: ValueId,
) -> Result<()> {
    if !has_env_layout {
        return lower_value_operation(cx, NativeRuntimeOp::GetProp, &[env, key], Some(dest));
    }

    let (expected_shape, slow_getprop, fast_block) = emit_env_layout_fast_gate(cx, slot)?;
    let merge_block = cx.builder.create_block();

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let (shape_hit, shape_miss) = emit_env_shape_guard(cx, env, expected_shape)?;
    cx.builder.switch_to_block(shape_miss);
    cx.builder.seal_block(shape_miss);
    cx.builder.ins().jump(slow_getprop, &[]);

    cx.builder.switch_to_block(shape_hit);
    cx.builder.seal_block(shape_hit);
    let offset = env_value_slot_offset(slot)?;
    emit_guarded_slot_read(
        cx,
        tables.barrier_thunks,
        dest,
        env,
        offset,
        merge_block,
        slow_getprop,
    )?;

    cx.builder.switch_to_block(slow_getprop);
    cx.builder.seal_block(slow_getprop);
    lower_value_operation(cx, NativeRuntimeOp::GetProp, &[env, key], Some(dest))?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

pub(crate) fn lower_store_env_slot(
    cx: &mut LoweringCx<'_, '_>,
    has_env_layout: bool,
    dest: Option<ValueId>,
    env: ValueId,
    slot: u32,
    value: ValueId,
    key: ValueId,
    strict: bool,
) -> Result<()> {
    let set_op = if strict {
        NativeRuntimeOp::SetPropStrict
    } else {
        NativeRuntimeOp::SetProp
    };
    if !has_env_layout {
        return lower_value_operation(cx, set_op, &[env, key, value], dest);
    }

    let (expected_shape, slow_setprop, fast_block) = emit_env_layout_fast_gate(cx, slot)?;
    let merge_block = cx.builder.create_block();

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let (shape_hit, shape_miss) = emit_env_shape_guard(cx, env, expected_shape)?;
    cx.builder.switch_to_block(shape_miss);
    cx.builder.seal_block(shape_miss);
    cx.builder.ins().jump(slow_setprop, &[]);

    cx.builder.switch_to_block(shape_hit);
    cx.builder.seal_block(shape_hit);
    let encoded = use_value_boxed(cx.builder, cx.variables, env)?;
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let (_tag_ok, addr) = resolve_object_addr(cx, encoded)?;
    let offset = env_value_slot_offset(slot)?;
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    }
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(slow_setprop);
    cx.builder.seal_block(slow_setprop);
    lower_value_operation(cx, set_op, &[env, key, value], dest)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}
