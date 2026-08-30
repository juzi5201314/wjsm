//! 终止器、dispatcher 与 cooperative poll

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;
use wjsm_ir::{Instruction, ValueId, constants, value};
use wjsm_native_abi::{COOPERATIVE_POLL_BUDGET, NativeRuntimeOp};

pub(crate) fn lower_value_operation(
    cx: &mut LoweringCx<'_, '_>,
    operation: NativeRuntimeOp,
    args: &[ValueId],
    destination: Option<ValueId>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value_boxed(cx.builder, cx.variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = cx.call(operation.id(), &args, None)?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

/// 每个函数按需 import 一次 typed math thunk，同函数内所有调用点复用同一 `FuncRef`。
pub(crate) fn import_math_thunk(
    builder: &mut FunctionBuilder<'_>,
    math_thunks: &HashMap<Builtin, DeclaredFunction>,
    imported: &mut HashMap<Builtin, ir::FuncRef>,
    builtin: Builtin,
) -> Result<ir::FuncRef> {
    if let Some(func_ref) = imported.get(&builtin).copied() {
        return Ok(func_ref);
    }
    let declaration = math_thunks
        .get(&builtin)
        .with_context(|| format!("math thunk {builtin:?} 未声明"))?;
    let func_ref = declaration.import(builder.func);
    imported.insert(builtin, func_ref);
    Ok(func_ref)
}

/// 计算反馈槽地址：`ctx.feedback_slots_base + slot × FEEDBACK_SLOT_SIZE`。
///
/// 反馈区由当前 base image 持有；特化 overlay 永不激活为 current image，其生成
/// 代码同样经由 vmctx 基址写 base image 的槽，编号在两种编译间保持一致。
pub(crate) fn emit_feedback_slot_ptr(
    builder: &mut FunctionBuilder<'_>,
    ctx: ir::Value,
    slot: u32,
) -> Result<ir::Value> {
    let pointer_type = builder.func.dfg.value_type(ctx);
    let base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, feedback_slots_base))?,
    );
    let offset = i64::from(slot)
        .checked_mul(i64::from(constants::FEEDBACK_SLOT_SIZE))
        .context("feedback slot offset exceeds i64")?;
    Ok(builder.ins().iadd_imm_s(base, offset))
}

