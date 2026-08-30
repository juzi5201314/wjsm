//! 严格相等与动态二元

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

#[derive(Clone, Copy)]
pub(crate) struct StrictEqMode {
    pub(crate) slow_operation: u32,
    pub(crate) invert: bool,
}

pub(crate) fn lower_strict_eq(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    lhs_id: ValueId,
    rhs_id: ValueId,
    mode: StrictEqMode,
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let lhs = use_value_boxed(cx.builder, cx.variables, lhs_id)?;
    let rhs = use_value_boxed(cx.builder, cx.variables, rhs_id)?;
    if let Some(slot) = feedback_ptr {
        emit_inline_binary_feedback(cx.builder, cx.ctx, slot, mode.slow_operation, lhs, rhs);
    }
    let lhs_plain = emit_strip_gc_color(cx.builder, lhs);
    let rhs_plain = emit_strip_gc_color(cx.builder, rhs);
    let same_plain = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, lhs_plain, rhs_plain);
    let same_raw = cx.builder.ins().icmp(ir::condcodes::IntCC::Equal, lhs, rhs);
    let tag_word = cx.builder.ins().ushr_imm_u(lhs_plain, 32);
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
    let same_runtime_string = cx.builder.ins().band(same_plain, is_string);
    let same_runtime_string = cx.builder.ins().band(same_runtime_string, is_runtime);
    let lhs_inline = emit_inline_string_predicate(cx.builder, lhs_plain);
    let rhs_inline = emit_inline_string_predicate(cx.builder, rhs_plain);
    let any_inline = cx.builder.ins().bor(lhs_inline, rhs_inline);
    let both_inline = cx.builder.ins().band(lhs_inline, rhs_inline);
    let same_block = cx.builder.create_block();
    let compare_block = cx.builder.create_block();
    let inline_block = cx.builder.create_block();
    let inline_equal_block = cx.builder.create_block();
    let non_inline_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let false_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(any_inline, inline_block, &[], non_inline_block, &[]);

    cx.builder.switch_to_block(inline_block);
    cx.builder.seal_block(inline_block);
    cx.builder
        .ins()
        .brif(both_inline, inline_equal_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(inline_equal_block);
    cx.builder.seal_block(inline_equal_block);
    let true_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    let false_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(mode.invert));
    let result = cx.builder.ins().select(same_raw, true_value, false_value);
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(non_inline_block);
    cx.builder.seal_block(non_inline_block);
    cx.builder
        .ins()
        .brif(same_runtime_string, same_block, &[], compare_block, &[]);

    cx.builder.switch_to_block(same_block);
    cx.builder.seal_block(same_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(compare_block);
    cx.builder.seal_block(compare_block);
    let left_address = emit_string_address(cx, barrier_thunks, lhs, miss_block)?;
    let right_address = emit_string_address(cx, barrier_thunks, rhs, miss_block)?;
    let left_header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        left_address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let right_header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        right_address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let left_length = cx
        .builder
        .ins()
        .band_imm_u(left_header, i64::from(u32::MAX));
    let right_length = cx
        .builder
        .ins()
        .band_imm_u(right_header, i64::from(u32::MAX));
    let same_length = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_length, right_length);
    let hash_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same_length, hash_block, &[], false_block, &[]);

    cx.builder.switch_to_block(hash_block);
    cx.builder.seal_block(hash_block);
    let left_hash = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        left_address,
        i32::try_from(constants::HEAP_STRING_HASH_OFFSET).expect("hash offset fits i32"),
    );
    let right_hash = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        right_address,
        i32::try_from(constants::HEAP_STRING_HASH_OFFSET).expect("hash offset fits i32"),
    );
    let left_hash_ready = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, left_hash, 0);
    let right_hash_ready =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::NotEqual, right_hash, 0);
    let hashes_ready = cx.builder.ins().band(left_hash_ready, right_hash_ready);
    let same_hash = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_hash, right_hash);
    let hashes_not_ready = cx.builder.ins().bnot(hashes_ready);
    let content_possible = cx.builder.ins().bor(hashes_not_ready, same_hash);
    let repr_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(content_possible, repr_block, &[], false_block, &[]);

    cx.builder.switch_to_block(repr_block);
    cx.builder.seal_block(repr_block);
    let left_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), left_address, 0);
    let right_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), right_address, 0);
    let repr_shift = i64::from(constants::HEAP_STRING_REPR_OFFSET * 8);
    let left_repr = cx.builder.ins().ushr_imm_u(left_word, repr_shift);
    let left_repr = cx.builder.ins().band_imm_u(left_repr, 0xff);
    let right_repr = cx.builder.ins().ushr_imm_u(right_word, repr_shift);
    let right_repr = cx.builder.ins().band_imm_u(right_repr, 0xff);
    let same_repr = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_repr, right_repr);
    let left_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        left_repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let left_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        left_repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let right_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        right_repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let right_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        right_repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let left_flat = cx.builder.ins().bor(left_latin1, left_utf16);
    let right_flat = cx.builder.ins().bor(right_latin1, right_utf16);
    let both_flat = cx.builder.ins().band(left_flat, right_flat);
    let same_flat = cx.builder.ins().band(same_repr, both_flat);
    let content_block = cx.builder.create_block();
    let mixed_check_block = cx.builder.create_block();
    let mixed_block = cx.builder.create_block();
    let mixed_equal_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same_flat, content_block, &[], mixed_check_block, &[]);
    cx.builder.switch_to_block(mixed_check_block);
    cx.builder.seal_block(mixed_check_block);
    cx.builder
        .ins()
        .brif(both_flat, mixed_block, &[], miss_block, &[]);
    cx.builder.switch_to_block(mixed_block);
    cx.builder.seal_block(mixed_block);
    emit_mixed_string_equal(
        cx,
        left_address,
        right_address,
        left_length,
        left_latin1,
        right_latin1,
        (mixed_equal_block, false_block),
    );
    cx.builder.switch_to_block(mixed_equal_block);
    cx.builder.seal_block(mixed_equal_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(content_block);
    cx.builder.seal_block(content_block);
    let left_payload = cx.builder.ins().iadd_imm_s(
        left_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let right_payload = cx.builder.ins().iadd_imm_s(
        right_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let utf16_bytes = cx.builder.ins().ishl_imm_u(left_length, 1);
    let byte_length = cx
        .builder
        .ins()
        .select(left_latin1, left_length, utf16_bytes);
    let comparison =
        cx.builder
            .call_memcmp(cx.target_config, left_payload, right_payload, byte_length);
    let equal = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, comparison, 0);
    let true_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(true));
    let false_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(false));
    let result = if mode.invert {
        cx.builder.ins().select(equal, false_value, true_value)
    } else {
        cx.builder.ins().select(equal, true_value, false_value)
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(false_block);
    cx.builder.seal_block(false_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(mode.slow_operation, &[lhs, rhs], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

pub(crate) fn lower_dynamic_binary(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    op: BinaryOp,
    lhs: ValueId,
    rhs: ValueId,
    feedback_ptr: Option<ir::Value>,
    f64_values: &HashSet<ValueId>,
) -> Result<()> {
    let lhs_id = lhs;
    let rhs_id = rhs;
    let lhs = use_value_boxed(cx.builder, cx.variables, lhs)?;
    let rhs = use_value_boxed(cx.builder, cx.variables, rhs)?;

    // 位运算、% 与 ** 仍需 ToPrimitive/ToNumber/ToBigInt 等完整语义，继续走 dispatcher。
    if !matches!(
        op,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
    ) {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        let result = cx.call(operation, &[lhs, rhs], feedback_ptr)?;
        return define_value_boxed(cx.builder, cx.variables, dest, result);
    }

    // #389 的 number 快路径不经过 dispatcher，二元反馈必须在守卫前内联更新，
    // number/number 热路径才可被观察。更新覆盖 fast/slow 两条路径，因此下方
    // 慢路径的 dispatcher 调用传 null 槽，避免同一次执行重复计数。
    if let Some(slot) = feedback_ptr {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        emit_inline_binary_feedback(cx.builder, cx.ctx, slot, operation, lhs, rhs);
    }

    // number/number 直接发原生浮点指令；已证明的 f64 操作数跳过 is_number。
    // 加法只要原始操作数一侧已是字符串，ToPrimitive 后必进入字符串拼接。
    let lhs_is_number = emit_number_or_proven_f64(cx.builder, lhs, lhs_id, f64_values);
    let rhs_is_number = emit_number_or_proven_f64(cx.builder, rhs, rhs_id, f64_values);
    let both_numbers = cx.builder.ins().band(lhs_is_number, rhs_is_number);

    let number_block = cx.builder.create_block();
    let non_number_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(both_numbers, number_block, &[], non_number_block, &[]);

    cx.builder.switch_to_block(number_block);
    cx.builder.seal_block(number_block);
    let lhs_f64 = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), lhs);
    let rhs_f64 = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), rhs);
    let result = match op {
        BinaryOp::Add => cx.builder.ins().fadd(lhs_f64, rhs_f64),
        BinaryOp::Sub => cx.builder.ins().fsub(lhs_f64, rhs_f64),
        BinaryOp::Mul => cx.builder.ins().fmul(lhs_f64, rhs_f64),
        BinaryOp::Div => cx.builder.ins().fdiv(lhs_f64, rhs_f64),
        _ => unreachable!("guard restricts guarded binary operations"),
    };
    let result = box_f64_result(cx.builder, result);
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(non_number_block);
    cx.builder.seal_block(non_number_block);
    if op == BinaryOp::Add {
        let string_tag = cx.builder.ins().iconst(
            types::I64,
            i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
        );
        let lhs_tag = emit_feedback_tag_code(cx.builder, lhs);
        let rhs_tag = emit_feedback_tag_code(cx.builder, rhs);
        let lhs_is_string = cx.builder.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            lhs_tag,
            string_tag,
        );
        let rhs_is_string = cx.builder.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            rhs_tag,
            string_tag,
        );
        let either_is_string = cx.builder.ins().bor(lhs_is_string, rhs_is_string);
        let string_block = cx.builder.create_block();
        let slow_block = cx.builder.create_block();
        cx.builder
            .ins()
            .brif(either_is_string, string_block, &[], slow_block, &[]);

        cx.builder.switch_to_block(string_block);
        cx.builder.seal_block(string_block);
        cx.flush()?;
        let call = cx.builder.ins().call(cx.string_add, &[cx.ctx, lhs, rhs]);
        let result = cx.builder.inst_results(call)[0];
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
        cx.builder.ins().jump(merge_block, &[]);

        cx.builder.switch_to_block(slow_block);
        cx.builder.seal_block(slow_block);
    }
    let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
    let result = cx.call(operation, &[lhs, rhs], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// 反馈槽字段 offset（`wjsm-ir::constants` 的 u32）转 CLIF 用的 i32。
pub(crate) fn feedback_offset_i32(offset: u32) -> i32 {
    i32::try_from(offset).expect("feedback slot offset fits i32")
}

/// 计算一个 boxed 值的反馈 tag 码：number → `0x1f`，NaN-box 值 → 自身 tag。
pub(crate) fn emit_feedback_tag_code(
    builder: &mut FunctionBuilder<'_>,
    input: ir::Value,
) -> ir::Value {
    let is_number = emit_is_number(builder, input);
    let tag = builder.ins().ushr_imm_u(input, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let number_code = builder.ins().iconst(
        types::I64,
        i64::from(wjsm_native_abi::NativeFeedbackTag::Number.code()),
    );
    builder.ins().select(is_number, number_code, tag)
}

/// 守卫二元运算的零分配反馈更新：与宿主 dispatcher 的记录共享同一套槽字段协议。
///
/// 只写 tag 签名与计数（目标字段恒 0：二元操作没有调用目标）；签名变化时
/// 重置连续计数。`ctx.flags` 未置反馈位时整个更新被跳过，generic 对照路径
/// 零开销。
pub(crate) fn emit_inline_binary_feedback(
    builder: &mut FunctionBuilder<'_>,
    ctx: ir::Value,
    slot: ir::Value,
    operation: u32,
    lhs: ir::Value,
    rhs: ir::Value,
) {
    let flags = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, flags)).expect("vmctx flags offset fits i32"),
    );
    let enabled = builder.ins().band_imm_u(
        flags,
        i64::from(wjsm_native_abi::NATIVE_FLAGS_FEEDBACK_ENABLED),
    );
    let enabled = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, enabled, 0);

    let update_block = builder.create_block();
    let skip_block = builder.create_block();
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(enabled, update_block, &[], skip_block, &[]);

    builder.switch_to_block(update_block);
    builder.seal_block(update_block);
    let lhs_code = emit_feedback_tag_code(builder, lhs);
    let rhs_code = emit_feedback_tag_code(builder, rhs);
    // 签名 = count(2) | lhs_tag << 4 | rhs_tag << 10，与 encode_feedback_tag_signature 一致。
    let count = builder.ins().iconst(types::I64, 2);
    let lhs_shifted = builder.ins().ishl_imm_u(lhs_code, 4);
    let rhs_shifted = builder.ins().ishl_imm_u(rhs_code, 10);
    let signature = builder.ins().bor(count, lhs_shifted);
    let signature = builder.ins().bor(signature, rhs_shifted);
    let previous = builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET),
    );
    builder.ins().store(
        MemFlagsData::trusted(),
        signature,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET),
    );
    let same_signature = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, previous, signature);
    let consecutive = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_CONSECUTIVE_OFFSET),
    );
    let incremented = builder.ins().iadd_imm_s(consecutive, 1);
    let restart = builder.ins().iconst(types::I32, 1);
    let next_consecutive = builder.ins().select(same_signature, incremented, restart);
    builder.ins().store(
        MemFlagsData::trusted(),
        next_consecutive,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_CONSECUTIVE_OFFSET),
    );
    let total = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TOTAL_OFFSET),
    );
    let total = builder.ins().iadd_imm_s(total, 1);
    builder.ins().store(
        MemFlagsData::trusted(),
        total,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TOTAL_OFFSET),
    );
    let operation_value = builder.ins().iconst(types::I32, i64::from(operation));
    builder.ins().store(
        MemFlagsData::trusted(),
        operation_value,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_OPERATION_OFFSET),
    );
    let recording = builder
        .ins()
        .iconst(types::I32, i64::from(constants::FEEDBACK_STATE_RECORDING));
    builder.ins().store(
        MemFlagsData::trusted(),
        recording,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_STATE_OFFSET),
    );
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(skip_block);
    builder.seal_block(skip_block);
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(continuation);
    builder.seal_block(continuation);
}
