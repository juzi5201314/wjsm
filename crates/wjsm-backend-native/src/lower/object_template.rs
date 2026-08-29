//! 对象分配与模板 meta

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;
use std::mem::offset_of;
use wjsm_ir::{
    BinaryOp, Builtin, CompareOp, Constant, ConstantId, Instruction, UnaryOp, ValueId, constants,
    value,
};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

pub(crate) fn lower_native_object_allocation(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    capacity: u32,
    array: bool,
) -> Result<()> {
    // 与宿主首次扩容策略一致：空对象预留常见构造器字段，避免尚未物化对象
    // 在第一次属性写入时立即搬迁。
    let capacity = if array {
        capacity
    } else {
        capacity.max(constants::HEAP_OBJECT_INITIAL_VALUE_CAPACITY)
    };
    let bytes = u64::from(capacity)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .and_then(|payload| payload.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
        .and_then(|bytes| bytes.checked_add(7))
        .map(|bytes| bytes & !7)
        .context("native object allocation size overflows")?;
    let bytes = i64::try_from(bytes).context("native object allocation size exceeds i64")?;
    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder.append_block_param(merge_block, types::I64);

    let flags = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_fast_flags))?,
    );
    let allocation_flag = if array {
        wjsm_native_abi::NATIVE_ALLOCATION_FAST_ARRAY
    } else {
        wjsm_native_abi::NATIVE_ALLOCATION_FAST_OBJECT
    };
    let enabled = cx
        .builder
        .ins()
        .band_imm_u(flags, i64::from(allocation_flag));
    let enabled = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, enabled, 0);
    let small_limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_small_limit))?,
    );
    let bytes_value = cx.builder.ins().iconst(types::I64, bytes);
    let small = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        bytes_value,
        small_limit,
    );
    let top = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_limit))?,
    );
    let end = cx.builder.ins().iadd(top, bytes_value);
    let object_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThanOrEqual, end, limit);
    let cursor = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let handle_limit = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_limit))?,
    );
    let handle_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, cursor, handle_limit);
    let ready = cx.builder.ins().band(enabled, small);
    let ready = cx.builder.ins().band(ready, object_fits);
    let ready = cx.builder.ins().band(ready, handle_fits);
    cx.builder
        .ins()
        .brif(ready, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let prototype = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(if array {
            offset_of!(NativeVmContext, array_prototype_handle)
        } else {
            offset_of!(NativeVmContext, object_prototype_handle)
        })?,
    );
    let prototype = cx.builder.ins().uextend(types::I64, prototype);
    let heap_type = if array {
        wjsm_ir::HEAP_TYPE_ARRAY
    } else {
        wjsm_ir::HEAP_TYPE_OBJECT
    };
    let type_word = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(u64::from(heap_type) << 32).expect("heap type word fits i64"),
    );
    let header_word = cx.builder.ins().bor(prototype, type_word);
    let heap_delta = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let address = cx.builder.ins().iadd(top, heap_delta);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), header_word, address, 0);
    let capacity_i32 = cx.builder.ins().iconst(types::I32, i64::from(capacity));
    let zero_i32 = cx.builder.ins().iconst(types::I32, 0);
    let first_header_value = if array { zero_i32 } else { capacity_i32 };
    let second_header_value = if array { capacity_i32 } else { zero_i32 };
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        first_header_value,
        address,
        i32::try_from(if array {
            constants::HEAP_ARRAY_LENGTH_OFFSET
        } else {
            constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET
        })
        .expect("capacity or length offset fits i32"),
    );
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        second_header_value,
        address,
        i32::try_from(if array {
            constants::HEAP_ARRAY_CAPACITY_OFFSET
        } else {
            constants::HEAP_OBJECT_SHAPE_ID_OFFSET
        })
        .expect("capacity or shape offset fits i32"),
    );
    let handle_value = cx.builder.ins().uextend(types::I64, cursor);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        handle_value,
        address,
        i32::try_from(constants::HEAP_OBJECT_GC_WORD_OFFSET).expect("GC word offset fits i32"),
    );
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_value, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let object_bits = cx.builder.ins().ishl_imm_u(top, 16);
    let stable_state = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(constants::HANDLE_STATE_STABLE_YOUNG));
    let entry_value = cx.builder.ins().bor(object_bits, stable_state);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), entry_value, entry_address);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        end,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let next_handle = cx.builder.ins().iadd_imm_u(cursor, 1);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        next_handle,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let tag = if array {
        value::TAG_ARRAY
    } else {
        value::TAG_OBJECT
    };
    let value_prefix = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes((value::BOX_BASE | (tag << 32)).to_ne_bytes()),
    );
    let encoded = cx.builder.ins().bor(handle_value, value_prefix);
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(encoded)]);

    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let slow_capacity = cx.builder.ins().iconst(types::I64, i64::from(capacity));
    let slow = cx.call(
        if array {
            NativeRuntimeOp::NewArray.id()
        } else {
            NativeRuntimeOp::NewObject.id()
        },
        &[slow_capacity],
        None,
    )?;
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(slow)]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    define_value_boxed(
        cx.builder,
        cx.variables,
        dest,
        cx.builder.block_params(merge_block)[0],
    )
}