pub(crate) fn call_dispatcher(
    builder: &mut FunctionBuilder<'_>,
    frame: Option<&mut FrameLowering>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    operation: u32,
    args: &[ir::Value],
    feedback_slot: Option<ir::Value>,
) -> Result<ir::Value> {
    let byte_len = args
        .len()
        .checked_mul(size_of::<i64>())
        .ok_or_else(|| anyhow!("host operation argument bytes overflow"))?;
    let byte_len = u32::try_from(byte_len).context("host operation argument area exceeds u32")?;
    let pointer_type = builder.func.dfg.value_type(ctx);
    let args_pointer = if args.is_empty() {
        builder.ins().iconst(pointer_type, 0)
    } else {
        let frame = frame.context("host call arguments require a root frame")?;
        let base = frame.reserve_arena(byte_len);
        for (index, value) in args.iter().enumerate() {
            let offset = index
                .checked_mul(size_of::<i64>())
                .and_then(|offset| i32::try_from(offset).ok())
                .context("host operation stack slot offset exceeds i32")?;
            builder
                .ins()
                .store(MemFlagsData::trusted(), *value, base, offset);
        }
        base
    };
    let operation = builder.ins().iconst(types::I32, i64::from(operation));
    let count = builder.ins().iconst(
        types::I32,
        i64::try_from(args.len()).context("host operation argument count exceeds i64")?,
    );
    let feedback_slot = match feedback_slot {
        Some(slot) => slot,
        None => builder.ins().iconst(pointer_type, 0),
    };
    let call = builder.ins().call(
        dispatcher,
        &[ctx, operation, args_pointer, count, feedback_slot],
    );
    builder
        .inst_results(call)
        .first()
        .copied()
        .context("host dispatcher returned no result")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_terminator(
    cx: &mut LoweringCx<'_, '_>,
    predecessor: BasicBlockId,
    terminator: &Terminator,
    constants: &[Constant],
    boolean_values: &HashSet<ValueId>,
    blocks: &HashMap<BasicBlockId, ir::Block>,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
    poll_edges: &HashSet<(BasicBlockId, BasicBlockId)>,
) -> Result<()> {
    match terminator {
        Terminator::Return { value } => {
            let result = match value {
                Some(value) => use_value_boxed(cx.builder, cx.variables, *value)?,
                None => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_undefined()),
            };
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
        }
        Terminator::Jump { target } => {
            if poll_edges.contains(&(predecessor, *target)) {
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            define_phi_edge(cx.builder, cx.variables, phi_edges, predecessor, *target)?;
            cx.builder.ins().jump(blocks[target], &[]);
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            if poll_edges.contains(&(predecessor, *true_block))
                || poll_edges.contains(&(predecessor, *false_block))
            {
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            let condition_is_boolean = boolean_values.contains(condition);
            let condition = use_value_boxed(cx.builder, cx.variables, *condition)?;
            let condition = if condition_is_boolean {
                cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::Equal,
                    condition,
                    value::encode_bool(true),
                )
            } else {
                let condition = cx.call(NativeRuntimeOp::IsTruthy.id(), &[condition], None)?;
                cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    condition,
                    value::encode_bool(false),
                )
            };
            define_phi_edge(
                cx.builder,
                cx.variables,
                phi_edges,
                predecessor,
                *true_block,
            )?;
            define_phi_edge(
                cx.builder,
                cx.variables,
                phi_edges,
                predecessor,
                *false_block,
            )?;
            cx.builder
                .ins()
                .brif(condition, blocks[true_block], &[], blocks[false_block], &[]);
        }
        Terminator::Switch {
            value,
            cases,
            default_block,
            ..
        } => {
            if cases
                .iter()
                .any(|case| poll_edges.contains(&(predecessor, case.target)))
                || poll_edges.contains(&(predecessor, *default_block))
            {
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            if cases.is_empty() {
                define_phi_edge(
                    cx.builder,
                    cx.variables,
                    phi_edges,
                    predecessor,
                    *default_block,
                )?;
                cx.builder.ins().jump(blocks[default_block], &[]);
            } else {
                for (index, case) in cases.iter().enumerate() {
                    let constant = constants
                        .get(
                            usize::try_from(case.constant.0)
                                .context("switch constant index exceeds usize")?,
                        )
                        .with_context(|| {
                            format!("switch constant {} is missing", case.constant.0)
                        })?;
                    let immediate = switch_constant_immediate(constant)?;
                    let condition =
                        cx.builder
                            .ins()
                            .icmp_imm_u(ir::condcodes::IntCC::Equal, value, immediate);
                    define_phi_edge(
                        cx.builder,
                        cx.variables,
                        phi_edges,
                        predecessor,
                        case.target,
                    )?;
                    if index + 1 == cases.len() {
                        define_phi_edge(
                            cx.builder,
                            cx.variables,
                            phi_edges,
                            predecessor,
                            *default_block,
                        )?;
                        cx.builder.ins().brif(
                            condition,
                            blocks[&case.target],
                            &[],
                            blocks[default_block],
                            &[],
                        );
                    } else {
                        let next_case = cx.builder.create_block();
                        cx.builder
                            .ins()
                            .brif(condition, blocks[&case.target], &[], next_case, &[]);
                        cx.builder.switch_to_block(next_case);
                    }
                }
            }
        }
        Terminator::Throw { value } => {
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            let exception = cx.call(NativeRuntimeOp::CreateException.id(), &[value], None)?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[exception]);
        }
        Terminator::Unreachable => {
            let result = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_undefined());
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
        }
        Terminator::Deopt { frames } => {
            let Some(frame) = frames.first() else {
                bail!("Deopt terminator requires a frame");
            };
            cx.current_instruction = frame.instruction_index;
            emit_deopt_to_generic(cx, frame.block, &frame.lives)?;
        }
    }
    Ok(())
}

pub(crate) fn emit_is_number(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(input, box_base);
    builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, boxed_bits, box_base)
}

/// CLIF 版「是否为规范 boxed tagged handle」：等价于 `value::is_tagged` 的 boxed
/// 前置判定——要求 `BOX_BASE` 前缀齐全，且 SSO marker 位（48–50）为零。
///
/// inline SSO 字符串同样带 `BOX_BASE`，其 7-bit/8-bit 码元载荷可覆盖 tag 位
/// （32–36），只查 `BOX_BASE` 会把这类字符串误判成 object/array/exception/
/// runtime-string 句柄，进而去解一个越界句柄索引。标准 tagged handle 的
/// bits 44–50 恒为零，因此并入 marker 掩码不会漏判任何真实句柄。
pub(crate) fn emit_is_boxed_handle(
    builder: &mut FunctionBuilder<'_>,
    input: ir::Value,
) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_mask =
        i64::from_ne_bytes((value::BOX_BASE | value::INLINE_STRING_MARKER_MASK).to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(input, boxed_mask);
    builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base)
}

/// CLIF 版 `value::strip_gc_color`：仅对 handle-backed reference 清除 GC color。
/// number 与 inline SSO 的 payload 可能占用 bits 38–43，必须原样保留。
pub(crate) fn emit_strip_gc_color(
    builder: &mut FunctionBuilder<'_>,
    input: ir::Value,
) -> ir::Value {
    let is_number = emit_is_number(builder, input);
    let is_inline = emit_inline_string_predicate(builder, input);
    let color_mask = i64::from_ne_bytes((!value::GC_COLOR_MASK).to_ne_bytes());
    let stripped = builder.ins().band_imm_u(input, color_mask);
    let keep_raw = builder.ins().bor(is_number, is_inline);
    builder.ins().select(keep_raw, input, stripped)
}

