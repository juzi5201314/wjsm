//! 模板/闩锁属性读写

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

pub(crate) fn lower_elem_shape_guard(
    cx: &mut LoweringCx<'_, '_>,
    constants: &[Constant],
    dest: ValueId,
    array: ValueId,
    template: ConstantId,
) -> Result<()> {
    let Some(meta_index) = object_template_meta_index(constants, template) else {
        bail!("elem_shape_guard template constant is invalid");
    };
    let array = use_value_boxed(cx.builder, cx.variables, array)?;
    let meta_index = cx.builder.ins().iconst(types::I64, i64::from(meta_index));
    let result = cx.call(
        NativeRuntimeOp::GuardElementsKind.id(),
        &[array, meta_index],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)
}

/// `GetProp` 闩锁快路径的操作数组。
pub(crate) struct GuardedPropAccess {
    pub(crate) dest: ValueId,
    pub(crate) object: ValueId,
    pub(crate) key: ValueId,
    pub(crate) guard: ValueId,
    pub(crate) template: ConstantId,
}

/// 守卫属性读：守卫为真时 receiver 已被 pre-header 的 `GuardElementsKind` 证明
/// 持有模板烘焙 shape，跳过逐迭代 tag/shape/proto 检查，解句柄后按模板槽
/// 偏移单指令直读；其余情况先把守卫值置 false（单向闩锁，宿主回退可能执行
/// 用户代码），再走与普通 `GetProp` 完全一致的 IC / 宿主路径。
pub(crate) fn lower_get_prop_guarded(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    access: GuardedPropAccess,
    roots: &[ValueId],
) -> Result<()> {
    let GuardedPropAccess {
        dest,
        object,
        key,
        guard,
        template,
    } = access;
    let prop_index =
        template_property_index_for_key(tables.constants, tables.constant_defs, template, key)
            .context("get_prop_guarded key must be a template own key")?;
    let offset = template_value_slot_offset(prop_index)?;

    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let guard_value = use_value_boxed(cx.builder, cx.variables, guard)?;
    let guard_on = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        guard_value,
        value::encode_bool(true),
    );
    cx.builder
        .ins()
        .brif(guard_on, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    emit_guarded_slot_read(
        cx,
        tables.barrier_thunks,
        dest,
        object,
        offset,
        merge_block,
        slow_block,
    )?;

    // 慢路径入口：先熄灭守卫再走通用路径（IC / 宿主可能执行用户代码）。
    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let disabled = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(false));
    define_value_boxed(cx.builder, cx.variables, guard, disabled)?;
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_get_prop_ic_non_nullish(
            cx,
            tables.barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            roots,
            merge_block,
        )?;
    } else {
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))?;
        cx.builder.ins().jump(merge_block, &[]);
    }

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// 守卫为真时的模板槽直读：不做 tag/shape 检查（pre-header 已一次性证明
/// receiver 是烘焙 shape 的普通对象），只保留句柄稳定态 / access epoch
/// 协议——GC 可能在循环回边 safepoint 重定位对象，句柄解析不是循环不变量。
/// 句柄表 entry 的 trusted load 必须留在守卫分支之后的独立块内，防止
/// Cranelift 把它投机提前到守卫为假（object 可能非对象）的路径上。
pub(crate) fn emit_guarded_slot_read(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    offset: i32,
    merge_block: ir::Block,
    slow_block: ir::Block,
) -> Result<()> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let ht_base = cx.ht_base;
    let barrier_state = cx.barrier_state;

    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);

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
    cx.builder
        .ins()
        .brif(barrier_disabled, legacy_block, &[], zgc_block, &[]);

    cx.builder.switch_to_block(legacy_block);
    cx.builder.seal_block(legacy_block);
    cx.builder.ins().brif(
        stable,
        hit_block,
        &[ir::BlockArg::Value(logical_addr)],
        slow_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
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
        .brif(direct, zgc_fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(hit_block, &[ir::BlockArg::Value(logical_addr)]);

    cx.builder.switch_to_block(assist_block);
    cx.builder.seal_block(assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    cx.builder.ins().brif(
        assisted_ok,
        hit_block,
        &[ir::BlockArg::Value(assisted)],
        slow_block,
        &[],
    );

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    define_value_boxed(cx.builder, cx.variables, dest, loaded)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

/// 模板对象自有数据属性写：shape 命中后 `store [obj+imm]`，失配回落 fallback。
pub(crate) fn lower_set_template_prop_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    value: ValueId,
    meta_index: u32,
    prop_index: u32,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);
    cx.builder.append_block_param(hit_block, types::I8);
    let receiver = emit_template_receiver_guard(
        cx,
        barrier_thunks,
        object,
        meta_index,
        hit_block,
        fallback_block,
        true,
    )?;
    emit_template_own_store(
        cx,
        barrier_thunks,
        dest,
        stored,
        receiver,
        prop_index,
        hit_block,
        merge_block,
        fallback_block,
    )
}

pub(crate) fn emit_template_own_store(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    stored: ir::Value,
    receiver: TemplateReceiver,
    prop_index: u32,
    hit_block: ir::Block,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let offset = template_value_slot_offset(prop_index)?;
    let legacy_store_block = cx.builder.create_block();
    let zgc_store_mode_block = cx.builder.create_block();
    let zgc_direct_store_block = cx.builder.create_block();
    let scalar_elide_block = cx.builder.create_block();
    let barrier_store_block = cx.builder.create_block();
    let store_done_block = cx.builder.create_block();

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let direct_store = cx.builder.block_params(hit_block)[1];
    let addr = cx.builder.ins().iadd(logical_addr, receiver.heap_delta);
    let logical_slot = cx.builder.ins().iadd_imm_s(logical_addr, i64::from(offset));
    let value_addr = cx.builder.ins().iadd_imm_s(addr, i64::from(offset));
    cx.builder.ins().brif(
        receiver.barrier_disabled,
        legacy_store_block,
        &[],
        zgc_store_mode_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_store_mode_block);
    cx.builder.seal_block(zgc_store_mode_block);
    cx.builder.ins().brif(
        direct_store,
        zgc_direct_store_block,
        &[],
        scalar_elide_block,
        &[],
    );

    cx.builder.switch_to_block(scalar_elide_block);
    cx.builder.seal_block(scalar_elide_block);
    let stored_unboxed = emit_unboxed_nanbox_predicate(cx.builder, stored);
    let old = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    let old_unboxed = emit_unboxed_nanbox_predicate(cx.builder, old);
    let scalar_direct = cx.builder.ins().band(stored_unboxed, old_unboxed);
    cx.builder.ins().brif(
        scalar_direct,
        zgc_direct_store_block,
        &[],
        barrier_store_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_store_block);
    cx.builder.seal_block(legacy_store_block);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(zgc_direct_store_block);
    cx.builder.seal_block(zgc_direct_store_block);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), stored, value_addr);
    increment_barrier_counter(
        cx.builder,
        cx.barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(barrier_store_block);
    cx.builder.seal_block(barrier_store_block);
    let call = cx.builder.ins().call(
        barrier_thunks.store,
        &[cx.ctx, receiver.handle_i32, logical_slot, stored],
    );
    let status = cx.builder.inst_results(call)[0];
    let stored_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, status, 0);
    cx.builder
        .ins()
        .brif(stored_ok, store_done_block, &[], fallback_block, &[]);

    cx.builder.switch_to_block(store_done_block);
    cx.builder.seal_block(store_done_block);
    define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