pub(crate) fn object_template_meta_index(
    constants: &[Constant],
    template: ConstantId,
) -> Option<u32> {
    crate::template_meta::object_template_meta_index(constants, template)
}

pub(crate) fn emit_load_object_template_meta_word(
    cx: &mut LoweringCx<'_, '_>,
    meta_index: u32,
    word_index: u32,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let meta_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_template_meta_base))?,
    );
    let entry_offset = (u64::from(meta_index) * u64::from(constants::OBJECT_TEMPLATE_META_WORDS)
        + u64::from(word_index))
    .checked_mul(4)
    .context("object template meta offset overflows")?;
    let entry_offset =
        i64::try_from(entry_offset).context("object template meta offset exceeds i64")?;
    let address = cx.builder.ins().iadd_imm_s(meta_base, entry_offset);
    let word = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), address, 0);
    Ok(cx.builder.ins().uextend(types::I64, word))
}

/// 模板自有数据属性的编译期槽偏移：空 shape 按键序追加时 `slot_index == prop_index`。
pub(crate) fn template_value_slot_offset(prop_index: u32) -> Result<i32> {
    let scaled = u64::from(prop_index)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .context("template slot scale overflows")?;
    let offset = u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
        .checked_add(scaled)
        .context("template slot offset overflows")?;
    i32::try_from(offset).context("template slot offset exceeds i32")
}

pub(crate) fn template_hit_args(
    logical_addr: ir::Value,
    direct_store: Option<ir::Value>,
) -> Vec<ir::BlockArg> {
    let mut args = vec![ir::BlockArg::Value(logical_addr)];
    if let Some(flag) = direct_store {
        args.push(ir::BlockArg::Value(flag));
    }
    args
}

pub(crate) struct TemplateReceiver {
    pub(crate) handle_i32: ir::Value,
    pub(crate) heap_delta: ir::Value,
    pub(crate) barrier_disabled: ir::Value,
}

