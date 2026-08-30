//! GetProp / SetProp inline cache 快路径。

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;
use std::mem::offset_of;
use wjsm_ir::{Builtin, Constant, ValueId, constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

/// 常量字符串键的 GetProp 快路径入口：创建 merge 块后交给共享的非 nullish
/// IC 核心。
pub(crate) fn lower_get_prop_ic(
    cx: &mut LoweringCx<'_, '_>,
    tables: &mut InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    roots: &[ValueId],
) -> Result<()> {
    let merge_block = cx.builder.create_block();
    lower_get_prop_ic_non_nullish(cx, tables, barrier_thunks, access, roots, merge_block)?;
    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// GetProp IC 的共享核心。命中路径有三条：
/// - OWN_DATA：接收者 shape 命中后单 load 值槽；
/// - PROTO_DATA：接收者 shape + proto 世代命中后，从 holder 值槽 load；
/// - ACCESSOR：接收者 shape + proto 世代命中后 load getter；若 IC 槽记录了
///   hypot 双槽下标且 getter 仍是编译期识别的 `TAG_FUNCTION`，则直读接收者
///   槽并调用 typed `Math.hypot` thunk，否则 `invoke_callable`。
///
/// 其余情况 miss 到 `GetPropIc` 走完整宿主 [[Get]] 并回填。
pub(crate) fn lower_get_prop_ic_non_nullish(
    cx: &mut LoweringCx<'_, '_>,
    tables: &mut InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    roots: &[ValueId],
    merge_block: ir::Block,
) -> Result<()> {
    let PropAccess {
        dest,
        object,
        key,
        slot,
        trio_field,
    } = access;
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let key_value = use_value_boxed(cx.builder, cx.variables, key)?;
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let ht_base = cx.ht_base;
    let ic_base = cx.ic_base;
    let barrier_state = cx.barrier_state;

    // 标签检查：仅 NaN-box 的 TAG_OBJECT 才可解句柄读 entry。boxed 判定并入 SSO
    // marker 位，避免 inline 字符串（BOX_BASE + 载荷伪造 tag）被误判成对象句柄。
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

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在入口块计算
    // 以支配所有后续分支（miss 分支需要它作为 GetPropIc 的回填目标）。
    let ic_ptr = cx.builder.ins().iadd_imm_s(
        ic_base,
        i64::from(slot) * i64::from(constants::IC_SLOT_SIZE),
    );

    let entry_block = cx.builder.create_block();
    let legacy_entry_block = cx.builder.create_block();
    let zgc_kind_block = cx.builder.create_block();
    let zgc_entry_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let receiver_assist_block = cx.builder.create_block();
    let shape_check_block = cx.builder.create_block();
    cx.builder.append_block_param(shape_check_block, types::I64);
    let shape_hit_block = cx.builder.create_block();
    let own_hit_block = cx.builder.create_block();
    let holder_check_block = cx.builder.create_block();
    let holder_block = cx.builder.create_block();
    let holder_resolve_block = cx.builder.create_block();
    let holder_legacy_block = cx.builder.create_block();
    let holder_zgc_block = cx.builder.create_block();
    let holder_fast_block = cx.builder.create_block();
    let holder_assist_block = cx.builder.create_block();
    let holder_addr_block = cx.builder.create_block();
    cx.builder.append_block_param(holder_addr_block, types::I64);
    let proto_hit_block = cx.builder.create_block();
    let accessor_hit_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    // 第一级：标签必须是 TAG_OBJECT。**句柄表 entry 读取必须放在此分支之后**：
    // `trusted()`（notrap）load 允许 Cranelift 块内投机提前，若 entry 读取与
    // tag 检查同块，非对象值（字符串等）的 handle 可能落在未提交的 block，
    // 投机读取直接段错误。条件分支隔离后跨块提升不合法，entry 只在
    // `tag_ok` 为真（对象句柄必然已分配提交）后才读取。
    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

    // 第二级：读取接收者句柄 entry。Disabled 模式沿用稳定态快链；ZGC 只有偶数
    // access epoch 与稳定 entry 能直接使用地址，其余状态进入 no-GC load assist。
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
    // IC 槽（32 字节）：
    // word0 = shape_id(lo32) | value_index(hi32)
    // word1 = kind(lo32) | proto_generation(hi32)
    // word2 = holder_handle(lo32) | expected_proto(hi32)
    let ic_word0 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 0);
    let ic_shape = cx.builder.ins().band_imm_u(ic_word0, i64::from(u32::MAX));
    let ic_val_idx = load_ic_value_index(cx.builder, ic_ptr, ic_word0, trio_field);
    let ic_word1 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 8);
    let ic_kind = cx.builder.ins().band_imm_u(ic_word1, i64::from(u32::MAX));
    let ic_generation = cx.builder.ins().ushr_imm_u(ic_word1, 32);
    let ic_word2 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 16);
    let ic_holder = cx.builder.ins().band_imm_u(ic_word2, i64::from(u32::MAX));
    let ic_expected_proto = cx.builder.ins().ushr_imm_u(ic_word2, 32);
    let kind_own = ic_kind_is_own_hit(cx.builder, ic_kind, trio_field);
    let kind_proto = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_PROTO_DATA),
    );
    let kind_accessor = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_ACCESSOR),
    );
    let kind_holder = cx.builder.ins().bor(kind_proto, kind_accessor);
    let kind_supported = cx.builder.ins().bor(kind_own, kind_holder);
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_kind_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_entry_block);
    cx.builder.seal_block(legacy_entry_block);
    let legacy_ok = cx.builder.ins().band(stable, kind_supported);
    cx.builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[ir::BlockArg::Value(logical_addr)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_kind_block);
    cx.builder.seal_block(zgc_kind_block);
    cx.builder
        .ins()
        .brif(kind_supported, zgc_entry_block, &[], miss_block, &[]);

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
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(shape_check_block, &[ir::BlockArg::Value(logical_addr)]);

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
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    // 第三级：对象地址已经过稳定态检查或 load assist，读取 shape 并与 IC 槽比对。
    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, ic_shape);
    cx.builder
        .ins()
        .brif(shape_match, shape_hit_block, &[], miss_block, &[]);

    // shape 命中后按 kind 分派：OWN_DATA 直达自有值槽；PROTO_DATA / ACCESSOR 先校验直接原型与世代；其余走 miss。
    cx.builder.switch_to_block(shape_hit_block);
    cx.builder.seal_block(shape_hit_block);
    cx.builder
        .ins()
        .brif(kind_own, own_hit_block, &[], holder_check_block, &[]);

    cx.builder.switch_to_block(holder_check_block);
    cx.builder.seal_block(holder_check_block);
    cx.builder
        .ins()
        .brif(kind_holder, holder_block, &[], miss_block, &[]);

    // ProtoData / Accessor：同一 shape 的 receiver 可以有不同直接原型，故先比较
    // 对象头里的 proto handle；再比较原型世代以覆盖链上属性或原型变化。
    cx.builder.switch_to_block(holder_block);
    cx.builder.seal_block(holder_block);
    let receiver_header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 0);
    let receiver_proto = cx
        .builder
        .ins()
        .band_imm_u(receiver_header, i64::from(u32::MAX));
    let proto_match = cx.builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        receiver_proto,
        ic_expected_proto,
    );
    let current_generation = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, proto_generation))?,
    );
    let current_generation = cx.builder.ins().uextend(types::I64, current_generation);
    let generation_match = cx.builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        current_generation,
        ic_generation,
    );
    let holder_valid = cx.builder.ins().band(proto_match, generation_match);
    cx.builder
        .ins()
        .brif(holder_valid, holder_resolve_block, &[], miss_block, &[]);

    // 解析 holder_handle → holder entry → holder 地址；ZGC holder 与 receiver 使用
    // 同一 access epoch 协议，odd epoch 或 relocating entry 必须进入 load assist。
    cx.builder.switch_to_block(holder_resolve_block);
    cx.builder.seal_block(holder_resolve_block);
    let holder_entry_offset = cx.builder.ins().ishl_imm_u(ic_holder, 3);
    let holder_entry_addr = cx.builder.ins().iadd(ht_base, holder_entry_offset);
    let holder_entry =
        cx.builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), holder_entry_addr, 0);
    let holder_state = cx.builder.ins().band_imm_u(holder_entry, 0xFFFF);
    let holder_stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        holder_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let holder_logical_addr = cx.builder.ins().ushr_imm_u(holder_entry, 16);
    cx.builder.ins().brif(
        barrier_disabled,
        holder_legacy_block,
        &[],
        holder_zgc_block,
        &[],
    );

    cx.builder.switch_to_block(holder_legacy_block);
    cx.builder.seal_block(holder_legacy_block);
    cx.builder.ins().brif(
        holder_stable,
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(holder_zgc_block);
    cx.builder.seal_block(holder_zgc_block);
    let holder_epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let holder_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), holder_epoch_addr);
    let holder_epoch_bit = cx.builder.ins().band_imm_u(holder_epoch, 1);
    let holder_epoch_even =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, holder_epoch_bit, 0);
    let holder_direct = cx.builder.ins().band(holder_stable, holder_epoch_even);
    cx.builder.ins().brif(
        holder_direct,
        holder_fast_block,
        &[],
        holder_assist_block,
        &[],
    );

    cx.builder.switch_to_block(holder_fast_block);
    cx.builder.seal_block(holder_fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder.ins().jump(
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
    );

    cx.builder.switch_to_block(holder_assist_block);
    cx.builder.seal_block(holder_assist_block);
    let holder_i32 = cx.builder.ins().ireduce(types::I32, ic_holder);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, holder_i32]);
    let assisted_holder = cx.builder.inst_results(call)[0];
    let assisted_holder_ok =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted_holder, 0);
    cx.builder.ins().brif(
        assisted_holder_ok,
        holder_addr_block,
        &[ir::BlockArg::Value(assisted_holder)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(holder_addr_block);
    cx.builder.seal_block(holder_addr_block);
    let holder_logical_addr = cx.builder.block_params(holder_addr_block)[0];
    let holder_addr = cx.builder.ins().iadd(holder_logical_addr, heap_delta);
    cx.builder
        .ins()
        .brif(kind_accessor, accessor_hit_block, &[], proto_hit_block, &[]);

    // OWN_DATA 命中：`HEAP_OBJECT_HEADER_SIZE + value_index * 8` 处单 load。
    cx.builder.switch_to_block(own_hit_block);
    cx.builder.seal_block(own_hit_block);
    let value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3); // × 值槽 8 字节
    let value_offset = cx
        .builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let value_addr = cx.builder.ins().iadd(addr, value_offset);
    let value = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0);
    define_value_boxed(cx.builder, cx.variables, dest, value)?;
    emit_feedback_shape_store(cx, obj_shape, ic_val_idx);
    cx.builder.ins().jump(merge_block, &[]);

    // PROTO_DATA 命中：从 holder 对象的值槽 load。
    cx.builder.switch_to_block(proto_hit_block);
    cx.builder.seal_block(proto_hit_block);
    let proto_value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let proto_value_offset = cx.builder.ins().iadd_imm_s(
        proto_value_shift,
        i64::from(constants::HEAP_OBJECT_HEADER_SIZE),
    );
    let proto_value_addr = cx.builder.ins().iadd(holder_addr, proto_value_offset);
    let proto_value =
        cx.builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), proto_value_addr, 0);
    define_value_boxed(cx.builder, cx.variables, dest, proto_value)?;
    cx.builder.ins().jump(merge_block, &[]);

    // ACCESSOR 命中：load getter 后优先走 hypot 快路径（CLIF 比较 function_id +
    // 双槽直读 + typed thunk），否则宿主 invoke_callable。getter 是刚从堆里
    // 读出的临时句柄，invoke 路径必须作为临时 root 发布后再发起可能触发 GC 的调用。
    cx.builder.switch_to_block(accessor_hit_block);
    cx.builder.seal_block(accessor_hit_block);
    let getter_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let getter_offset = cx
        .builder
        .ins()
        .iadd_imm_s(getter_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let getter_addr = cx.builder.ins().iadd(holder_addr, getter_offset);
    let getter = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), getter_addr, 0);
    lower_accessor_invoke_or_hypot(
        cx,
        tables,
        dest,
        obj,
        getter,
        addr,
        ic_ptr,
        key,
        roots,
        merge_block,
    )?;

    // miss：宿主完整 [[Get]] + IC 回填；`ic_ptr` 作为回填目标传入。
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        NativeRuntimeOp::GetPropIc.id(),
        &[obj, key_value, ic_ptr],
        cx.feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    Ok(())
}

