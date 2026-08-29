//! IR 指令 lowering 分发。

#![allow(unused_imports)]
use super::*;
use anyhow::{Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, types};
use wjsm_ir::{Builtin, Instruction, ValueId, constants};
use wjsm_native_abi::NativeRuntimeOp;

pub(crate) fn lower_instruction(
    cx: &mut LoweringCx<'_, '_>,
    tables: &mut InstructionTables<'_>,
    instruction: &Instruction,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    match instruction {
        Instruction::Const {
            dest,
            constant: constant_id,
        } => {
            let constant_index =
                usize::try_from(constant_id.0).context("constant index does not fit usize")?;
            let constant = tables
                .constants
                .get(constant_index)
                .with_context(|| format!("constant {} is missing", constant_id.0))?;
            // typed 目标直接物化成浮点常量，省掉「iconst + bitcast」这对指令。
            if let Constant::Number(number) = constant
                && cx.variables.is_typed_value(*dest)
            {
                let canonical = f64::from_bits(value::encode_f64(*number) as u64);
                let native = cx.builder.ins().f64const(canonical);
                return define_value_f64(cx.builder, cx.variables, *dest, native);
            }
            let native = match constant {
                Constant::Number(value) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_f64(*value)),
                Constant::Bool(value) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(*value)),
                Constant::Null => cx.builder.ins().iconst(types::I64, value::encode_null()),
                Constant::Undefined => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_undefined()),
                Constant::Uninitialized => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_uninitialized()),
                Constant::FunctionRef(function) => {
                    let index = cx.builder.ins().iconst(types::I64, i64::from(function.0));
                    cx.call(NativeRuntimeOp::MaterializeFunction.id(), &[index], None)?
                }
                Constant::NativeCallableEval => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_native_callable_idx(0)),
                Constant::ModuleId(module) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_f64(f64::from(module.0))),
                Constant::String(_) | Constant::Utf16String(_) | Constant::BigInt(_) => tables
                    .hoisted_constants
                    .get(constant_id)
                    .copied()
                    .context("immutable constant was not hoisted")?,
                Constant::ArrayTemplate(_) => {
                    bail!("array templates are materialized by clone_array_template")
                }
                Constant::ObjectTemplate { .. } => {
                    bail!("object templates are materialized by init_object_literal")
                }
                Constant::RegExp { .. } => {
                    let index = cx
                        .builder
                        .ins()
                        .iconst(types::I64, i64::from(constant_id.0));
                    let result =
                        cx.call(NativeRuntimeOp::MaterializeRegExp.id(), &[index], None)?;
                    return_if_exception(cx.builder, result, cx.root_frame.as_deref_mut(), cx.ctx)?;
                    result
                }
            };
            define_value_boxed(cx.builder, cx.variables, *dest, native)
        }
        Instruction::Binary { dest, op, lhs, rhs }
            if tables.speculative
                && tables.int32_values.contains(dest)
                && matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul) =>
        {
            let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
            let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
            let result = emit_i32_arithmetic(cx, *op, lhs_val, rhs_val)?;
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::Binary { dest, op, lhs, rhs }
            if tables.f64_values.contains(dest)
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
        {
            let lhs = use_value_f64(cx.builder, cx.variables, *lhs)?;
            let rhs = use_value_f64(cx.builder, cx.variables, *rhs)?;
            let result = match op {
                BinaryOp::Add => cx.builder.ins().fadd(lhs, rhs),
                BinaryOp::Sub => cx.builder.ins().fsub(lhs, rhs),
                BinaryOp::Mul => cx.builder.ins().fmul(lhs, rhs),
                BinaryOp::Div => cx.builder.ins().fdiv(lhs, rhs),
                _ => unreachable!("guard restricts direct f64 operations"),
            };
            if cx.variables.is_typed_value(*dest) {
                return define_value_f64(cx.builder, cx.variables, *dest, result);
            }
            let result = box_f64_arithmetic(cx.builder, *op, result);
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::Binary { dest, op, lhs, rhs } => {
            lower_dynamic_binary(cx, *dest, *op, *lhs, *rhs, feedback_ptr, tables.f64_values)
        }
        Instruction::Unary { dest, op, value } => {
            if tables.f64_values.contains(dest) && matches!(op, UnaryOp::Neg | UnaryOp::Pos) {
                if *op == UnaryOp::Neg {
                    let input = use_value_f64(cx.builder, cx.variables, *value)?;
                    let result = cx.builder.ins().fneg(input);
                    return define_value_f64(cx.builder, cx.variables, *dest, result);
                }
                // 一元 `+` 对已证明 number 是恒等运算，按目标表示原样搬运即可。
                let dest_is_typed = cx.variables.is_typed_value(*dest);
                let native = use_value_as(cx.builder, cx.variables, dest_is_typed, *value)?;
                define_value_as(cx.builder, cx.variables, *dest, native)
            } else {
                let operation = DYNAMIC_UNARY_BASE + u32::from(unary_tag(*op));
                let input = use_value_boxed(cx.builder, cx.variables, *value)?;
                let result = cx.call(operation, &[input], feedback_ptr)?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            }
        }
        Instruction::Compare { dest, op, lhs, rhs } if op.is_relational() => {
            if tables.speculative
                && tables.int32_values.contains(lhs)
                && tables.int32_values.contains(rhs)
            {
                let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
                let result = emit_i32_relational(cx.builder, lhs_val, rhs_val, *op)?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            } else if tables.f64_values.contains(lhs) && tables.f64_values.contains(rhs) {
                let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
                let result = emit_f64_relational(cx.builder, lhs_val, rhs_val, *op);
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            } else {
                let lhs_val = use_value_boxed(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_boxed(cx.builder, cx.variables, *rhs)?;
                let reverse = matches!(*op, CompareOp::Gt | CompareOp::LtEq);
                let invert = matches!(*op, CompareOp::LtEq | CompareOp::GtEq);
                let reverse_v = cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(reverse));
                let invert_v = cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(invert));
                let result = cx.call(
                    u32::from(Builtin::AbstractCompare.wire_id()),
                    &[lhs_val, rhs_val, reverse_v, invert_v],
                    feedback_ptr,
                )?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            }
        }
        Instruction::Compare { dest, op, lhs, rhs } => {
            let operation = DYNAMIC_COMPARE_BASE + u32::from(compare_tag(*op));
            lower_strict_eq(
                cx,
                tables.barrier_thunks,
                *dest,
                *lhs,
                *rhs,
                StrictEqMode {
                    slow_operation: operation,
                    invert: *op == CompareOp::StrictNotEq,
                },
                feedback_ptr,
            )
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::AbstractCompare,
            args,
        } if args.len() == 4 => {
            let reverse = use_value_boxed(cx.builder, cx.variables, args[2])?;
            let invert = use_value_boxed(cx.builder, cx.variables, args[3])?;
            if tables.f64_values.contains(&args[0]) && tables.f64_values.contains(&args[1]) {
                let lhs = use_value_f64(cx.builder, cx.variables, args[0])?;
                let rhs = use_value_f64(cx.builder, cx.variables, args[1])?;
                let result = emit_f64_abstract_compare(cx.builder, lhs, rhs, reverse, invert);
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            } else {
                let lhs = use_value_boxed(cx.builder, cx.variables, args[0])?;
                let rhs = use_value_boxed(cx.builder, cx.variables, args[1])?;
                let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
                let lhs_masked = cx.builder.ins().band_imm_s(lhs, box_base);
                let lhs_is_f64 = cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    lhs_masked,
                    box_base,
                );
                let rhs_masked = cx.builder.ins().band_imm_s(rhs, box_base);
                let rhs_is_f64 = cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    rhs_masked,
                    box_base,
                );
                let both_f64 = cx.builder.ins().band(lhs_is_f64, rhs_is_f64);

                let fast_block = cx.builder.create_block();
                let slow_block = cx.builder.create_block();
                let merge_block = cx.builder.create_block();
                cx.builder.append_block_param(merge_block, types::I64);

                cx.builder
                    .ins()
                    .brif(both_f64, fast_block, &[], slow_block, &[]);

                cx.builder.switch_to_block(fast_block);
                cx.builder.seal_block(fast_block);
                let lhs_f64 = unbox_f64(cx.builder, lhs);
                let rhs_f64 = unbox_f64(cx.builder, rhs);
                let fast_result =
                    emit_f64_abstract_compare(cx.builder, lhs_f64, rhs_f64, reverse, invert);
                cx.builder
                    .ins()
                    .jump(merge_block, &[ir::BlockArg::Value(fast_result)]);

                cx.builder.switch_to_block(slow_block);
                cx.builder.seal_block(slow_block);
                let slow_result = cx.call(
                    u32::from(Builtin::AbstractCompare.wire_id()),
                    &[lhs, rhs, reverse, invert],
                    feedback_ptr,
                )?;
                cx.builder
                    .ins()
                    .jump(merge_block, &[ir::BlockArg::Value(slow_result)]);

                cx.builder.switch_to_block(merge_block);
                cx.builder.seal_block(merge_block);
                let result = cx.builder.block_params(merge_block)[0];
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
        }
        // 已证明 f64 的单参数 Math builtin：直接发 CLIF 浮点指令，零 host 往返。
        // guard 即类型检查——参数未证明 f64 时本 arm 不匹配，落到下方通用 dispatcher 路径。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin:
                builtin @ (Builtin::MathAbs
                | Builtin::MathSqrt
                | Builtin::MathCeil
                | Builtin::MathFloor
                | Builtin::MathTrunc
                | Builtin::MathFround),
            args,
        } if tables.f64_values.contains(dest) && args.len() == 1 => {
            let input = use_value_f64(cx.builder, cx.variables, args[0])?;
            let result = match builtin {
                Builtin::MathAbs => cx.builder.ins().fabs(input),
                Builtin::MathSqrt => cx.builder.ins().sqrt(input),
                Builtin::MathCeil => cx.builder.ins().ceil(input),
                Builtin::MathFloor => cx.builder.ins().floor(input),
                Builtin::MathTrunc => cx.builder.ins().trunc(input),
                Builtin::MathFround => {
                    let narrowed = cx.builder.ins().fdemote(types::F32, input);
                    cx.builder.ins().fpromote(types::F64, narrowed)
                }
                _ => unreachable!("arm 模式已限定这六个 builtin"),
            };
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin:
                builtin @ (Builtin::StringCharCodeAt | Builtin::StringCharAt | Builtin::StringAt),
            args,
        } if matches!(args.len(), 1 | 2) => lower_string_char_builtin(
            cx,
            tables.barrier_thunks,
            *dest,
            *builtin,
            args,
            feedback_ptr,
        ),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::IsString,
            args,
        } if args.len() == 1 => {
            let encoded = use_value_boxed(cx.builder, cx.variables, args[0])?;
            let inline = emit_inline_string_predicate(cx.builder, encoded);
            let boxed = emit_is_boxed_handle(cx.builder, encoded);
            let tag = cx.builder.ins().ushr_imm_u(encoded, 32);
            let tag = cx.builder.ins().band_imm_u(
                tag,
                i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
            );
            let is_string = cx.builder.ins().icmp_imm_u(
                ir::condcodes::IntCC::Equal,
                tag,
                i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
            );
            let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
            let runtime_flag = cx.builder.ins().band_imm_u(
                tag_word,
                i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
            );
            let is_runtime =
                cx.builder
                    .ins()
                    .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
            let valid_handle = cx.builder.ins().band(boxed, is_string);
            let valid_handle = cx.builder.ins().band(valid_handle, is_runtime);
            let valid = cx.builder.ins().bor(inline, valid_handle);
            let yes = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(true));
            let no = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(false));
            let result = cx.builder.ins().select(valid, yes, no);
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StrictEq,
            args,
        } if args.len() == 2 => lower_strict_eq(
            cx,
            tables.barrier_thunks,
            *dest,
            args[0],
            args[1],
            StrictEqMode {
                slow_operation: u32::from(Builtin::StrictEq.wire_id()),
                invert: false,
            },
            feedback_ptr,
        ),
        // 非逃逸累加器追加：JIT 内联写入 payload 并就地更新 length；最后片段
        // 按运行时类型分派字符串直拷 / 安全整数 itoa，容量不足、builder 首建
        // 或其余形态回落宿主 thunk。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StringBuilderAppend,
            args,
        } if args.len() >= 2 => lower_string_builder_append(cx, *dest, args, feedback_ptr),
        Instruction::CallBuiltin {
            dest,
            builtin: Builtin::StringBuilderFinish,
            args,
        } if args.len() == 1 => {
            let builder = use_value_boxed(cx.builder, cx.variables, args[0])?;
            cx.flush()?;
            let call = cx
                .builder
                .ins()
                .call(cx.string_builder_finish, &[cx.ctx, builder]);
            if let Some(dest) = dest {
                let result = cx.builder.inst_results(call)[0];
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
        }
        // 已证明 f64 的 21 个 libm Math builtin：typed native direct call。
        // guard 即类型检查——实参未证明 f64 时落入下方 dispatcher 路径，
        // 保留 to_number_coerced 与 BigInt TypeError 语义。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } if tables.f64_values.contains(dest)
            && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                args.len() == usize::from(symbol.signature().argument_count())
            }) =>
        {
            let symbol = NativeHostSymbol::for_builtin(*builtin)
                .context("guard 已限制为 math thunk builtin")?;
            let thunk = import_math_thunk(
                cx.builder,
                tables.math_thunks,
                tables.imported_math_thunks,
                *builtin,
            )?;
            let result = match symbol.signature() {
                NativeSignature::F64Unary => {
                    let input = use_value_f64(cx.builder, cx.variables, args[0])?;
                    let call = cx.builder.ins().call(thunk, &[input]);
                    *cx.builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::F64Binary => {
                    let lhs = use_value_f64(cx.builder, cx.variables, args[0])?;
                    let rhs = use_value_f64(cx.builder, cx.variables, args[1])?;
                    let call = cx.builder.ins().call(thunk, &[lhs, rhs]);
                    *cx.builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::HostOperation
                | NativeSignature::ValueBinary
                | NativeSignature::ValueUnary
                | NativeSignature::ValueTernary
                | NativeSignature::ValueBinaryF64
                | NativeSignature::ZgcLoadBarrier
                | NativeSignature::ZgcStoreBarrier => {
                    unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
                }
            };
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StringSlice,
            args,
        } if !args.is_empty() => lower_string_slice_builtin(cx, *dest, args, feedback_ptr),
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ArrayPush,
            args,
        } if args.len() == 2 => {
            lower_array_push_inline(cx, tables.barrier_thunks, args[0], args[1])
        }
        Instruction::CallBuiltin {
            dest,
            builtin,
            args,
        } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(use_value_boxed(cx.builder, cx.variables, *arg)?);
            }
            let result = cx.call(u32::from(builtin.wire_id()), &values, feedback_ptr)?;
            if let Some(dest) = dest {
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
        }
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
            // callsite 只进宿主侧文案表（callsites_by_feedback_slot），
            // 代码生成不消费。
            callsite: _,
        } => {
            let direct_callee = tables
                .constant_defs
                .get(callee)
                .and_then(|c| tables.constants.get(c.0 as usize))
                .and_then(|c| match c {
                    Constant::FunctionRef(target) => Some(*target),
                    _ => None,
                });
            if let Some(target) = direct_callee
                && tables.direct_callable_functions.contains(&target)
                && let Some(decl) = tables.function_decls.get(target.0 as usize)
            {
                let func_ref = *tables
                    .imported_function_decls
                    .entry(target)
                    .or_insert_with(|| decl.import(cx.builder.func));
                if let Some(arity) = fast_js_arity(decl.signature()) {
                    lower_fast_direct_call_instruction(
                        cx, func_ref, *dest, *this_val, args, roots, arity,
                    )
                } else {
                    lower_direct_call_instruction(cx, func_ref, *dest, *this_val, args, roots)
                }
            } else {
                lower_call_instruction(
                    cx,
                    tables.slow_call_signature,
                    CallLowering {
                        destination: *dest,
                        callee: *callee,
                        this_value: *this_val,
                        args,
                        operation: NativeRuntimeOp::PrepareCall,
                        forward_args: false,
                    },
                    roots,
                    feedback_ptr,
                )
            }
        }
        Instruction::SuperCall {
            dest,
            callee,
            this_val,
            args,
            forward_args,
        } => lower_call_instruction(
            cx,
            tables.slow_call_signature,
            CallLowering {
                destination: *dest,
                callee: *callee,
                this_value: *this_val,
                args,
                operation: if *forward_args {
                    NativeRuntimeOp::PrepareSuperCallForward
                } else {
                    NativeRuntimeOp::PrepareSuperCall
                },
                forward_args: *forward_args,
            },
            roots,
            feedback_ptr,
        ),
        Instruction::ConstructCall {
            dest,
            callee,
            this_val,
            args,
            callsite: _,
        } => lower_call_instruction(
            cx,
            tables.slow_call_signature,
            CallLowering {
                destination: *dest,
                callee: *callee,
                this_value: *this_val,
                args,
                operation: NativeRuntimeOp::PrepareConstruct,
                forward_args: false,
            },
            roots,
            feedback_ptr,
        ),
        Instruction::StringConcatVa { dest, parts } => {
            lower_value_operation(cx, NativeRuntimeOp::StringConcat, parts, Some(*dest))
        }
        Instruction::NewPromise { dest } => {
            lower_native_object_allocation(cx, *dest, 2, false)?;
            let object = use_value_boxed(cx.builder, cx.variables, *dest)?;
            let initialized = cx.call(NativeRuntimeOp::InitPromise.id(), &[object], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, initialized)
        }
        Instruction::NewObject { dest, capacity } => {
            lower_native_object_allocation(cx, *dest, *capacity, false)
        }
        Instruction::GetProp {
            dest,
            object,
            key,
            latch: Some(guard),
            latch_template: Some(template),
        } => lower_get_prop_guarded(
            cx,
            tables,
            GuardedPropAccess {
                dest: *dest,
                object: *object,
                key: *key,
                guard: *guard,
                template: *template,
            },
            roots,
        ),
        Instruction::GetProp {
            dest, object, key, ..
        } => lower_get_prop_with_template_or_ic(
            cx,
            tables,
            tables.barrier_thunks,
            *dest,
            *object,
            *key,
            roots,
        ),
        Instruction::SetProp {
            dest,
            object,
            key,
            value,
            strict,
        } => lower_set_prop_with_template_or_ic(
            cx,
            tables,
            tables.barrier_thunks,
            *dest,
            *object,
            *key,
            *value,
            *strict,
        ),
        Instruction::CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => lower_value_operation(
            cx,
            NativeRuntimeOp::CreateDataProperty,
            &[*object, *key, *value],
            Some(*dest),
        ),
        Instruction::DeleteProp {
            dest,
            object,
            key,
            strict,
        } => lower_value_operation(
            cx,
            if *strict {
                NativeRuntimeOp::DeletePropStrict
            } else {
                NativeRuntimeOp::DeleteProp
            },
            &[*object, *key],
            Some(*dest),
        ),
        Instruction::SetProto { object, value } => {
            lower_value_operation(cx, NativeRuntimeOp::SetProto, &[*object, *value], None)
        }
        Instruction::NewArray { dest, capacity } => {
            lower_native_object_allocation(cx, *dest, *capacity, true)
        }
        Instruction::CloneArrayTemplate { dest, template } => {
            let template = cx.builder.ins().iconst(types::I64, i64::from(template.0));
            let result = cx.call(NativeRuntimeOp::CloneArrayTemplate.id(), &[template], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::InitObjectLiteral {
            dest,
            template,
            values,
        } => lower_init_object_literal(
            cx,
            tables.barrier_thunks,
            tables.constants,
            *dest,
            *template,
            values,
        ),
        Instruction::GetElem {
            dest,
            object,
            index,
            latch,
        } => lower_string_element(
            cx,
            tables.barrier_thunks,
            *dest,
            *object,
            *index,
            *latch,
            tables.speculative,
        ),
        Instruction::GuardElementsKind {
            dest,
            array,
            kind,
            template,
        } => {
            if let Some(template) = template {
                lower_elem_shape_guard(cx, tables.constants, *dest, *array, *template)
            } else {
                lower_guard_elements_kind(cx, *dest, *array, *kind)
            }
        }
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
            strict,
        } => lower_set_elem(
            cx,
            tables.barrier_thunks,
            *dest,
            *object,
            *index,
            *value,
            *strict,
            tables.speculative,
        ),
        Instruction::GetSuperBase { dest } => {
            let result = cx.call(NativeRuntimeOp::GetSuperBase.id(), &[], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::GetSuperConstructor { dest } => {
            let result = cx.call(NativeRuntimeOp::GetSuperConstructor.id(), &[], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::ObjectSpread {
            dest,
            object,
            source,
        } => lower_value_operation(
            cx,
            NativeRuntimeOp::ObjectSpread,
            &[*object, *source],
            // 结果槽：成功为 object，getter/Proxy 抛错为 TAG_EXCEPTION，
            // 丢弃它会吞掉 CopyDataProperties 的异常。
            Some(*dest),
        ),
        Instruction::GuardSameFunction {
            dest,
            callee,
            function,
        } => {
            let callee = use_value_boxed(cx.builder, cx.variables, *callee)?;
            let function = cx.builder.ins().iconst(types::I64, i64::from(function.0));
            let result = cx.call(
                NativeRuntimeOp::GuardSameFunction.id(),
                &[callee, function],
                None,
            )?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CollectRestArgs { dest, skip } => {
            let skip = cx.builder.ins().iconst(types::I64, i64::from(*skip));
            let result = cx.call(NativeRuntimeOp::CollectRestArguments.id(), &[skip], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::IsException { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let condition = emit_is_exception(cx.builder, input);
            let true_value = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(true));
            let false_value = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(false));
            let boolean = cx.builder.ins().select(condition, true_value, false_value);
            define_value_boxed(cx.builder, cx.variables, *dest, boolean)
        }
        Instruction::EncodeException { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let result = cx.call(NativeRuntimeOp::CreateException.id(), &[input], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::PromiseResolve { promise, value } => lower_builtin_operation(
            cx,
            Builtin::PromiseInstanceResolve,
            &[*promise, *value],
            None,
        ),
        Instruction::PromiseReject { promise, reason } => lower_builtin_operation(
            cx,
            Builtin::PromiseInstanceReject,
            &[*promise, *reason],
            None,
        ),
        Instruction::ExceptionToObject { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let result = cx.call(NativeRuntimeOp::ExceptionValue.id(), &[input], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::StoreVar { name, value } => {
            if let Some(local) = tables.frame_locals.get(name).copied() {
                // typed 局部与 typed 源同为浮点表示时这里不产出任何转换指令，
                // 归纳变量的回写因此留在浮点寄存器内。
                let typed_local = cx.variables.is_typed_local(name);
                let native = use_value_as(cx.builder, cx.variables, typed_local, *value)?;
                cx.builder.def_var(local, native);
                if let Some(index) = tables.frame_local_indices.get(name).copied() {
                    cx.update_pinned_local(index, native)?;
                }
                return Ok(());
            }
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            let slot = tables
                .variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
            let _ = cx.call(NativeRuntimeOp::StoreVar.id(), &[slot, value], None)?;
            Ok(())
        }
        Instruction::LoadVar { dest, name } => {
            if let Some(local) = tables.frame_locals.get(name).copied() {
                let typed_local = cx.variables.is_typed_local(name);
                let value = cx.builder.use_var(local);
                return if typed_local {
                    define_value_f64(cx.builder, cx.variables, *dest, value)
                } else {
                    define_value_boxed(cx.builder, cx.variables, *dest, value)
                };
            }
            let slot = tables
                .variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
            let value = cx.call(NativeRuntimeOp::LoadVar.id(), &[slot], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, value)
        }

        Instruction::Suspend { promise, state } => {
            let promise = use_value_boxed(cx.builder, cx.variables, *promise)?;
            let suspend_state = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let result = cx.call(
                Builtin::AsyncFunctionSuspend.wire_id().into(),
                &[promise, suspend_state],
                None,
            )?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::GeneratorSuspend { result, state } => {
            let result = use_value_boxed(cx.builder, cx.variables, *result)?;
            let continuation = cx.call(NativeRuntimeOp::LoadCallEnv.id(), &[], None)?;
            cx.publish_roots(roots, &[continuation])?;
            let slot = cx.builder.ins().iconst(types::I64, value::encode_f64(0.0));
            let suspend_state = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let _ = cx.call(
                Builtin::ContinuationSaveVar.wire_id().into(),
                &[continuation, slot, suspend_state],
                None,
            )?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::GuardTag { dest, value, tag } => lower_guard_tag(cx, *dest, *value, *tag),
        Instruction::GuardShape {
            dest,
            object,
            shape_id,
        } => lower_guard_shape(cx, *dest, *object, *shape_id),
        Instruction::GuardCallTarget {
            dest,
            callee,
            function,
        } => lower_guard_call_target(cx, *dest, *callee, *function),
        Instruction::LoadSlot {
            dest,
            object,
            index,
        } => lower_load_slot(cx, tables.barrier_thunks, *dest, *object, *index),
        Instruction::StoreSlot {
            dest,
            object,
            index,
            value,
            transition_shape,
        } => lower_store_slot(
            cx,
            tables.barrier_thunks,
            *dest,
            *object,
            *index,
            *value,
            *transition_shape,
        ),
        Instruction::DebugCheck { line, col } => {
            let function = cx
                .builder
                .ins()
                .iconst(types::I64, i64::from(tables.function_index));
            let line = cx.builder.ins().iconst(types::I64, i64::from(*line));
            let col = cx.builder.ins().iconst(types::I64, i64::from(*col));
            let _ = cx.call(
                NativeRuntimeOp::DebugCheck.id(),
                &[function, line, col],
                None,
            )?;
            Ok(())
        }
        unsupported => bail!("native lowering does not yet own instruction {unsupported}"),
    }
}
