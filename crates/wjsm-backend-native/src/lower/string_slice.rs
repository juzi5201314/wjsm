//! 字符串 slice / char

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

pub(crate) fn emit_inline_ascii_only_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let is_inline = emit_inline_string_predicate(builder, encoded);
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
    builder.ins().band(is_inline, is_ascii)
}

pub(crate) fn emit_extract_inline_ascii_unit(
    builder: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    index: ir::Value,
) -> ir::Value {
    let shift = builder.ins().ishl_imm_u(index, 3);
    let shift = builder.ins().isub(shift, index);
    let unit = builder.ins().ushr(receiver, shift);
    builder.ins().band_imm_u(unit, 0x7f)
}

pub(crate) fn emit_is_inline_latin1_marker(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let marker_bits = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_MARKER_MASK).expect("SSO marker mask fits i64"),
    );
    builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_LATIN1_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("Latin-1 SSO marker fits i64"),
    )
}

pub(crate) fn emit_extract_inline_latin1_unit(
    builder: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    index: ir::Value,
) -> ir::Value {
    let payload = builder.ins().band_imm_u(
        receiver,
        i64::try_from(value::INLINE_STRING_PAYLOAD_MASK).expect("SSO payload mask fits i64"),
    );
    let shift = builder.ins().ishl_imm_u(index, 3);
    let unit = builder.ins().ushr(payload, shift);
    builder.ins().band_imm_u(unit, 0xff)
}

pub(crate) fn emit_unsigned_min(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> ir::Value {
    let less = builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, lhs, rhs);
    builder.ins().select(less, lhs, rhs)
}

pub(crate) fn emit_unsigned_max(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> ir::Value {
    let greater = builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedGreaterThan, lhs, rhs);
    builder.ins().select(greater, lhs, rhs)
}

pub(crate) fn emit_relative_slice_index(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
    length: ir::Value,
) -> ir::Value {
    let number = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), encoded);
    let index = builder.ins().fcvt_to_sint_sat(types::I64, number);
    let negative = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::SignedLessThan, index, 0);
    let relative = builder.ins().iadd(length, index);
    let zero = builder.ins().iconst(types::I64, 0);
    let clamped_negative = emit_unsigned_max(builder, relative, zero);
    let clamped_positive = emit_unsigned_min(builder, index, length);
    builder
        .ins()
        .select(negative, clamped_negative, clamped_positive)
}

pub(crate) fn emit_pack_inline_ascii_slice(
    cx: &mut LoweringCx<'_, '_>,
    receiver: ir::Value,
    start: ir::Value,
    end: ir::Value,
) -> ir::Value {
    let result_len = cx.builder.ins().isub(end, start);
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    cx.builder.append_block_param(head, types::I64);
    let done = cx.builder.create_block();
    cx.builder.append_block_param(done, types::I64);
    let body = cx.builder.create_block();

    let base = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let marker = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("SSO marker fits i64"),
    );
    let base_payload = cx.builder.ins().bor(base, marker);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(
        head,
        &[ir::BlockArg::Value(zero), ir::BlockArg::Value(base_payload)],
    );

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let payload = cx.builder.block_params(head)[1];
    let finished = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        index,
        result_len,
    );
    cx.builder
        .ins()
        .brif(finished, done, &[ir::BlockArg::Value(payload)], body, &[]);

    cx.builder.switch_to_block(body);
    let src_index = cx.builder.ins().iadd(start, index);
    let unit = emit_extract_inline_ascii_unit(cx.builder, receiver, src_index);
    let shift = cx.builder.ins().ishl_imm_u(index, 3);
    let shift = cx.builder.ins().isub(shift, index);
    let shifted = cx.builder.ins().ishl(unit, shift);
    let merged = cx.builder.ins().bor(payload, shifted);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(
        head,
        &[ir::BlockArg::Value(next), ir::BlockArg::Value(merged)],
    );

    cx.builder.switch_to_block(done);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
    let payload = cx.builder.block_params(done)[0];
    let length_bits = cx
        .builder
        .ins()
        .ishl_imm_u(result_len, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    cx.builder.ins().bor(payload, length_bits)
}