/// 模板对象：标签 / 句柄 / epoch / 烘焙 shape 命中后跳到 `hit_block`。
///
/// `store` 时 `hit_block` 额外接收 `direct_store: i8`（young + 未标记）。
pub(crate) fn emit_template_receiver_guard(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ValueId,
    meta_index: u32,
    hit_block: ir::Block,
    fallback_block: ir::Block,
    store: bool,
) -> Result<TemplateReceiver> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let ht_base = cx.ht_base;
    let barrier_state = cx.barrier_state;
    let is_boxed = emit_is_boxed_handle(cx.builder, obj);
    let tag = cx.builder.ins().ushr_imm_u(obj, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = cx.builder.ins().band(is_boxed, is_obj);

    let entry_block = cx.builder.create_block();
    let legacy_entry_block = cx.builder.create_block();
    let zgc_entry_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let receiver_assist_block = cx.builder.create_block();
    let shape_check_block = cx.builder.create_block();
    cx.builder.append_block_param(shape_check_block, types::I64);
    if store {
        cx.builder.append_block_param(shape_check_block, types::I8);
    }

    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], fallback_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle_idx = cx.builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let entry_state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        entry_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_entry_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_entry_block);
    cx.builder.seal_block(legacy_entry_block);
    let legacy_direct = store.then(|| cx.builder.ins().iconst(types::I8, 1));
    let legacy_args = template_hit_args(logical_addr, legacy_direct);
    cx.builder
        .ins()
        .brif(stable, shape_check_block, &legacy_args, fallback_block, &[]);

    cx.builder.switch_to_block(zgc_entry_block);
    cx.builder.seal_block(zgc_entry_block);
    let epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = cx.builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, zgc_fast_block, &[], receiver_assist_block, &[]);

    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    let zgc_direct = if store {
        let phase_addr = cx.builder.ins().iadd_imm_s(
            barrier_state,
            i64::try_from(offset_of!(NativeBarrierState, phase)).expect("phase offset fits i64"),
        );
        let phase = cx
            .builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), phase_addr);
        let marking = cx
            .builder
            .ins()
            .band_imm_u(phase, NATIVE_BARRIER_MARKING_MASK as i64);
        let marking_idle = cx
            .builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, marking, 0);
        let stable_young = cx.builder.ins().icmp_imm_u(
            ir::condcodes::IntCC::Equal,
            entry_state,
            i64::from(constants::HANDLE_STATE_STABLE_YOUNG),
        );
        Some(cx.builder.ins().band(marking_idle, stable_young))
    } else {
        increment_barrier_counter(
            cx.builder,
            barrier_state,
            offset_of!(NativeBarrierState, load_fast_events),
        );
        None
    };
    let fast_args = template_hit_args(logical_addr, zgc_direct);
    cx.builder.ins().jump(shape_check_block, &fast_args);

    cx.builder.switch_to_block(receiver_assist_block);
    cx.builder.seal_block(receiver_assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    let assist_direct = store.then(|| cx.builder.ins().iconst(types::I8, 0));
    let assist_args = template_hit_args(assisted, assist_direct);
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &assist_args,
        fallback_block,
        &[],
    );

    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let direct_store = store.then(|| cx.builder.block_params(shape_check_block)[1]);
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let baked_shape = emit_load_object_template_meta_word(cx, meta_index, 0)?;
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, baked_shape);
    let hit_args = template_hit_args(logical_addr, direct_store);
    cx.builder
        .ins()
        .brif(shape_match, hit_block, &hit_args, fallback_block, &[]);

    Ok(TemplateReceiver {
        handle_i32,
        heap_delta,
        barrier_disabled,
    })
}

/// 在新分配对象已知 value slot 上直写属性值（仅用于 unboxed 数字等无需 store barrier 的值）。
pub(crate) fn lower_create_data_property_fast(
    cx: &mut LoweringCx<'_, '_>,
    logical_addr: ir::Value,
    heap_delta: ir::Value,
    prop_index: u32,
    stored: ir::Value,
) -> Result<()> {
    let offset = template_value_slot_offset(prop_index)?;
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    Ok(())
}

/// 模板对象自有数据属性读：shape 命中后 `load [obj+imm]`，失配回落 fallback。
pub(crate) fn lower_get_template_prop_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    meta_index: u32,
    prop_index: u32,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);
    let receiver = emit_template_receiver_guard(
        cx,
        barrier_thunks,
        object,
        meta_index,
        hit_block,
        fallback_block,
        false,
    )?;
    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, receiver.heap_delta);
    let offset = template_value_slot_offset(prop_index)?;
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    define_value_boxed(cx.builder, cx.variables, dest, loaded)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}