fn hypot_fast_path_for_key(tables: &InstructionTables<'_>, key: ValueId) -> bool {
    let Some(constant_id) = tables.constant_defs.get(&key) else {
        return false;
    };
    matches!(
        tables.constants.get(constant_id.0 as usize),
        Some(Constant::String(name)) if tables.hypot_property_names.contains(name)
    )
}

fn load_own_data_slot(
    builder: &mut FunctionBuilder<'_>,
    addr: ir::Value,
    index: ir::Value,
) -> ir::Value {
    let shift = builder.ins().ishl_imm_u(index, 3);
    let offset = builder
        .ins()
        .iadd_imm_s(shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let value_addr = builder.ins().iadd(addr, offset);
    builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0)
}

fn emit_is_function(builder: &mut FunctionBuilder<'_>, encoded: ir::Value) -> ir::Value {
    let is_boxed = emit_is_boxed_handle(builder, encoded);
    let tag = builder.ins().ushr_imm_u(encoded, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_fn = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_FUNCTION).expect("function tag fits i64"),
    );
    builder.ins().band(is_boxed, is_fn)
}

/// ACCESSOR 命中：hypot getter 走 CLIF 双槽直读 + typed thunk，其余 invoke。
fn lower_accessor_invoke_or_hypot(
    cx: &mut LoweringCx<'_, '_>,
    tables: &mut InstructionTables<'_>,
    dest: ValueId,
    obj: ir::Value,
    getter: ir::Value,
    addr: ir::Value,
    ic_ptr: ir::Value,
    key: ValueId,
    roots: &[ValueId],
    merge_block: ir::Block,
) -> Result<()> {
    if !hypot_fast_path_for_key(tables, key) {
        cx.publish_roots(roots, &[getter])?;
        let result = cx.call(NativeRuntimeOp::GetPropAccessor.id(), &[getter, obj], None)?;
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
        cx.builder.ins().jump(merge_block, &[]);
        return Ok(());
    }

    let hypot_block = cx.builder.create_block();
    let hypot_num_block = cx.builder.create_block();
    let invoke_block = cx.builder.create_block();

    let packed = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ic_ptr,
        i32::try_from(constants::IC_SLOT_RESERVED1_OFFSET).expect("reserved1 offset fits i32"),
    );
    let packed64 = cx.builder.ins().uextend(types::I64, packed);
    let expected_fn = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ic_ptr,
        i32::try_from(constants::IC_SLOT_RESERVED2_OFFSET).expect("reserved2 offset fits i32"),
    );
    let expected_fn = cx.builder.ins().uextend(types::I64, expected_fn);
    let packed_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, packed64, 0);
    let getter_plain = emit_strip_gc_color(cx.builder, getter);
    let is_fn = emit_is_function(cx.builder, getter_plain);
    let fn_idx = cx
        .builder
        .ins()
        .band_imm_u(getter_plain, i64::from(u32::MAX));
    let fn_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, fn_idx, expected_fn);
    let hypot_ok = cx.builder.ins().band(packed_ok, is_fn);
    let hypot_ok = cx.builder.ins().band(hypot_ok, fn_match);
    cx.builder
        .ins()
        .brif(hypot_ok, hypot_block, &[], invoke_block, &[]);

    cx.builder.switch_to_block(hypot_block);
    cx.builder.seal_block(hypot_block);
    let lhs_idx = cx.builder.ins().ushr_imm_u(packed64, 16);
    let rhs_idx = cx.builder.ins().band_imm_u(packed64, i64::from(u16::MAX));
    let lhs = load_own_data_slot(cx.builder, addr, lhs_idx);
    let rhs = load_own_data_slot(cx.builder, addr, rhs_idx);
    let lhs_num = emit_is_number(cx.builder, lhs);
    let rhs_num = emit_is_number(cx.builder, rhs);
    let both_num = cx.builder.ins().band(lhs_num, rhs_num);
    cx.builder
        .ins()
        .brif(both_num, hypot_num_block, &[], invoke_block, &[]);

    cx.builder.switch_to_block(hypot_num_block);
    cx.builder.seal_block(hypot_num_block);
    let thunk = import_math_thunk(
        cx.builder,
        tables.math_thunks,
        tables.imported_math_thunks,
        Builtin::MathHypot,
    )?;
    let lhs_f64 = unbox_f64(cx.builder, lhs);
    let rhs_f64 = unbox_f64(cx.builder, rhs);
    let call = cx.builder.ins().call(thunk, &[lhs_f64, rhs_f64]);
    let native = *cx
        .builder
        .inst_results(call)
        .first()
        .context("Math.hypot thunk 未返回结果")?;
    let boxed = box_f64_result(cx.builder, native);
    define_value_boxed(cx.builder, cx.variables, dest, boxed)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(invoke_block);
    cx.builder.seal_block(invoke_block);
    cx.publish_roots(roots, &[getter])?;
    let result = cx.call(NativeRuntimeOp::GetPropAccessor.id(), &[getter, obj], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}
