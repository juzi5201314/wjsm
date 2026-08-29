//! 字符串 builder 追加

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

pub(crate) fn emit_string_repr(builder: &mut FunctionBuilder<'_>, address: ir::Value) -> ir::Value {
    let first_word = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let repr = builder.ins().ushr_imm_u(
        first_word,
        i64::from(constants::HEAP_STRING_REPR_OFFSET * 8),
    );
    builder.ins().band_imm_u(repr, 0xff)
}

/// 内联追加的 builder 状态：对象地址、当前码元长度与字节容量。
pub(crate) struct InlineBuilderState {
    address: ir::Value,
    length: ir::Value,
    capacity: ir::Value,
}

/// 解析累加器 current 为 BUILDER repr 的堆对象并读出长度/容量；其余形态进
/// miss（首建 builder、flat 化后的再追加都由宿主处理）。
pub(crate) fn emit_inline_builder_state(
    cx: &mut LoweringCx<'_, '_>,
    current: ir::Value,
    miss_block: ir::Block,
) -> Result<InlineBuilderState> {
    let address = emit_idle_string_address(cx, current, miss_block)?;
    let repr = emit_string_repr(cx.builder, address);
    let is_builder = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_BUILDER),
    );
    let builder_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_builder, builder_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(builder_block);
    cx.builder.seal_block(builder_block);
    let length_capacity = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx
        .builder
        .ins()
        .band_imm_u(length_capacity, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(length_capacity, 32);
    Ok(InlineBuilderState {
        address,
        length,
        capacity,
    })
}

/// 已解析的 flat 字符串片段：payload 地址、是否 Latin-1、码元数。
pub(crate) struct InlineStringPart {
    payload: ir::Value,
    is_latin1: ir::Value,
    units: ir::Value,
}

/// 解析字符串片段；仅 Latin-1/UTF-16 flat 直拷，Cons/Slice/builder 片段进 miss。
pub(crate) fn emit_inline_string_part(
    cx: &mut LoweringCx<'_, '_>,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<InlineStringPart> {
    let address = emit_idle_string_address(cx, encoded, miss_block)?;
    let repr = emit_string_repr(cx.builder, address);
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
    let flat = cx.builder.ins().bor(is_latin1, is_utf16);
    let flat_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(flat, flat_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(flat_block);
    cx.builder.seal_block(flat_block);
    let length_word = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let units = cx
        .builder
        .ins()
        .band_imm_u(length_word, i64::from(u32::MAX));
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    Ok(InlineStringPart {
        payload,
        is_latin1,
        units,
    })
}

/// UTF-16 flat 片段 → builder payload 的逐码元拷贝循环。
pub(crate) fn emit_copy_utf16_part(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
    done_block: ir::Block,
) {
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(zero)]);

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let more = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, part.units);
    let body = cx.builder.create_block();
    cx.builder.ins().brif(more, body, &[], done_block, &[]);

    cx.builder.switch_to_block(body);
    let byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let src = cx.builder.ins().iadd(part.payload, byte_offset);
    let unit = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), src, 0);
    let dst_unit = cx.builder.ins().iadd(dst, byte_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, dst_unit, 0);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(next)]);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
}

/// Latin-1 flat 片段 → builder UTF-16 payload 的逐码元加宽拷贝循环。
pub(crate) fn emit_copy_latin1_part(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
    done_block: ir::Block,
) {
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(zero)]);

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let more = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, part.units);
    let body = cx.builder.create_block();
    cx.builder.ins().brif(more, body, &[], done_block, &[]);

    cx.builder.switch_to_block(body);
    let src = cx.builder.ins().iadd(part.payload, index);
    let unit = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), src, 0);
    let unit = cx.builder.ins().uextend(types::I16, unit);
    let dst_byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let dst_unit = cx.builder.ins().iadd(dst, dst_byte_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, dst_unit, 0);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(next)]);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
}

