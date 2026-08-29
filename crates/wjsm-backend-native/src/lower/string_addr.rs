//! 字符串地址与码元

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

pub(crate) fn emit_inline_string_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed = builder.ins().band_imm_u(encoded, box_base);
    let boxed = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, boxed, box_base);
    let marker_bits = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_MARKER_MASK).expect("SSO marker mask fits i64"),
    );
    let is_ascii = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("ASCII SSO marker fits i64"),
    );
    let is_latin1 = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_LATIN1_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("Latin-1 SSO marker fits i64"),
    );
    let reserved = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_RESERVED_MASK).expect("SSO reserved mask fits i64"),
    );
    let reserved_zero = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, reserved, 0);
    let length = builder
        .ins()
        .ushr_imm_u(encoded, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let length = builder.ins().band_imm_u(length, 0b111);
    let length_ok = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        length,
        i64::try_from(value::INLINE_STRING_MAX_LEN).expect("SSO length fits i64"),
    );
    let ascii_ok = builder.ins().band(is_ascii, reserved_zero);
    let ascii_ok = builder.ins().band(ascii_ok, length_ok);
    let latin1_ok = builder.ins().band(is_latin1, length_ok);
    let kind_ok = builder.ins().bor(ascii_ok, latin1_ok);
    builder.ins().band(boxed, kind_ok)
}

/// 判定 NaN-box 之外的标量（热路径上几乎都是 number）。
///
/// 无 `BOX_BASE` 的值不是 handle-backed reference：着色是空操作，SATB / Mark /
/// remset 都不触发，IC 命中且 access epoch 为偶时可以跳过 store barrier thunk。
pub(crate) fn emit_unboxed_nanbox_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(encoded, box_base);
    builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, boxed_bits, box_base)
}

/// 把运行时字符串句柄解析为当前读取作用域内稳定的堆地址。
///
/// 每次调用都生成独立控制流；地址不跨块记忆，避免 ZGC epoch 变化后复用旧地址。
pub(crate) fn emit_string_address(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);

    let is_boxed = emit_is_boxed_handle(cx.builder, encoded);
    let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let runtime_flag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
    );
    let is_runtime = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
    let valid = cx.builder.ins().band(is_boxed, is_string);
    let valid = cx.builder.ins().band(valid, is_runtime);
    let inline = emit_inline_string_predicate(cx.builder, encoded);
    let inline = cx.builder.ins().bnot(inline);
    let valid = cx.builder.ins().band(valid, inline);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle);
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_address, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xffff);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_address = cx.builder.ins().ushr_imm_u(entry, 16);
    let barrier_state = cx.barrier_state;
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
        resolved_block,
        &[ir::BlockArg::Value(logical_address)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_address = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let epoch = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_address);
    let epoch_bit = cx.builder.ins().band_imm_u(epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

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
        resolved_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(resolved_block);
    cx.builder.seal_block(resolved_block);
    let logical_address = cx.builder.block_params(resolved_block)[0];
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    Ok(cx.builder.ins().iadd(logical_address, heap_delta))
}

/// 把运行时数组句柄解析为当前读取作用域内稳定的堆地址。
pub(crate) fn emit_array_address(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);

    let is_boxed = emit_is_boxed_handle(cx.builder, encoded);
    let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    let valid = cx.builder.ins().band(is_boxed, is_array);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle);
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_address, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xffff);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_address = cx.builder.ins().ushr_imm_u(entry, 16);
    let barrier_state = cx.barrier_state;
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
        resolved_block,
        &[ir::BlockArg::Value(logical_address)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_address = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let epoch = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_address);
    let epoch_bit = cx.builder.ins().band_imm_u(epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

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
        resolved_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(resolved_block);
    cx.builder.seal_block(resolved_block);
    let logical_address = cx.builder.block_params(resolved_block)[0];
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    Ok(cx.builder.ins().iadd(logical_address, heap_delta))
}

pub(crate) fn emit_nonnegative_integer_index(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> (ir::Value, ir::Value) {
    let is_number = emit_is_number(builder, encoded);
    let number = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), encoded);
    let index = builder.ins().fcvt_to_uint_sat(types::I64, number);
    let roundtrip = builder.ins().fcvt_from_uint(types::F64, index);
    let exact = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, number, roundtrip);
    let below_sentinel = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThan,
        index,
        i64::from(u32::MAX),
    );
    let valid = builder.ins().band(is_number, exact);
    (index, builder.ins().band(valid, below_sentinel))
}

