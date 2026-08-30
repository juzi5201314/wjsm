//! 调用与算术 overlay 辅助

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

fn lower_native_direct_call(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    env_value: ir::Value,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
    arity: usize,
) -> Result<()> {
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    let mut call_args = Vec::with_capacity(3 + arity);
    call_args.push(cx.ctx);
    call_args.push(env_value);
    call_args.push(this_value);
    for index in 0..arity {
        if let Some(argument) = args.get(index) {
            call_args.push(use_value_boxed(cx.builder, cx.variables, *argument)?);
        } else {
            call_args.push(undefined);
        }
    }

    let depth_offset = i32::try_from(offset_of!(NativeVmContext, js_call_depth))
        .context("js call depth offset exceeds i32")?;
    let depth = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), cx.ctx, depth_offset);
    let new_depth = cx.builder.ins().iadd_imm_s(depth, 1);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), new_depth, cx.ctx, depth_offset);

    cx.flush()?;
    let call = cx.builder.ins().call(target, &call_args);
    let result = cx.builder.inst_results(call)[0];

    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), depth, cx.ctx, depth_offset);

    cx.publish_roots(roots, &[result])?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

pub(crate) fn lower_closure_direct_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    env_value: ValueId,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
    arity: usize,
) -> Result<()> {
    let env_value = use_value_boxed(cx.builder, cx.variables, env_value)?;
    lower_native_direct_call(
        cx,
        target,
        destination,
        env_value,
        this_value,
        args,
        roots,
        arity,
    )
}

pub(crate) fn lower_fast_direct_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
    arity: usize,
) -> Result<()> {
    let undefined_env = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    lower_native_direct_call(
        cx,
        target,
        destination,
        undefined_env,
        this_value,
        args,
        roots,
        arity,
    )
}

pub(crate) fn lower_direct_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    env_value: Option<ValueId>,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
) -> Result<()> {
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let env_value = match env_value {
        Some(env) => use_value_boxed(cx.builder, cx.variables, env)?,
        None => cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_undefined()),
    };
    let active_len_offset = i32::try_from(offset_of!(NativeVmContext, call_arena_active_len))
        .context("call arena active length offset exceeds i32")?;
    let active_len = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        active_len_offset,
    );
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let arena_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
    );
    let active_len_u64 = cx.builder.ins().uextend(types::I64, active_len);
    let active_len_bytes = cx.builder.ins().ishl_imm_u(active_len_u64, 3);
    let base_addr = cx.builder.ins().iadd(arena_base, active_len_bytes);

    for (i, arg) in args.iter().enumerate() {
        let arg_val = use_value_boxed(cx.builder, cx.variables, *arg)?;
        let offset = i32::try_from(i * size_of::<i64>()).context("argument offset exceeds i32")?;
        cx.builder
            .ins()
            .store(MemFlagsData::trusted(), arg_val, base_addr, offset);
    }

    let args_len = u32::try_from(args.len()).context("args len exceeds u32")?;
    let new_active_len = cx.builder.ins().iadd_imm_s(active_len, i64::from(args_len));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        new_active_len,
        cx.ctx,
        active_len_offset,
    );

    let depth_offset = i32::try_from(offset_of!(NativeVmContext, js_call_depth))
        .context("js call depth offset exceeds i32")?;
    let depth = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), cx.ctx, depth_offset);
    let new_depth = cx.builder.ins().iadd_imm_s(depth, 1);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), new_depth, cx.ctx, depth_offset);

    let args_len_val = cx.builder.ins().iconst(types::I32, i64::from(args_len));

    cx.flush()?;
    let call = cx.builder.ins().call(
        target,
        &[cx.ctx, env_value, this_value, active_len, args_len_val],
    );
    let result = cx.builder.inst_results(call)[0];

    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), depth, cx.ctx, depth_offset);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        active_len,
        cx.ctx,
        active_len_offset,
    );

    cx.publish_roots(roots, &[result])?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