/// CLIF 版 `value::is_exception`：与 `value::is_tagged` 一致，boxed 判定并入 SSO
/// marker 位排除 inline 字符串（详见 [`emit_is_boxed_handle`]），再比对 tag。
pub(crate) fn emit_is_exception(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let boxed = emit_is_boxed_handle(builder, input);
    let tag = builder.ins().ushr_imm_u(input, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let exception_tag = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_EXCEPTION).expect("exception tag fits i64"),
    );
    builder.ins().band(boxed, exception_tag)
}

/// CLIF 版 `value::is_function`：boxed handle 且 tag 为 `TAG_FUNCTION`。
pub(crate) fn emit_is_function(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let boxed = emit_is_boxed_handle(builder, input);
    let tag = builder.ins().ushr_imm_u(input, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_fn = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_FUNCTION).expect("function tag fits i64"),
    );
    builder.ins().band(boxed, is_fn)
}

pub(crate) fn return_if_exception(cx: &mut LoweringCx<'_, '_>, result: ir::Value) -> Result<()> {
    let is_exception = emit_is_exception(cx.builder, result);
    let exception_block = cx.builder.create_block();
    let continue_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_exception, exception_block, &[], continue_block, &[]);
    cx.builder.switch_to_block(exception_block);
    cx.unlink_roots()?;
    cx.builder.ins().return_(&[result]);
    cx.builder.switch_to_block(continue_block);
    Ok(())
}

fn current_poll_budget(cx: &mut LoweringCx<'_, '_>) -> Result<ir::Value> {
    if let Some(var) = cx.poll_budget {
        Ok(cx.builder.use_var(var))
    } else {
        load_vmctx_poll_budget(cx.builder, cx.ctx)
    }
}

fn commit_poll_budget(cx: &mut LoweringCx<'_, '_>, remaining: ir::Value) -> Result<()> {
    if let Some(var) = cx.poll_budget {
        cx.builder.def_var(var, remaining);
        return Ok(());
    }
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        remaining,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, stack_budget_bytes))?,
    );
    Ok(())
}

pub(crate) fn lower_cooperative_poll(cx: &mut LoweringCx<'_, '_>) -> Result<()> {
    let budget = current_poll_budget(cx)?;
    let step_i64 =
        i64::try_from(COOPERATIVE_POLL_LOOP_BACKEDGE_STEP_BYTES).expect("loop poll step fits i64");
    // 预算已 ≤ 步长（含耗尽的 0）→ 慢路径：进 dispatcher 轮询并重置预算。
    let exhausted = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        budget,
        step_i64,
    );
    let slow_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(exhausted, slow_block, &[], fast_block, &[]);

    emit_poll_fast_path(cx, fast_block, budget, step_i64)?;
    let continue_block = cx.builder.create_block();
    cx.builder.ins().jump(continue_block, &[]);
    emit_poll_slow_path(cx, slow_block, continue_block)?;

    cx.builder.switch_to_block(continue_block);
    cx.builder.seal_block(continue_block);
    Ok(())
}

fn emit_poll_fast_path(
    cx: &mut LoweringCx<'_, '_>,
    fast_block: ir::Block,
    budget: ir::Value,
    step_i64: i64,
) -> Result<()> {
    // 快路径：预算充足，扣减步长后继续回边，不调用 dispatcher。
    // 寄存器预算（safepoint-free）只 def Variable，不写 vmctx。
    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let step = cx.builder.ins().iconst(types::I64, step_i64);
    let remaining = cx.builder.ins().isub(budget, step);
    commit_poll_budget(cx, remaining)
}

fn emit_poll_slow_path(
    cx: &mut LoweringCx<'_, '_>,
    slow_block: ir::Block,
    continue_block: ir::Block,
) -> Result<()> {
    // 慢路径：预算耗尽，进 dispatcher 轮询（inspector / GC / 外部事件 / 期限）；
    // 宿主在 CooperativePoll 处理中把预算重置回初始值。
    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let result = call_dispatcher(
        cx.builder,
        None,
        cx.dispatcher,
        cx.ctx,
        NativeRuntimeOp::CooperativePoll.id(),
        &[],
        None,
    )?;
    // 先把寄存器同步成宿主刚写入的预算，异常返回才不会用耗尽值覆盖 vmctx。
    reset_poll_budget_after_host(cx)?;
    return_if_exception(cx, result)?;
    cx.builder.ins().jump(continue_block, &[]);
    Ok(())
}

fn reset_poll_budget_after_host(cx: &mut LoweringCx<'_, '_>) -> Result<()> {
    let Some(var) = cx.poll_budget else {
        return Ok(());
    };
    let budget = i64::try_from(COOPERATIVE_POLL_BUDGET).expect("poll budget fits i64");
    let reset = cx.builder.ins().iconst(types::I64, budget);
    cx.builder.def_var(var, reset);
    Ok(())
}