/// 按片段表示分派拷贝循环，返回继续块。
pub(crate) fn emit_copy_part_dispatch(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
) -> ir::Block {
    let done = cx.builder.create_block();
    let latin1_head = cx.builder.create_block();
    let utf16_head = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(part.is_latin1, latin1_head, &[], utf16_head, &[]);

    cx.builder.switch_to_block(latin1_head);
    cx.builder.seal_block(latin1_head);
    emit_copy_latin1_part(cx, part, dst, done);

    cx.builder.switch_to_block(utf16_head);
    cx.builder.seal_block(utf16_head);
    emit_copy_utf16_part(cx, part, dst, done);

    cx.builder.switch_to_block(done);
    cx.builder.seal_block(done);
    done
}

/// `0 ≤ magnitude ≤ 2^53-1` 的十进制位数（1..=16）：对 10 的幂做比较阶梯，
/// 无除法。
pub(crate) fn emit_decimal_digit_count(
    builder: &mut FunctionBuilder<'_>,
    magnitude: ir::Value,
) -> ir::Value {
    let mut digits = builder.ins().iconst(types::I64, 1);
    for exponent in 1..=15 {
        let threshold = builder.ins().iconst(
            types::I64,
            10_i64.pow(u32::try_from(exponent).expect("≤15")),
        );
        let reached = builder.ins().icmp(
            ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
            magnitude,
            threshold,
        );
        let count = builder.ins().iconst(types::I64, i64::from(exponent) + 1);
        digits = builder.ins().select(reached, count, digits);
    }
    digits
}

/// 宿主回落的统一出口：dispatcher 承载全部通用语义（builder 首建、增长、
/// 非安全整数格式化、非字符串片段）。
pub(crate) fn emit_string_builder_append_miss(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ir::Value],
    feedback_ptr: Option<ir::Value>,
    miss_block: ir::Block,
    merge_block: ir::Block,
) -> Result<()> {
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        u32::from(Builtin::StringBuilderAppend.wire_id()),
        args,
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

/// 内联更新 builder length（`+8` word 低 32 位，高 32 位 capacity 不动）。
pub(crate) fn emit_store_builder_length(
    cx: &mut LoweringCx<'_, '_>,
    builder: &InlineBuilderState,
    total_units: ir::Value,
) {
    let length = cx.builder.ins().ireduce(types::I32, total_units);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        length,
        builder.address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
}

/// `string + f64` 片段的数字位写入:负号、位数阶梯与两位一组的 itoa 反向
/// 写入。调用后当前块保持打开,由调用方收尾。
pub(crate) fn emit_append_number_digits(
    cx: &mut LoweringCx<'_, '_>,
    payload_base: ir::Value,
    start_units: ir::Value,
    negative: ir::Value,
    magnitude: ir::Value,
    digits: ir::Value,
    len_store_block: ir::Block,
) {
    let minus_block = cx.builder.create_block();
    let digits_entry = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(negative, minus_block, &[], digits_entry, &[]);

    cx.builder.switch_to_block(minus_block);
    cx.builder.seal_block(minus_block);
    let minus_offset_bytes = cx.builder.ins().ishl_imm_u(start_units, 1);
    let minus_address = cx.builder.ins().iadd(payload_base, minus_offset_bytes);
    let minus = cx.builder.ins().iconst(types::I16, i64::from(b'-'));
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), minus, minus_address, 0);
    cx.builder.ins().jump(digits_entry, &[]);

    cx.builder.switch_to_block(digits_entry);
    cx.builder.seal_block(digits_entry);
    let write_pos = cx.builder.ins().iadd(start_units, digits);
    let digit_loop = cx.builder.create_block();
    cx.builder.append_block_param(digit_loop, types::I64);
    cx.builder.append_block_param(digit_loop, types::I64);
    let is_zero = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, magnitude, 0);
    let zero_block = cx.builder.create_block();
    cx.builder.ins().brif(
        is_zero,
        zero_block,
        &[],
        digit_loop,
        &[
            ir::BlockArg::Value(magnitude),
            ir::BlockArg::Value(write_pos),
        ],
    );

    cx.builder.switch_to_block(zero_block);
    cx.builder.seal_block(zero_block);
    let zero_char = cx.builder.ins().iconst(types::I16, i64::from(b'0'));
    let zero_pos = cx.builder.ins().iadd_imm_u(write_pos, -1);
    let zero_offset = cx.builder.ins().ishl_imm_u(zero_pos, 1);
    let zero_address = cx.builder.ins().iadd(payload_base, zero_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), zero_char, zero_address, 0);
    cx.builder.ins().jump(len_store_block, &[]);

    cx.builder.switch_to_block(digit_loop);
    let m = cx.builder.block_params(digit_loop)[0];
    let pos = cx.builder.block_params(digit_loop)[1];
    let done = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, m, 0);
    let leading_block = cx.builder.create_block();
    let single_block = cx.builder.create_block();
    let pair_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(done, len_store_block, &[], leading_block, &[]);

    cx.builder.switch_to_block(leading_block);
    cx.builder.seal_block(leading_block);
    let leading = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::UnsignedLessThan, m, 10);
    cx.builder
        .ins()
        .brif(leading, single_block, &[], pair_block, &[]);

    cx.builder.switch_to_block(pair_block);
    cx.builder.seal_block(pair_block);
    let high = cx.builder.ins().udiv_imm_u(m, 100);
    let pair = cx.builder.ins().urem_imm_u(m, 100);
    let tens = cx.builder.ins().udiv_imm_u(pair, 10);
    let ones = cx.builder.ins().urem_imm_u(pair, 10);
    emit_store_digit(cx, payload_base, pos, -1, ones);
    emit_store_digit(cx, payload_base, pos, -2, tens);
    let next_pos = cx.builder.ins().iadd_imm_u(pos, -2);
    cx.builder.ins().jump(
        digit_loop,
        &[ir::BlockArg::Value(high), ir::BlockArg::Value(next_pos)],
    );
    cx.builder.seal_block(digit_loop);

    cx.builder.switch_to_block(single_block);
    cx.builder.seal_block(single_block);
    emit_store_digit(cx, payload_base, pos, -1, m);
    cx.builder.ins().jump(len_store_block, &[]);
}

