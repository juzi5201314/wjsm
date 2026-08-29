//! SetProp inline cache 快路径。

#![allow(unused_imports)]
use super::*;
use anyhow::Result;
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use std::mem::offset_of;
use wjsm_ir::{ValueId, constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

pub(crate) fn lower_set_prop_ic(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    value: ValueId,
    strict: bool,
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
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
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

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在本块计算
    // 以支配所有后续分支（miss 分支需要它作为 SetPropIc 的回填目标）。
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
    cx.builder.append_block_param(shape_check_block, types::I8);
    let hit_block = cx.builder.create_block();
    let zgc_store_mode_block = cx.builder.create_block();
    let legacy_store_block = cx.builder.create_block();
    let zgc_direct_store_block = cx.builder.create_block();
    let scalar_elide_block = cx.builder.create_block();
    let barrier_store_block = cx.builder.create_block();
    let store_done_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

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
    let state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );

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
    let kind_own = ic_kind_is_own_hit(cx.builder, ic_kind, trio_field);
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
    let legacy_ok = cx.builder.ins().band(stable, kind_own);
    let direct_store = cx.builder.ins().iconst(types::I8, 1);
    cx.builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_kind_block);
    cx.builder.seal_block(zgc_kind_block);
    cx.builder
        .ins()
        .brif(kind_own, zgc_entry_block, &[], miss_block, &[]);

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
    let direct_resolve = cx.builder.ins().band(stable, epoch_even);
    cx.builder.ins().brif(
        direct_resolve,
        zgc_fast_block,
        &[],
        receiver_assist_block,
        &[],
    );

    // IC 命中且 access epoch 为偶：对象地址稳定，可尝试跳过 store barrier thunk。
    // 引用写入仍受 SATB / remset / 着色约束，因此这里只预计算「young + 未标记」
    // 直写；number 等非 box 槽在命中后再与旧 word 一起判定。
    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
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
        state,
        i64::from(constants::HANDLE_STATE_STABLE_YOUNG),
    );
    let direct_store = cx.builder.ins().band(marking_idle, stable_young);
    cx.builder.ins().jump(
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
    );

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
    let no_direct_store = cx.builder.ins().iconst(types::I8, 0);
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(assisted),
            ir::BlockArg::Value(no_direct_store),
        ],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let direct_store = cx.builder.block_params(shape_check_block)[1];
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
        .brif(shape_match, hit_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let value_offset = cx
        .builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let logical_slot = cx.builder.ins().iadd(logical_addr, value_offset);
    let value_addr = cx.builder.ins().iadd(addr, value_offset);
    cx.builder.ins().brif(
        barrier_disabled,
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

    // 偶数 epoch 下的标量直写：新旧 word 都不是 NaN-box 时 SATB/Mark/remset
    // 均为空操作，晋升后的长寿对象（property-key 的 RECORD）也能跳过 thunk。
    cx.builder.switch_to_block(scalar_elide_block);
    cx.builder.seal_block(scalar_elide_block);
    let stored_unboxed = emit_unboxed_nanbox_predicate(cx.builder, stored);
    let old = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0);
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
        .store(MemFlagsData::trusted(), stored, value_addr, 0);
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(zgc_direct_store_block);
    cx.builder.seal_block(zgc_direct_store_block);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), stored, value_addr);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(barrier_store_block);
    cx.builder.seal_block(barrier_store_block);
    let call = cx.builder.ins().call(
        barrier_thunks.store,
        &[cx.ctx, handle_i32, logical_slot, stored],
    );
    let status = cx.builder.inst_results(call)[0];
    let stored_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, status, 0);
    cx.builder
        .ins()
        .brif(stored_ok, store_done_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(store_done_block);
    cx.builder.seal_block(store_done_block);
    define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    cx.builder.ins().jump(merge_block, &[]);

    // miss：宿主完整 [[Set]] + IC 回填；`ic_ptr` 作为回填目标传入。
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let set_prop_ic_op = if strict {
        NativeRuntimeOp::SetPropIcStrict
    } else {
        NativeRuntimeOp::SetPropIc
    };
    let result = cx.call(set_prop_ic_op.id(), &[obj, key_value, stored, ic_ptr], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}