pub(crate) fn lower_get_prop_with_template_or_ic(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    roots: &[ValueId],
) -> Result<()> {
    let template_inline = tables.template_origins.get(&object).and_then(|site| {
        template_property_index_for_key(tables.constants, tables.constant_defs, site.template, key)
            .map(|prop_index| (site.meta_index, prop_index))
    });
    if let Some((meta_index, prop_index)) = template_inline {
        let merge_block = cx.builder.create_block();
        let fallback_block = cx.builder.create_block();
        lower_get_template_prop_inline(
            cx,
            barrier_thunks,
            dest,
            object,
            meta_index,
            prop_index,
            merge_block,
            fallback_block,
        )?;
        cx.builder.switch_to_block(fallback_block);
        cx.builder.seal_block(fallback_block);
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))?;
        cx.builder.ins().jump(merge_block, &[]);
        cx.builder.switch_to_block(merge_block);
        cx.builder.seal_block(merge_block);
        return Ok(());
    }
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_get_prop_ic(
            cx,
            barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            roots,
        )
    } else {
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))
    }
}

pub(crate) fn lower_set_prop_with_template_or_ic(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    value: ValueId,
    strict: bool,
) -> Result<()> {
    // strict 位只改变宿主 miss 路径的失败行为（基元接收者 TypeError vs no-op）；
    // CLIF 快路径仅命中真实对象自有数据属性，与 strict 无关。
    let set_prop_op = if strict {
        NativeRuntimeOp::SetPropStrict
    } else {
        NativeRuntimeOp::SetProp
    };
    let template_inline = tables.template_origins.get(&object).and_then(|site| {
        template_property_index_for_key(tables.constants, tables.constant_defs, site.template, key)
            .map(|prop_index| (site.meta_index, prop_index))
    });
    if let Some((meta_index, prop_index)) = template_inline {
        let merge_block = cx.builder.create_block();
        let fallback_block = cx.builder.create_block();
        lower_set_template_prop_inline(
            cx,
            barrier_thunks,
            dest,
            object,
            value,
            meta_index,
            prop_index,
            merge_block,
            fallback_block,
        )?;
        cx.builder.switch_to_block(fallback_block);
        cx.builder.seal_block(fallback_block);
        lower_value_operation(cx, set_prop_op, &[object, key, value], Some(dest))?;
        cx.builder.ins().jump(merge_block, &[]);
        cx.builder.switch_to_block(merge_block);
        cx.builder.seal_block(merge_block);
        return Ok(());
    }
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_set_prop_ic(
            cx,
            barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            value,
            strict,
        )
    } else {
        lower_value_operation(cx, set_prop_op, &[object, key, value], Some(dest))
    }
}