/// 在 payload 起算的 `pos + delta` 绝对码元位写入一个 '0' 起始的数字码元。
pub(crate) fn emit_store_digit(
    cx: &mut LoweringCx<'_, '_>,
    payload_base: ir::Value,
    pos: ir::Value,
    delta: i64,
    digit: ir::Value,
) {
    let at = cx.builder.ins().iadd_imm_u(pos, delta);
    let offset = cx.builder.ins().ishl_imm_u(at, 1);
    let address = cx.builder.ins().iadd(payload_base, offset);
    let ascii_zero = cx.builder.ins().iconst(types::I64, i64::from(b'0'));
    let unit = cx.builder.ins().iadd(digit, ascii_zero);
    let unit = cx.builder.ins().ireduce(types::I16, unit);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, address, 0);
}

/// 非逃逸累加器的内联追加:前缀片段必须为 flat 字符串;最后一个片段按运行时
/// 类型分派——字符串直拷、数字走安全整数 itoa,其余形态(对象/BigInt/非 flat
/// 数字、需要增长)回落宿主 thunk,语义与未内联时完全一致。
pub(crate) fn lower_string_builder_append(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(use_value_boxed(cx.builder, cx.variables, *arg)?);
    }
    let last = *values
        .last()
        .context("string builder append needs a part")?;
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();

    let builder_state = emit_inline_builder_state(cx, values[0], miss_block)?;
    let mut prefix_parts = Vec::with_capacity(values.len() - 2);
    for encoded in &values[1..values.len() - 1] {
        prefix_parts.push(emit_inline_string_part(cx, *encoded, miss_block)?);
    }

    // 最后片段:先按 flat 字符串解析,tag/repr 不符再进数字分派。
    let number_check_block = cx.builder.create_block();
    let last_part = emit_inline_string_part(cx, last, number_check_block)?;

    // ── 字符串路径:全部片段直拷。──
    let mut string_total = builder_state.length;
    for part in &prefix_parts {
        string_total = cx.builder.ins().iadd(string_total, part.units);
    }
    string_total = cx.builder.ins().iadd(string_total, last_part.units);
    let string_bytes = cx.builder.ins().ishl_imm_u(string_total, 1);
    let string_fits = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        string_bytes,
        builder_state.capacity,
    );
    let string_write_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(string_fits, string_write_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(string_write_block);
    cx.builder.seal_block(string_write_block);
    let payload_base = cx.builder.ins().iadd_imm_s(
        builder_state.address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let mut cursor_units = builder_state.length;
    for part in prefix_parts.iter().chain(std::iter::once(&last_part)) {
        let cursor_bytes = cx.builder.ins().ishl_imm_u(cursor_units, 1);
        let part_dst = cx.builder.ins().iadd(payload_base, cursor_bytes);
        let done = emit_copy_part_dispatch(cx, part, part_dst);
        cx.builder.switch_to_block(done);
        cursor_units = cx.builder.ins().iadd(cursor_units, part.units);
    }
    emit_store_builder_length(cx, &builder_state, string_total);
    define_value_boxed(cx.builder, cx.variables, dest, values[0])?;
    cx.builder.ins().jump(merge_block, &[]);

    // ── 数字路径:末片段是 Number 且为安全整数时内联 itoa。──
    cx.builder.switch_to_block(number_check_block);
    cx.builder.seal_block(number_check_block);
    let is_number = emit_is_number(cx.builder, last);
    let classify_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_number, classify_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(classify_block);
    cx.builder.seal_block(classify_block);
    // NaN/±Inf 超出安全整数范围(有序比较对 NaN 恒假),小数无法经 i64
    // roundtrip,全部回落宿主的完整 Number→String 语义。
    let number = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), last);
    let magnitude_f64 = cx.builder.ins().fabs(number);
    let bound = cx.builder.ins().f64const(9_007_199_254_740_991.0);
    let in_range = cx.builder.ins().fcmp(
        ir::condcodes::FloatCC::LessThanOrEqual,
        magnitude_f64,
        bound,
    );
    let as_int = cx.builder.ins().fcvt_to_sint_sat(types::I64, number);
    let roundtrip = cx.builder.ins().fcvt_from_sint(types::F64, as_int);
    let exact = cx
        .builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, number, roundtrip);
    let number_ok = cx.builder.ins().band(in_range, exact);
    let number_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(number_ok, number_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(number_block);
    cx.builder.seal_block(number_block);
    let negative = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::SignedLessThan, as_int, 0);
    let negated = cx.builder.ins().ineg(as_int);
    let magnitude = cx.builder.ins().select(negative, negated, as_int);
    let digits = emit_decimal_digit_count(cx.builder, magnitude);
    let one = cx.builder.ins().iconst(types::I64, 1);
    let zero_units = cx.builder.ins().iconst(types::I64, 0);
    let negative_units = cx.builder.ins().select(negative, one, zero_units);
    let mut number_total = builder_state.length;
    for part in &prefix_parts {
        number_total = cx.builder.ins().iadd(number_total, part.units);
    }
    number_total = cx.builder.ins().iadd(number_total, negative_units);
    number_total = cx.builder.ins().iadd(number_total, digits);
    let number_bytes = cx.builder.ins().ishl_imm_u(number_total, 1);
    let number_fits = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        number_bytes,
        builder_state.capacity,
    );
    let number_write_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(number_fits, number_write_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(number_write_block);
    cx.builder.seal_block(number_write_block);
    let payload_base = cx.builder.ins().iadd_imm_s(
        builder_state.address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let mut cursor_units = builder_state.length;
    for part in &prefix_parts {
        let cursor_bytes = cx.builder.ins().ishl_imm_u(cursor_units, 1);
        let part_dst = cx.builder.ins().iadd(payload_base, cursor_bytes);
        let done = emit_copy_part_dispatch(cx, part, part_dst);
        cx.builder.switch_to_block(done);
        cursor_units = cx.builder.ins().iadd(cursor_units, part.units);
    }
    let len_store_block = cx.builder.create_block();
    emit_append_number_digits(
        cx,
        payload_base,
        cursor_units,
        negative,
        magnitude,
        digits,
        len_store_block,
    );

    cx.builder.switch_to_block(len_store_block);
    cx.builder.seal_block(len_store_block);
    emit_store_builder_length(cx, &builder_state, number_total);
    define_value_boxed(cx.builder, cx.variables, dest, values[0])?;
    cx.builder.ins().jump(merge_block, &[]);

    emit_string_builder_append_miss(cx, dest, &values, feedback_ptr, miss_block, merge_block)?;

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}