pub(crate) fn emit_flat_string_code_unit(
    cx: &mut LoweringCx<'_, '_>,
    address: ir::Value,
    index: ir::Value,
    miss_block: ir::Block,
    out_of_bounds_block: ir::Block,
) -> ir::Value {
    let latin1_block = cx.builder.create_block();
    let utf16_block = cx.builder.create_block();
    let payload_block = cx.builder.create_block();
    cx.builder.append_block_param(payload_block, types::I64);

    let header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(header, i64::from(u32::MAX));
    let in_bounds = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, length);
    let repr_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, repr_block, &[], out_of_bounds_block, &[]);

    cx.builder.switch_to_block(repr_block);
    cx.builder.seal_block(repr_block);
    let first_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let repr = cx.builder.ins().ushr_imm_u(
        first_word,
        i64::from(constants::HEAP_STRING_REPR_OFFSET * 8),
    );
    let repr = cx.builder.ins().band_imm_u(repr, 0xff);
    let is_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let is_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let flat_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_latin1, latin1_block, &[], flat_block, &[]);

    cx.builder.switch_to_block(flat_block);
    cx.builder.seal_block(flat_block);
    cx.builder
        .ins()
        .brif(is_utf16, utf16_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(latin1_block);
    cx.builder.seal_block(latin1_block);
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    let unit_address = cx.builder.ins().iadd(payload, index);
    let unit = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), unit_address, 0);
    let unit = cx.builder.ins().uextend(types::I64, unit);
    cx.builder
        .ins()
        .jump(payload_block, &[ir::BlockArg::Value(unit)]);

    cx.builder.switch_to_block(utf16_block);
    cx.builder.seal_block(utf16_block);
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    let byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let unit_address = cx.builder.ins().iadd(payload, byte_offset);
    let unit = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), unit_address, 0);
    let unit = cx.builder.ins().uextend(types::I64, unit);
    cx.builder
        .ins()
        .jump(payload_block, &[ir::BlockArg::Value(unit)]);

    cx.builder.switch_to_block(payload_block);
    cx.builder.seal_block(payload_block);
    cx.builder.block_params(payload_block)[0]
}

pub(crate) fn emit_latin1_char_handle(
    cx: &mut LoweringCx<'_, '_>,
    unit: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let cached_block = cx.builder.create_block();
    let is_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        unit,
        i64::from(u8::MAX),
    );
    cx.builder
        .ins()
        .brif(is_latin1, cached_block, &[], miss_block, &[]);
    cx.builder.switch_to_block(cached_block);
    cx.builder.seal_block(cached_block);
    let table = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, latin1_char_strings))?,
    );
    let offset = cx.builder.ins().ishl_imm_u(unit, 3);
    let address = cx.builder.ins().iadd(table, offset);
    Ok(cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0))
}
pub(crate) fn emit_mixed_string_equal(
    cx: &mut LoweringCx<'_, '_>,
    left_address: ir::Value,
    right_address: ir::Value,
    length: ir::Value,
    left_latin1: ir::Value,
    right_latin1: ir::Value,
    result_blocks: (ir::Block, ir::Block),
) {
    let (equal_block, false_block) = result_blocks;
    let loop_block = cx.builder.create_block();
    let left_select_block = cx.builder.create_block();
    let left_utf16_block = cx.builder.create_block();
    let left_latin1_block = cx.builder.create_block();
    let left_latin_right_latin_block = cx.builder.create_block();
    let left_latin_right_utf16_block = cx.builder.create_block();
    let left_utf16_right_latin_block = cx.builder.create_block();
    let left_utf16_right_utf16_block = cx.builder.create_block();
    let units_block = cx.builder.create_block();
    cx.builder.append_block_param(loop_block, types::I64);
    cx.builder.append_block_param(units_block, types::I64);
    cx.builder.append_block_param(units_block, types::I64);

    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder
        .ins()
        .jump(loop_block, &[ir::BlockArg::Value(zero)]);
    cx.builder.switch_to_block(loop_block);
    let index = cx.builder.block_params(loop_block)[0];
    let done = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        index,
        length,
    );
    cx.builder
        .ins()
        .brif(done, equal_block, &[], left_select_block, &[]);
    cx.builder.switch_to_block(left_select_block);
    cx.builder.seal_block(left_select_block);
    let left_payload = cx.builder.ins().iadd_imm_s(
        left_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let right_payload = cx.builder.ins().iadd_imm_s(
        right_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let left_byte_offset = index;
    let left_word_offset = cx.builder.ins().ishl_imm_u(index, 1);
    cx.builder
        .ins()
        .brif(left_latin1, left_latin1_block, &[], left_utf16_block, &[]);

    cx.builder.switch_to_block(left_latin1_block);
    cx.builder.seal_block(left_latin1_block);
    cx.builder.ins().brif(
        right_latin1,
        left_latin_right_latin_block,
        &[],
        left_latin_right_utf16_block,
        &[],
    );

    cx.builder.switch_to_block(left_utf16_block);
    cx.builder.seal_block(left_utf16_block);
    cx.builder.ins().brif(
        right_latin1,
        left_utf16_right_latin_block,
        &[],
        left_utf16_right_utf16_block,
        &[],
    );

    cx.builder.switch_to_block(left_latin_right_latin_block);
    cx.builder.seal_block(left_latin_right_latin_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_byte_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_byte_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_latin_right_utf16_block);
    cx.builder.seal_block(left_latin_right_utf16_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_byte_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_word_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_utf16_right_latin_block);
    cx.builder.seal_block(left_utf16_right_latin_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_word_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_byte_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_utf16_right_utf16_block);
    cx.builder.seal_block(left_utf16_right_utf16_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_word_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_word_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(units_block);
    cx.builder.seal_block(units_block);
    let left = cx.builder.block_params(units_block)[0];
    let right = cx.builder.block_params(units_block)[1];
    let same = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left, right);
    let next_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same, next_block, &[], false_block, &[]);
    cx.builder.switch_to_block(next_block);
    cx.builder.seal_block(next_block);
    let index = cx.builder.block_params(loop_block)[0];
    let next_index = cx.builder.ins().iadd_imm_s(index, 1);
    cx.builder
        .ins()
        .jump(loop_block, &[ir::BlockArg::Value(next_index)]);
    cx.builder.seal_block(loop_block);
}