pub(crate) fn lower_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    slow_call_signature: ir::SigRef,
    call: CallLowering<'_>,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let CallLowering {
        destination,
        callee,
        this_value,
        args,
        operation,
        forward_args,
    } = call;
    let callee = use_value_boxed(cx.builder, cx.variables, callee)?;
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let mut call_args = Vec::with_capacity(if forward_args { 1 } else { args.len() + 1 });
    call_args.push(callee);
    if !forward_args {
        for argument in args {
            call_args.push(use_value_boxed(cx.builder, cx.variables, *argument)?);
        }
    }
    let entry = cx.call(operation.id(), &call_args, feedback_ptr)?;
    let args_len = if forward_args {
        let entry_block = cx
            .builder
            .func
            .layout
            .entry_block()
            .context("native function is missing entry block")?;
        cx.builder.block_params(entry_block)[4]
    } else {
        cx.builder.ins().iconst(
            types::I32,
            i64::try_from(args.len()).context("call argument count exceeds i64")?,
        )
    };
    let active_len = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        i32::try_from(offset_of!(NativeVmContext, call_arena_active_len))
            .context("call arena active length offset exceeds i32")?,
    );
    let args_base = cx.builder.ins().isub(active_len, args_len);
    let env = cx.call(NativeRuntimeOp::LoadCallEnv.id(), &[], None)?;
    cx.flush()?;
    let call = cx.builder.ins().call_indirect(
        slow_call_signature,
        entry,
        &[cx.ctx, env, this_value, args_base, args_len],
    );
    let result = cx.builder.inst_results(call)[0];
    cx.publish_roots(roots, &[result])?;
    let _ = cx.call(NativeRuntimeOp::FinishCall.id(), &[], None)?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

pub(crate) fn lower_builtin_operation(
    cx: &mut LoweringCx<'_, '_>,
    builtin: Builtin,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value_boxed(cx.builder, cx.variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = cx.call(builtin.wire_id().into(), &args, feedback_ptr)?;
    if builtin == Builtin::PromiseInstanceResolve || builtin == Builtin::PromiseInstanceReject {
        return Ok(());
    }
    let _ = result;
    Ok(())
}

/// `lhs` / `rhs` 是原始 f64（非 NaN-Box 编码）；`reverse` / `invert` 仍是编码布尔。
pub(crate) fn emit_f64_abstract_compare(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    reverse: ir::Value,
    invert: ir::Value,
) -> ir::Value {
    let normal = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::LessThan, lhs, rhs);
    let reversed = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::LessThan, rhs, lhs);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let reverse = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, reverse, true_value);
    let invert = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, invert, true_value);
    let relation = builder.ins().select(reverse, reversed, normal);
    let ordered = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Ordered, lhs, rhs);
    let not_relation = builder.ins().bnot(relation);
    let inverted = builder.ins().band(ordered, not_relation);
    let condition = builder.ins().select(invert, inverted, relation);
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    builder.ins().select(condition, true_value, false_value)
}

/// `lhs` / `rhs` 是原始 f64（非 NaN-Box 编码）。
pub(crate) fn emit_f64_relational(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    op: CompareOp,
) -> ir::Value {
    let cc = match op {
        CompareOp::Lt => ir::condcodes::FloatCC::LessThan,
        CompareOp::Gt => ir::condcodes::FloatCC::GreaterThan,
        CompareOp::LtEq => ir::condcodes::FloatCC::LessThanOrEqual,
        CompareOp::GtEq => ir::condcodes::FloatCC::GreaterThanOrEqual,
        _ => ir::condcodes::FloatCC::LessThan,
    };
    let condition = builder.ins().fcmp(cc, lhs, rhs);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    builder.ins().select(condition, true_value, false_value)
}

/// 把一个原始 f64 收窄成 i32，并返回「收窄是否无损」。
pub(crate) fn f64_to_i32(
    builder: &mut FunctionBuilder<'_>,
    float: ir::Value,
) -> (ir::Value, ir::Value) {
    let sat = builder.ins().fcvt_to_sint_sat(types::I32, float);
    let back = builder.ins().fcvt_from_sint(types::F64, sat);
    let ordered = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, float, back);
    (sat, ordered)
}