pub(crate) fn lower_string_slice_builtin(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let receiver = use_value_boxed(cx.builder, cx.variables, args[0])?;
    let encoded_start = if let Some(start) = args.get(1) {
        use_value_boxed(cx.builder, cx.variables, *start)?
    } else {
        cx.builder.ins().iconst(types::I64, value::encode_f64(0.0))
    };
    let encoded_end = if let Some(end) = args.get(2) {
        Some(use_value_boxed(cx.builder, cx.variables, *end)?)
    } else {
        None
    };

    let ascii_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let is_ascii = emit_inline_ascii_only_predicate(cx.builder, receiver);
    cx.builder
        .ins()
        .brif(is_ascii, ascii_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(ascii_block);
    cx.builder.seal_block(ascii_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(receiver, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let start_is_number = emit_is_number(cx.builder, encoded_start);
    let end_is_number = if let Some(encoded_end) = encoded_end {
        emit_is_number(cx.builder, encoded_end)
    } else {
        cx.builder.ins().iconst(types::I8, 1)
    };
    let bounds_block = cx.builder.create_block();
    let bounds_ok = cx.builder.ins().band(start_is_number, end_is_number);
    cx.builder
        .ins()
        .brif(bounds_ok, bounds_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(bounds_block);
    cx.builder.seal_block(bounds_block);
    let start = emit_relative_slice_index(cx.builder, encoded_start, inline_length);
    let end = if let Some(encoded_end) = encoded_end {
        emit_relative_slice_index(cx.builder, encoded_end, inline_length)
    } else {
        inline_length
    };
    let empty_block = cx.builder.create_block();
    let slice_block = cx.builder.create_block();
    let end_before_start =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, end, start);
    cx.builder
        .ins()
        .brif(end_before_start, empty_block, &[], slice_block, &[]);

    cx.builder.switch_to_block(empty_block);
    cx.builder.seal_block(empty_block);
    let empty = cx.builder.ins().iconst(
        types::I64,
        value::encode_inline_ascii(b"").expect("empty inline ascii"),
    );
    define_value_boxed(cx.builder, cx.variables, dest, empty)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(slice_block);
    cx.builder.seal_block(slice_block);
    let sliced = emit_pack_inline_ascii_slice(cx, receiver, start, end);
    define_value_boxed(cx.builder, cx.variables, dest, sliced)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let mut call_args = vec![receiver, encoded_start];
    if let Some(end) = encoded_end {
        call_args.push(end);
    }
    let result = cx.call(
        u32::from(Builtin::StringSlice.wire_id()),
        &call_args,
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

pub(crate) fn emit_inline_ascii_char_value(
    cx: &mut LoweringCx<'_, '_>,
    unit: ir::Value,
) -> ir::Value {
    let base = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let marker = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("SSO marker fits i64"),
    );
    let length = cx
        .builder
        .ins()
        .iconst(types::I64, 1_i64 << value::INLINE_STRING_LENGTH_SHIFT);
    let result = cx.builder.ins().bor(base, marker);
    let result = cx.builder.ins().bor(result, length);
    let unit = cx.builder.ins().band_imm_u(unit, 0x7f);
    cx.builder.ins().bor(result, unit)
}

pub(crate) fn lower_string_char_builtin(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    builtin: Builtin,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let receiver = use_value_boxed(cx.builder, cx.variables, args[0])?;
    let encoded_index = if let Some(index) = args.get(1) {
        use_value_boxed(cx.builder, cx.variables, *index)?
    } else {
        cx.builder.ins().iconst(types::I64, value::encode_f64(0.0))
    };
    let (index, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let inline_string_block = cx.builder.create_block();
    let inline_char_block = cx.builder.create_block();
    let string_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let out_of_bounds_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let is_inline = emit_inline_string_predicate(cx.builder, receiver);
    cx.builder
        .ins()
        .brif(is_inline, inline_string_block, &[], string_block, &[]);

    cx.builder.switch_to_block(inline_string_block);
    cx.builder.seal_block(inline_string_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(receiver, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let in_bounds =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, inline_length);
    cx.builder
        .ins()
        .brif(in_bounds, inline_char_block, &[], out_of_bounds_block, &[]);

    cx.builder.switch_to_block(inline_char_block);
    cx.builder.seal_block(inline_char_block);
    let inline_latin1_char_block = cx.builder.create_block();
    let inline_ascii_char_block = cx.builder.create_block();
    let is_inline_latin1 = emit_is_inline_latin1_marker(cx.builder, receiver);
    cx.builder.ins().brif(
        is_inline_latin1,
        inline_latin1_char_block,
        &[],
        inline_ascii_char_block,
        &[],
    );

    cx.builder.switch_to_block(inline_ascii_char_block);
    cx.builder.seal_block(inline_ascii_char_block);
    let ascii_unit = emit_extract_inline_ascii_unit(cx.builder, receiver, index);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, ascii_unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_inline_ascii_char_value(cx, ascii_unit)
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(inline_latin1_char_block);
    cx.builder.seal_block(inline_latin1_char_block);
    let latin1_unit = emit_extract_inline_latin1_unit(cx.builder, receiver, index);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, latin1_unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_latin1_char_handle(cx, latin1_unit, miss_block)?
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(string_block);
    cx.builder.seal_block(string_block);
    let address = emit_string_address(cx, barrier_thunks, receiver, miss_block)?;
    let unit = emit_flat_string_code_unit(cx, address, index, miss_block, out_of_bounds_block);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_latin1_char_handle(cx, unit, miss_block)?
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(out_of_bounds_block);
    cx.builder.seal_block(out_of_bounds_block);
    if builtin == Builtin::StringCharCodeAt {
        let result = cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_f64(f64::NAN));
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
        cx.builder.ins().jump(merge_block, &[]);
    } else {
        cx.builder.ins().jump(miss_block, &[]);
    }

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        u32::from(builtin.wire_id()),
        &[receiver, encoded_index],
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

// ── 非逃逸字符串累加器的内联追加（阶段 3）──
//
// 快路径条件：current 是堆内 BUILDER、全部片段为 flat 字符串（数字片段要求
// 安全整数）、剩余容量充足，并且 entry 稳定 + ZGC 搬迁未激活（access epoch
// 为偶）。满足时直接把码元写入 payload 并就地更新 length，零宿主往返；容量
// 不足（增长走宿主搬迁）或任何守卫不满足时回落宿主 thunk，语义与未内联时
// 完全一致。并发标记期间照常直写：payload 与 length 属纯数据，标记器不扫描
// builder 载荷，宿主侧 `write_string_payload` 同样不因标记活跃而阻塞。

/// 写入路径专用的保守字符串地址解析。
///
/// 与 `emit_string_address` 的差异：不做 load assist。assist 之后并发搬迁仍
/// 可能与随后的直写竞争，因此只有在 entry 稳定且 ZGC 的 access epoch 为偶
/// （搬迁未激活）时才返回地址，其余一律进 miss 块。
pub(crate) fn emit_idle_string_address(
    cx: &mut LoweringCx<'_, '_>,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
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
    let not_inline = cx.builder.ins().bnot(inline);
    let valid = cx.builder.ins().band(valid, not_inline);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
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
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);
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
        .brif(direct, fast_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

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