pub(crate) fn emit_init_object_literal_heap_value_guard(
    cx: &mut LoweringCx<'_, '_>,
    values: &[ValueId],
    slow_block: ir::Block,
) -> Result<()> {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let mut check_block = cx
        .builder
        .current_block()
        .context("init_object_literal guard requires an active block")?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            cx.builder.switch_to_block(check_block);
            cx.builder.seal_block(check_block);
        }
        let stored = use_value_boxed(cx.builder, cx.variables, *value)?;
        let boxed_bits = cx.builder.ins().band_imm_s(stored, box_base);
        let is_heap_value =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
        let next_block = cx.builder.create_block();
        cx.builder
            .ins()
            .brif(is_heap_value, slow_block, &[], next_block, &[]);
        check_block = next_block;
    }
    cx.builder.switch_to_block(check_block);
    cx.builder.seal_block(check_block);
    Ok(())
}

pub(crate) fn lower_init_object_literal(
    cx: &mut LoweringCx<'_, '_>,
    _tables: &BarrierThunks,
    constants: &[Constant],
    dest: ValueId,
    template: ConstantId,
    values: &[ValueId],
) -> Result<()> {
    let Some(meta_index) = object_template_meta_index(constants, template) else {
        bail!("init_object_literal template constant is invalid");
    };
    let Constant::ObjectTemplate { keys } = constants
        .get(usize::try_from(template.0).context("template index")?)
        .context("missing object template constant")?
    else {
        bail!("init_object_literal template constant is invalid");
    };
    if keys.len() != values.len() {
        bail!("init_object_literal value count mismatch");
    }
    let prop_count = u32::try_from(keys.len()).context("property count exceeds u32")?;

    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder.append_block_param(merge_block, types::I64);

    let meta_count = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_template_meta_count))?,
    );
    let meta_index_i32 = cx.builder.ins().iconst(types::I32, i64::from(meta_index));
    let meta_ready = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThan,
        meta_index_i32,
        meta_count,
    );
    cx.builder
        .ins()
        .brif(meta_ready, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);

    let shape_id = emit_load_object_template_meta_word(cx, meta_index, 0)?;
    let _slot_count = emit_load_object_template_meta_word(cx, meta_index, 1)?;
    let capacity = emit_load_object_template_meta_word(cx, meta_index, 2)?;

    let flags = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_fast_flags))?,
    );
    let enabled = cx.builder.ins().band_imm_u(
        flags,
        i64::from(wjsm_native_abi::NATIVE_ALLOCATION_FAST_OBJECT),
    );
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
    let slot_bytes = cx
        .builder
        .ins()
        .imul_imm_u(capacity, i64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE));
    let header_bytes = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let mut bytes_value = cx.builder.ins().iadd(slot_bytes, header_bytes);
    bytes_value = cx.builder.ins().iadd_imm_u(bytes_value, 7);
    bytes_value = cx.builder.ins().band_imm_s(bytes_value, !7);
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
    let mut ready = cx.builder.ins().band(enabled, small);
    ready = cx.builder.ins().band(ready, object_fits);
    ready = cx.builder.ins().band(ready, handle_fits);
    let fast_alloc_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(ready, fast_alloc_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_alloc_block);
    cx.builder.seal_block(fast_alloc_block);
    emit_init_object_literal_heap_value_guard(cx, values, slow_block)?;
    let prototype = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_prototype_handle))?,
    );
    let prototype = cx.builder.ins().uextend(types::I64, prototype);
    let type_word = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(u64::from(wjsm_ir::HEAP_TYPE_OBJECT) << 32).expect("heap type word fits i64"),
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
    let capacity_i32 = cx.builder.ins().ireduce(types::I32, capacity);
    let shape_i32 = cx.builder.ins().ireduce(types::I32, shape_id);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        capacity_i32,
        address,
        i32::try_from(constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET)
            .expect("capacity offset fits i32"),
    );
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        shape_i32,
        address,
        i32::try_from(constants::HEAP_OBJECT_SHAPE_ID_OFFSET).expect("shape offset fits i32"),
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
    let logical_addr = top;
    for property in 0..prop_count {
        let stored = use_value_boxed(
            cx.builder,
            cx.variables,
            values[usize::try_from(property).expect("property index fits usize")],
        )?;
        lower_create_data_property_fast(cx, logical_addr, heap_delta, property, stored)?;
    }
    let value_prefix = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes((value::BOX_BASE | (value::TAG_OBJECT << 32)).to_ne_bytes()),
    );
    let encoded = cx.builder.ins().bor(handle_value, value_prefix);
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(encoded)]);

    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let mut call_args = vec![cx.builder.ins().iconst(types::I64, i64::from(meta_index))];
    for value in values {
        call_args.push(use_value_boxed(cx.builder, cx.variables, *value)?);
    }
    let slow = cx.call(NativeRuntimeOp::InitObjectLiteral.id(), &call_args, None)?;
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