pub(crate) fn emit_i32_relational(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    op: CompareOp,
) -> Result<ir::Value> {
    let (lhs_i, lhs_ok) = f64_to_i32(builder, lhs);
    let (rhs_i, rhs_ok) = f64_to_i32(builder, rhs);
    let both = builder.ins().band(lhs_ok, rhs_ok);
    let cc = match op {
        CompareOp::Lt => ir::condcodes::IntCC::SignedLessThan,
        CompareOp::Gt => ir::condcodes::IntCC::SignedGreaterThan,
        CompareOp::LtEq => ir::condcodes::IntCC::SignedLessThanOrEqual,
        CompareOp::GtEq => ir::condcodes::IntCC::SignedGreaterThanOrEqual,
        _ => ir::condcodes::IntCC::SignedLessThan,
    };
    let cmp = builder.ins().icmp(cc, lhs_i, rhs_i);
    let cmp = builder.ins().band(cmp, both);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    Ok(builder.ins().select(cmp, true_value, false_value))
}

pub(crate) fn emit_i32_arithmetic(
    cx: &mut LoweringCx<'_, '_>,
    op: BinaryOp,
    lhs: ir::Value,
    rhs: ir::Value,
) -> Result<ir::Value> {
    let (lhs_i, lhs_ok) = f64_to_i32(cx.builder, lhs);
    let (rhs_i, rhs_ok) = f64_to_i32(cx.builder, rhs);
    let both = cx.builder.ins().band(lhs_ok, rhs_ok);
    let (sum, overflow) = match op {
        BinaryOp::Add => cx.builder.ins().sadd_overflow(lhs_i, rhs_i),
        BinaryOp::Sub => cx.builder.ins().ssub_overflow(lhs_i, rhs_i),
        BinaryOp::Mul => cx.builder.ins().smul_overflow(lhs_i, rhs_i),
        _ => unreachable!("guard restricts int32 arithmetic"),
    };
    let overflow_i64 = cx.builder.ins().uextend(types::I64, overflow);
    let fail_ov = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, overflow_i64, 0);
    let not_ok = cx.builder.ins().bnot(both);
    let fail = cx.builder.ins().bor(fail_ov, not_ok);
    // JS 数是 IEEE 754：i32 溢出不是类型 miss，直接回退同一对操作数的 f64 运算，
    // 避免为每条 Add/Sub/Mul 保留 resume pad（deopt 会把热循环切成逐指令块）。
    let wide = match op {
        BinaryOp::Add => cx.builder.ins().fadd(lhs, rhs),
        BinaryOp::Sub => cx.builder.ins().fsub(lhs, rhs),
        BinaryOp::Mul => cx.builder.ins().fmul(lhs, rhs),
        _ => unreachable!("guard restricts int32 arithmetic"),
    };
    let widened = cx.builder.ins().sextend(types::I64, sum);
    let narrow = cx.builder.ins().fcvt_from_sint(types::F64, widened);
    Ok(cx.builder.ins().select(fail, wide, narrow))
}

pub(crate) fn emit_osr_poll(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    header: BasicBlockId,
    instruction_index: u32,
    lives: &[ValueId],
) -> Result<()> {
    // osr_entry 当前指向「整函数」specialized body（见 specialize.rs /
    // install_osr_entry），不是循环中段续跑入口。若在 header 把控制权交给它，
    // 等于用特化 SSA/投机假设重跑整个函数；与 generic 的 resume 槽、根帧状态
    // 交织后会在 array_inline / Map churn 等分配路径上 SIGSEGV。
    // 在真正的中段 OSR 或可证明的入口重启落地前，轮询只保留占位，不转移。
    let _ = (cx, tables, header, instruction_index, lives);
    Ok(())
}

pub(crate) fn emit_overlay_header_guards(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    header: BasicBlockId,
    lives: &[ValueId],
) -> Result<()> {
    for live in lives {
        if !tables.f64_values.contains(live) && !tables.int32_values.contains(live) {
            continue;
        }
        // 常驻 F64 寄存器的值按构造就是 number，守卫恒真；发它反而会把
        // 原始 NaN 位模式误判成 NaN-Box 前缀而无谓 deopt。
        if cx.variables.is_typed_value(*live) {
            continue;
        }
        let encoded = use_value_boxed(cx.builder, cx.variables, *live)?;
        let ok = emit_is_number(cx.builder, encoded);
        let deopt = cx.builder.create_block();
        let cont = cx.builder.create_block();
        cx.builder.ins().brif(ok, cont, &[], deopt, &[]);
        cx.builder.switch_to_block(deopt);
        emit_deopt_to_generic(cx, header, lives)?;
        cx.builder.switch_to_block(cont);
        cx.builder.seal_block(cont);
    }
    Ok(())
}
