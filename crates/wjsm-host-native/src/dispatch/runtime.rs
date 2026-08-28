use num_bigint::BigInt;
use num_traits::{FromPrimitive, Zero};
use wjsm_gc::HeapAccessV2Error;

use crate::gc::NativeGcError;
use wjsm_host::RuntimeString;
use wjsm_ir::{Constant, constants, value};
use wjsm_native_abi::{
    COOPERATIVE_POLL_BUDGET, NativeRuntimeOp, NativeVmContext, PendingExceptionKind,
};

use super::callable_chain::{self, CallableChainHit};
use super::property_write;
use crate::PropertyKey;
use crate::specialization::ValidatedFeedbackSlot;
use crate::{
    ASSIGNED_PROPERTY_FLAGS, NativeAgentState, NativeConstantMaterializeError, NativeRuntimeError,
};

pub(super) fn dispatch_runtime(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    operation: NativeRuntimeOp,
    args: &[i64],
    feedback_slot: Option<ValidatedFeedbackSlot>,
) -> i64 {
    match operation {
        NativeRuntimeOp::CooperativePoll => {
            // 生成代码在每个回边饱和减 `stack_budget_bytes`，耗尽后才进入本分支；
            // 重置预算使后续回边继续走内联快路径（否则 budget 恒为 0，每次回边
            // 都会重进本分支，内联退化为逐次 dispatcher 调用）。
            ctx.stack_budget_bytes = COOPERATIVE_POLL_BUDGET;
            crate::inspector::poll(ctx, state);
            if let Err(error) = state.collect_garbage_if_needed(ctx) {
                ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
                state
                    .stderr
                    .borrow_mut()
                    .extend_from_slice(error.to_string().as_bytes());
                return fail_dispatch(ctx);
            }
            let deadline = state.node_vm.current_deadline();
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                ctx.pending_exception_kind = PendingExceptionKind::Terminated;
                value::encode_handle(value::TAG_EXCEPTION, 0)
            } else {
                value::encode_undefined()
            }
        }
        NativeRuntimeOp::DebugCheck => {
            let [function, line, column] = args else {
                return fail_dispatch(ctx);
            };
            let (Ok(function), Ok(line), Ok(column)) = (
                u32::try_from(*function),
                u32::try_from(*line),
                u32::try_from(*column),
            ) else {
                return fail_dispatch(ctx);
            };
            crate::inspector::debug_check(ctx, state, function, line, column);
            value::encode_undefined()
        }
        NativeRuntimeOp::DeoptToGeneric => {
            let [function_id, block_id, env, this_value, _live_count] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(function_id) = u32::try_from(*function_id) else {
                return fail_dispatch(ctx);
            };
            let Ok(block_id) = u32::try_from(*block_id) else {
                return fail_dispatch(ctx);
            };
            ctx.resume_function_id = function_id;
            ctx.resume_block_plus_one = block_id.saturating_add(1);
            if ctx.function_table.is_null() || function_id >= ctx.function_table_len {
                return fail_dispatch(ctx);
            }
            // SAFETY: function_table 由当前 base image 钉扎，owner thread 同步调用。
            let entry = unsafe { &*ctx.function_table.add(function_id as usize) };
            // 类型 miss 后禁止立刻 OSR 回同一 overlay，避免 generic 头 ↔ overlay 死循环。
            entry
                .osr_entry
                .store(0, std::sync::atomic::Ordering::Release);
            state.evict_overlays_for_function(function_id);
            unsafe { (entry.slow_entry)(ctx, *env, *this_value, 0, 0) }
        }
        NativeRuntimeOp::StoreVar => {
            let [slot, stored] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(slot) = usize::try_from(*slot) else {
                return fail_dispatch(ctx);
            };
            let Some(destination) = state.variables.get_mut(slot) else {
                return fail_dispatch(ctx);
            };
            *destination = *stored;

            *stored
        }
        NativeRuntimeOp::LoadVar => {
            let [slot] = args else {
                return fail_dispatch(ctx);
            };
            usize::try_from(*slot)
                .ok()
                .and_then(|slot| state.variables.get(slot).copied())
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::IsTruthy => args
            .first()
            .map(|input| value::encode_bool(is_truthy(state, *input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        // MaterializeString 已随 install 期常量发布（4.1）退役；字符串常量由
        // 生成代码经 vmctx `string_constants_base` 直读，BigInt/RegExp 仍惰性物化。
        NativeRuntimeOp::MaterializeBigInt | NativeRuntimeOp::MaterializeRegExp => {
            let [index] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(index) = usize::try_from(*index) else {
                return fail_dispatch(ctx);
            };
            match state.materialize_constant(index, operation) {
                Ok(value) => value,
                Err(NativeConstantMaterializeError::InvalidRegExp(error)) => {
                    syntax_error(ctx, state, &error.to_string())
                }
                Err(NativeConstantMaterializeError::InternalInvariant) => fail_dispatch(ctx),
            }
        }
        NativeRuntimeOp::MaterializeFunction => {
            let [function_index] = args else {
                return fail_dispatch(ctx);
            };
            u32::try_from(*function_index)
                .ok()
                .and_then(|function_index| state.materialize_function(function_index))
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::StringConcat => {
            let mut parts = Vec::with_capacity(args.len());
            for part in args {
                let part = match to_runtime_string_coerced(ctx, state, *part) {
                    Ok(part) => part,
                    Err(exception) => return exception,
                };
                parts.push(part);
            }
            super::string::intern(ctx, state, RuntimeString::concat_many(parts))
        }
        NativeRuntimeOp::CloneArrayTemplate => {
            let [template] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(template) = usize::try_from(*template) else {
                return fail_dispatch(ctx);
            };
            let Some(Constant::ArrayTemplate(elements)) = state.constants.get(template) else {
                return fail_dispatch(ctx);
            };
            let mut values = Vec::with_capacity(elements.len());
            for element in elements {
                let Some(constant) = state.constants.get(element.0 as usize) else {
                    return fail_dispatch(ctx);
                };
                let encoded = match constant {
                    Constant::String(_) => state
                        .string_constants
                        .get(element.0 as usize)
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                    Constant::Number(number) => value::encode_f64(*number),
                    Constant::Bool(boolean) => value::encode_bool(*boolean),
                    Constant::Null => value::encode_null(),
                    Constant::Undefined => value::encode_undefined(),
                    _ => return fail_dispatch(ctx),
                };
                values.push(encoded);
            }
            state
                .allocate_array_values_with_gc_retry(ctx, &values)
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        NativeRuntimeOp::NewObject | NativeRuntimeOp::NewArray => {
            let [capacity] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(capacity) = u32::try_from(*capacity) else {
                return fail_dispatch(ctx);
            };
            allocate_object_or_out_of_memory(
                ctx,
                state,
                capacity,
                operation == NativeRuntimeOp::NewArray,
            )
        }
        NativeRuntimeOp::InitPromise => {
            let [object] = args else {
                return fail_dispatch(ctx);
            };
            super::promise::init_allocated_promise(ctx, state, *object)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::InitObjectLiteral => {
            let Some(template_index) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let Ok(template_index) = u32::try_from(template_index) else {
                return fail_dispatch(ctx);
            };
            init_object_literal_or_fail(ctx, state, template_index, &args[1..])
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::ElemShapeGuard => {
            let [array, template_index] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(template_index) = u32::try_from(*template_index) else {
                return fail_dispatch(ctx);
            };
            value::encode_bool(elem_shape_guard_holds(state, *array, template_index))
        }
        NativeRuntimeOp::GetProp => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            // 动态键（`o[k]++` / 解构计算键等）可能是对象，须先 ToPropertyKey 再入。
            let key = &match to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::GetPropIc => {
            let [object, key, ic_slot_ptr] = args else {
                return fail_dispatch(ctx);
            };
            let result =
                get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx));
            backfill_get_prop_ic(state, *object, *key, *ic_slot_ptr);
            result
        }
        NativeRuntimeOp::GetPropAccessor => {
            let [getter, receiver] = args else {
                return fail_dispatch(ctx);
            };
            if !value::is_callable(*getter) {
                return value::encode_undefined();
            }
            state
                .invoke_callable(ctx, *getter, *receiver, &[])
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::OptionalGetProp => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_null(*object) || value::is_undefined(*object) {
                value::encode_undefined()
            } else {
                get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx))
            }
        }
        NativeRuntimeOp::OptionalGetElem => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_null(*object) || value::is_undefined(*object) {
                return value::encode_undefined();
            }
            // 基座非空后才做 ToPropertyKey（可选链短路时不得再入用户转换）。
            let key = &match to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            if let Some(index) = array_index(state, *key) {
                if let Some(stored) =
                    super::typedarray::get_element_intern(state, *object, index as usize)
                {
                    return stored;
                }
                if value::is_array(*object) {
                    let handle = value::decode_handle(*object);
                    if state.gc.heap().array_kind(handle).ok()
                        != Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
                    {
                        match state.gc.heap().get_element(handle, index) {
                            Ok(Some(stored)) if !value::is_array_hole(stored as i64) => {
                                return stored as i64;
                            }
                            Ok(_) => {}
                            Err(_) => return fail_dispatch(ctx),
                        }
                    }
                }
            }
            get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::SetProp | NativeRuntimeOp::SetPropStrict => {
            let [object, key, stored] = args else {
                return fail_dispatch(ctx);
            };
            // PutValue 步骤 3.a：ToObject(base) 先于 ToPropertyKey，null/undefined
            // 基座须在键转换（可能执行用户代码）之前抛 TypeError。
            if let Some(exception) = set_on_nullish_receiver(ctx, state, *object, *key) {
                return exception;
            }
            // 动态键可能是对象，[[Set]]（含 proxy trap）须接收已转换的属性键。
            let key = match to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            let strict = operation == NativeRuntimeOp::SetPropStrict;
            if let Some(result) =
                set_on_primitive_receiver(ctx, state, *object, key, *stored, strict)
            {
                return result;
            }
            let completion = set_property_completion(ctx, state, *object, key, *stored);
            property_write::finish_property_set(
                ctx, state, *object, key, *stored, strict, completion,
            )
        }
        NativeRuntimeOp::CreateDataProperty => {
            let [object, key, stored] = args else {
                return fail_dispatch(ctx);
            };
            // CreateDataPropertyOrThrow 接收属性键：对象键先 ToPropertyKey 再入。
            let key = match to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            create_data_property_impl(ctx, state, *object, key, *stored)
        }
        NativeRuntimeOp::SetPropIc | NativeRuntimeOp::SetPropIcStrict => {
            let [object, key, stored, ic_slot_ptr] = args else {
                return fail_dispatch(ctx);
            };
            let strict = operation == NativeRuntimeOp::SetPropIcStrict;
            if let Some(result) =
                set_on_primitive_receiver(ctx, state, *object, *key, *stored, strict)
            {
                // 基元接收者不可训练 IC：退化 MEGAMORPHIC 后走宿主完整路径。
                backfill_set_prop_ic(state, *object, *key, false, *ic_slot_ptr);
                return result;
            }
            let completion = set_property_completion(ctx, state, *object, *key, *stored);
            // 只有真实写入成功才可训练 OWN_DATA：失败写入（如不可写自有数据
            // 属性）的槽位命中会让后续快路径绕过可写性检查直接改值。
            let success = matches!(completion, Ok(property_write::SetCompletion::Written));
            backfill_set_prop_ic(state, *object, *key, success, *ic_slot_ptr);
            property_write::finish_property_set(
                ctx, state, *object, *key, *stored, strict, completion,
            )
        }
        NativeRuntimeOp::DeleteProp => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            // `delete o[k]`：[[Delete]]（含 proxy trap）须接收已转换的属性键。
            let key = &match to_property_key_value(ctx, state, *key) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            if value::is_proxy(*object) {
                return super::proxy::dispatch_proxy(
                    ctx,
                    state,
                    wjsm_ir::Builtin::ReflectDeleteProperty,
                    &[*object, *key],
                )
                .expect("ReflectDeleteProperty is handled");
            }
            delete_property(state, *object, *key)
                .map(value::encode_bool)
                .unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::ObjectSpread => {
            let [destination, source] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_null(*source) || value::is_undefined(*source) {
                return *destination;
            }
            match super::object::copy_data_properties(ctx, state, *destination, *source, &[]) {
                Ok(()) => *destination,
                Err(exception) => exception,
            }
        }
        NativeRuntimeOp::SetProto => {
            let [_, proto] = args else {
                return fail_dispatch(ctx);
            };
            // 字面量 `__proto__:` 三态（§B.3.1）：对象或 null 走
            // [[SetPrototypeOf]]（null 由其写入 PROTO_NULL_SENTINEL，
            // 不可写 0——0 是合法句柄），其余值静默忽略。
            if value::is_null(*proto)
                || value::is_object(*proto)
                || value::is_array(*proto)
                || value::is_callable(*proto)
                || value::is_proxy(*proto)
            {
                let result = super::object::dispatch_object(
                    ctx,
                    state,
                    wjsm_ir::Builtin::ObjectSetPrototypeOf,
                    args,
                )
                .unwrap_or_else(|| fail_dispatch(ctx));
                if value::is_exception(result) {
                    return result;
                }
            }
            value::encode_undefined()
        }
        NativeRuntimeOp::GetSuperBase => super_base(state).unwrap_or_else(value::encode_undefined),
        NativeRuntimeOp::GetSuperConstructor => {
            super_constructor(state).unwrap_or_else(value::encode_undefined)
        }
        NativeRuntimeOp::GetElem => {
            let [object, index] = args else {
                return fail_dispatch(ctx);
            };
            // `o[k]`：[[Get]]（含 proxy trap）之前先做 ToPropertyKey 再入。
            let index = &match to_property_key_value(ctx, state, *index) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            if value::is_proxy(*object) {
                return super::proxy::get(ctx, state, *object, *index, *object);
            }
            if let Some(index) = array_index(state, *index)
                && let Some(stored) =
                    super::typedarray::get_element_intern(state, *object, index as usize)
            {
                return stored;
            }
            if value::is_array(*object)
                && let Some(index) = array_index(state, *index)
            {
                let handle = value::decode_handle(*object);
                if state.gc.heap().array_kind(handle).ok()
                    != Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
                {
                    match state.gc.heap().get_element(handle, index) {
                        Ok(Some(element))
                            if !value::is_array_hole(i64::from_ne_bytes(element.to_ne_bytes())) =>
                        {
                            return i64::from_ne_bytes(element.to_ne_bytes());
                        }
                        Ok(_) => {}
                        Err(_) => return fail_dispatch(ctx),
                    }
                }
            }
            get_property(ctx, state, *object, *index).unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::SetElem | NativeRuntimeOp::SetElemStrict => {
            let [object, index, stored] = args else {
                return fail_dispatch(ctx);
            };
            // PutValue 步骤 3.a：null/undefined 基座先于 ToPropertyKey 抛 TypeError。
            if let Some(exception) = set_on_nullish_receiver(ctx, state, *object, *index) {
                return exception;
            }
            // `o[k] = v`：[[Set]]（含 proxy trap）之前先做 ToPropertyKey 再入。
            let index = &match to_property_key_value(ctx, state, *index) {
                Ok(key) => key,
                Err(exception) => return exception,
            };
            let strict = operation == NativeRuntimeOp::SetElemStrict;
            // 基元接收者先行短路：decode_handle 对 SSO/基元产出无效句柄，
            // 后续 typed_arrays 等按句柄查表的分支不得先于本判定执行。
            if let Some(result) =
                set_on_primitive_receiver(ctx, state, *object, *index, *stored, strict)
            {
                return result;
            }
            let completion = set_element_completion(ctx, state, *object, *index, *stored);
            property_write::finish_property_set(
                ctx, state, *object, *index, *stored, strict, completion,
            )
        }
        NativeRuntimeOp::PrepareCall => state
            .prepare_call(ctx, args, false, feedback_slot)
            .unwrap_or_else(|| {
                state.prepare_rejected_call(
                    ctx,
                    args.first()
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                    false,
                )
            }),
        NativeRuntimeOp::PrepareConstruct => state
            .prepare_call(ctx, args, true, feedback_slot)
            .unwrap_or_else(|| {
                state.prepare_rejected_call(
                    ctx,
                    args.first()
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                    true,
                )
            }),
        NativeRuntimeOp::PrepareSuperCall | NativeRuntimeOp::PrepareSuperCallForward => state
            .prepare_super_call(
                ctx,
                args,
                operation == NativeRuntimeOp::PrepareSuperCallForward,
                feedback_slot,
            )
            .unwrap_or_else(|| {
                state.prepare_rejected_call(
                    ctx,
                    args.first()
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                    true,
                )
            }),
        NativeRuntimeOp::FinishCall => state.finish_call(ctx).unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::LoadArgument => state
            .load_argument(args)
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::LoadCallEnv => state
            .call_environment()
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::CollectRestArguments => state
            .collect_rest_arguments(ctx, args)
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::GuardSameFunction => {
            let [callable, function] = args else {
                return fail_dispatch(ctx);
            };
            value::encode_bool(
                u32::try_from(*function).ok().is_some_and(|function| {
                    state.callable_matches_local_function(*callable, function)
                }),
            )
        }
        NativeRuntimeOp::CreateException => {
            let Some(thrown) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let exception = state
                .create_exception(thrown)
                .unwrap_or_else(|| fail_dispatch(ctx));
            crate::inspector::pause_for_exception(ctx, state, thrown, false);
            exception
        }
        NativeRuntimeOp::ExceptionValue => args
            .first()
            .and_then(|exception| state.exception_value(*exception))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::BinaryAdd => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            binary_add(ctx, state, *left, *right)
        }
        NativeRuntimeOp::BinarySub => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntSub,
            |left, right| left - right,
        ),
        NativeRuntimeOp::BinaryMul => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntMul,
            |left, right| left * right,
        ),
        NativeRuntimeOp::BinaryDiv => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntDiv,
            |left, right| left / right,
        ),
        NativeRuntimeOp::BinaryMod => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntMod,
            |left, right| left % right,
        ),
        NativeRuntimeOp::BinaryExp => {
            numeric_or_bigint(ctx, state, args, wjsm_ir::Builtin::BigIntPow, f64::powf)
        }
        NativeRuntimeOp::BinaryBitAnd => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntBitAnd,
            |left, right| f64::from(to_int32(left) & to_int32(right)),
        ),
        NativeRuntimeOp::BinaryBitOr => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntBitOr,
            |left, right| f64::from(to_int32(left) | to_int32(right)),
        ),
        NativeRuntimeOp::BinaryBitXor => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntBitXor,
            |left, right| f64::from(to_int32(left) ^ to_int32(right)),
        ),
        NativeRuntimeOp::BinaryShl => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntShl,
            |left, right| f64::from(to_int32(left) << (to_uint32(right) & 31)),
        ),
        NativeRuntimeOp::BinaryShr => numeric_or_bigint(
            ctx,
            state,
            args,
            wjsm_ir::Builtin::BigIntShr,
            |left, right| f64::from(to_int32(left) >> (to_uint32(right) & 31)),
        ),
        NativeRuntimeOp::BinaryUShr => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            let left = match to_number_coerced(ctx, state, *left) {
                Ok(number) => number,
                Err(exception) => return exception,
            };
            let right = match to_number_coerced(ctx, state, *right) {
                Ok(number) => number,
                Err(exception) => return exception,
            };
            value::encode_f64(f64::from(to_uint32(left) >> (to_uint32(right) & 31)))
        }
        // `!` 自身不抛（ToBoolean 全定义），但 async 状态机等不插表达式级
        // 分叉的上下文里操作数可能是 TAG_EXCEPTION（如 `!=`/`!==` 先经
        // AbstractEq/StrictEq 透传），须原样透传而非折叠成布尔值吞掉。
        NativeRuntimeOp::UnaryNot => args
            .first()
            .map(|input| {
                if value::is_exception(*input) {
                    *input
                } else {
                    value::encode_bool(!is_truthy(state, *input))
                }
            })
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::UnaryNeg if args.first().is_some_and(|input| value::is_bigint(*input)) => {
            super::bigint::dispatch_bigint(ctx, state, wjsm_ir::Builtin::BigIntNeg, args)
                .expect("BigIntNeg is handled")
        }
        NativeRuntimeOp::UnaryNeg => unary_number(ctx, state, args, |number| -number),
        NativeRuntimeOp::UnaryPos => unary_number(ctx, state, args, |number| number),
        NativeRuntimeOp::UnaryBitNot
            if args.first().is_some_and(|input| value::is_bigint(*input)) =>
        {
            super::bigint::dispatch_bigint(ctx, state, wjsm_ir::Builtin::BigIntBitNot, args)
                .expect("BigIntBitNot is handled")
        }
        NativeRuntimeOp::UnaryBitNot => {
            unary_number(ctx, state, args, |number| f64::from(!to_int32(number)))
        }
        NativeRuntimeOp::UnaryVoid => value::encode_undefined(),
        NativeRuntimeOp::UnaryIsNullish => args
            .first()
            .map(|value| value::encode_bool(value::is_null(*value) || value::is_undefined(*value)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::UnaryDelete => value::encode_bool(true),
        NativeRuntimeOp::CompareStrictEq | NativeRuntimeOp::CompareStrictNotEq => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            // 操作数求值异常按求值顺序透传（同 Builtin::StrictEq），
            // 不得当普通值比较后吞掉。
            if value::is_exception(*left) {
                return *left;
            }
            if value::is_exception(*right) {
                return *right;
            }
            let equal = strict_equal(state, *left, *right);
            value::encode_bool(if operation == NativeRuntimeOp::CompareStrictEq {
                equal
            } else {
                !equal
            })
        }
    }
}

pub(super) fn binary_add(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    left: i64,
    right: i64,
) -> i64 {
    let left = match to_primitive(ctx, state, left, PrimitiveHint::Default) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let right = match to_primitive(ctx, state, right, PrimitiveHint::Default) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    if value::is_string(left) || value::is_string(right) {
        let left = match primitive_to_runtime_string(ctx, state, left) {
            Ok(text) => text,
            Err(exception) => return exception,
        };
        let right = match primitive_to_runtime_string(ctx, state, right) {
            Ok(text) => text,
            Err(exception) => return exception,
        };
        super::string::intern(ctx, state, RuntimeString::concat(left, right))
    } else if value::is_bigint(left) || value::is_bigint(right) {
        super::bigint::dispatch_bigint(ctx, state, wjsm_ir::Builtin::BigIntAdd, &[left, right])
            .expect("BigIntAdd is handled")
    } else {
        binary_number(ctx, state, &[left, right], |left, right| left + right)
    }
}

/// CreateDataPropertyOrThrow（ES §7.3.7）：在 receiver 上定义自有数据属性
/// { value, writable/enumerable/configurable: true }。区别于 [[Set]]：原型链
/// setter 一律不触发；自有访问器整体替换为数据属性（静态字段覆盖先前同名
/// 静态访问器）；既有不可配置属性按 ValidateAndApplyPropertyDescriptor 拒绝
/// （desc 恒要求 configurable: true）。receiver 覆盖类字段初始化全部形态：
/// 普通对象（实例 `this`）、callable（静态字段的类构造器）、array
/// （`class C extends Array` 实例）、Proxy（基类构造器返回 Proxy 时走
/// [[DefineOwnProperty]] trap）。`key_value` 已完成 ToPropertyKey。
fn create_data_property_impl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key_value: i64,
    stored: i64,
) -> i64 {
    let configurable = constants::FLAG_CONFIGURABLE as u32;
    if value::is_proxy(object) {
        let descriptor = super::object::full_data_descriptor(ctx, state, stored);
        if value::is_exception(descriptor) {
            return descriptor;
        }
        return super::proxy::define_property(ctx, state, object, key_value, descriptor);
    }
    if value::is_array(object) && state.text_matches(key_value, "length") {
        let Some(length) = array_length(state, stored) else {
            return range_error(ctx, state, "Invalid array length");
        };
        return state
            .gc
            .heap()
            .set_array_length(value::decode_handle(object), length)
            .map(|()| object)
            .unwrap_or_else(|_| fail_dispatch(ctx));
    }
    let Some(key) = property_key(state, key_value) else {
        return fail_dispatch(ctx);
    };
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        let frozen = state
            .array_property_flags
            .get(&(handle, key))
            .is_some_and(|flags| flags & configurable == 0)
            || state
                .array_accessors
                .get(&(handle, key))
                .is_some_and(|(_, _, flags)| flags & configurable == 0);
        if frozen {
            return type_error(ctx, state, "Cannot redefine non-configurable property");
        }
        state.array_accessors.remove(&(handle, key));
        state.note_array_property(handle, key);
        state.array_properties.insert((handle, key), stored);
        state
            .array_property_flags
            .insert((handle, key), ASSIGNED_PROPERTY_FLAGS);
        return object;
    }
    if value::is_callable(object) {
        let callable = value::strip_gc_color(object);
        if state
            .callable_property_flags
            .get(&(callable, key))
            .is_some_and(|flags| flags & configurable == 0)
        {
            return type_error(ctx, state, "Cannot redefine non-configurable property");
        }
        state.callable_accessors.remove(&(callable, key));
        state.callable_properties.insert((callable, key), stored);
        state
            .callable_property_flags
            .insert((callable, key), ASSIGNED_PROPERTY_FLAGS);
        return object;
    }
    if !value::is_object(object) {
        return fail_dispatch(ctx);
    }
    let handle = value::decode_object_handle(object);
    let current = match state.gc.heap().get_property_slot(handle, key) {
        Ok(current) => current,
        Err(_) => return fail_dispatch(ctx),
    };
    match current {
        Some(current) if current.flags & configurable == 0 => {
            return type_error(ctx, state, "Cannot redefine non-configurable property");
        }
        None if state.non_extensible_objects.contains(&handle) => {
            return type_error(
                ctx,
                state,
                "Cannot define property on a non-extensible object",
            );
        }
        _ => {}
    }
    match define_data_property_or_out_of_memory(ctx, state, handle, key, stored as u64) {
        Ok(()) => object,
        Err(exception) => exception,
    }
}

/// `define_data_property` 的 TLAB / OOM 重试包装：语义同
/// `set_property_or_out_of_memory`，但按 OrdinaryDefineOwnProperty 整槽重定义
/// （清除 ACCESSOR 位并写入完整数据属性特性），而非沿用既有槽位特性。
fn define_data_property_or_out_of_memory(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    key: PropertyKey,
    stored: u64,
) -> Result<(), i64> {
    let define = |state: &mut NativeAgentState| {
        state
            .gc
            .heap()
            .define_data_property(handle, key, stored, ASSIGNED_PROPERTY_FLAGS)
    };
    match define(state) {
        Ok(()) => Ok(()),
        Err(HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state
                .gc
                .flush_native_tlab(ctx)
                .map_err(|_| fail_dispatch(ctx))?;
            define(state).map_err(|_| fail_dispatch(ctx))
        }
        Err(HeapAccessV2Error::HeapExhausted { .. }) => {
            state.collect_garbage(ctx).map_err(|_| fail_dispatch(ctx))?;
            define(state).map_err(|_| fail_dispatch(ctx))
        }
        Err(_) => Err(fail_dispatch(ctx)),
    }
}

/// 完整命名属性 [[Set]] 语义：proxy / 数组 length / regexp lastIndex /
/// 数组命名属性 / callable 命名属性 / 普通对象 `ordinary_set`。
/// 基元接收者（含 null/undefined）由调用方先行短路。
fn set_property_completion(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    stored: i64,
) -> property_write::SetResult {
    if value::is_proxy(object) {
        return super::proxy::set(ctx, state, object, key, stored, object);
    }
    if value::is_array(object) && state.text_matches(key, "length") {
        return set_array_length_completion(ctx, state, object, stored);
    }
    if value::is_regexp(object) && state.text_matches(key, "lastIndex") {
        let result = super::regexp::set_last_index(ctx, state, &[object, stored]);
        if value::is_exception(result) {
            return Err(result);
        }
        return Ok(property_write::SetCompletion::Written);
    }
    let Some(key) = property_key(state, key) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_array(object) {
        return property_write::set_array_named_property(ctx, state, object, key, stored);
    }
    if value::is_callable(object) {
        // callable 接收者的完整 [[Set]]：链上 setter / 可写性拒绝 / 自有属性写入。
        return callable_chain::set_with_receiver(ctx, state, object, key, stored, object);
    }
    let receiver = object;
    ordinary_set(
        ctx,
        state,
        receiver,
        encoded_property_key(key),
        stored,
        receiver,
    )
}

/// 完整按键（含数字下标）[[Set]] 语义：proxy / typed array / 数组 length /
/// 字典数组下标覆盖 / 数组元素 / callable / 数组命名属性 / 普通对象。
/// 基元接收者（含 null/undefined）由调用方先行短路。
fn set_element_completion(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    index: i64,
    stored: i64,
) -> property_write::SetResult {
    use property_write::{SetCompletion, SetFailure};
    if value::is_proxy(object) {
        return super::proxy::set(ctx, state, object, index, stored, object);
    }
    if let Some(array) = state
        .typed_arrays
        .get(&value::decode_handle(object))
        .cloned()
    {
        if let Some(index) = array_index(state, index) {
            if index as usize >= array.length {
                // IntegerIndexedElementSet：越界写入静默成功（strict 亦不抛）。
                return Ok(SetCompletion::Written);
            }
            if array.kind.is_bigint() != value::is_bigint(stored) {
                return Err(type_error(ctx, state, "Cannot convert value to a BigInt"));
            }
            return super::typedarray::set_element(state, object, index as usize, stored)
                .map(|_| SetCompletion::Written)
                .ok_or_else(|| fail_dispatch(ctx));
        }
        if value::is_f64(index) {
            return Ok(SetCompletion::Written);
        }
    }
    if value::is_array(object) && state.text_matches(index, "length") {
        return set_array_length_completion(ctx, state, object, stored);
    }
    if value::is_array(object)
        && let Some(index) = array_index(state, index)
    {
        let handle = value::decode_handle(object);
        if state.gc.heap().array_kind(handle).ok()
            == Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
        {
            let Some(key) = property_key(state, value::encode_f64(f64::from(index))) else {
                return Err(fail_dispatch(ctx));
            };
            if let Some((_, setter, _)) = state.array_accessors.get(&(handle, key)).copied() {
                if !value::is_callable(setter) {
                    return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
                }
                let result = state
                    .invoke_callable(ctx, setter, object, &[stored])
                    .ok_or_else(|| fail_dispatch(ctx))?;
                if value::is_exception(result) {
                    return Err(result);
                }
                return Ok(SetCompletion::Written);
            }
            // flags 条目独立于覆盖层取值存在（seal/freeze 只迁移特性，
            // 元素值仍以元素存储为准），可写性检查先于取值条目。
            if state
                .array_property_flags
                .get(&(handle, key))
                .is_some_and(|flags| flags & wjsm_ir::constants::FLAG_WRITABLE as u32 == 0)
            {
                return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
            }
            if state.array_properties.contains_key(&(handle, key)) {
                state.array_properties.insert((handle, key), stored);
                // 覆盖层写入成功后同步在范围内的元素存储，保持 render /
                // 迭代等直读元素的路径与 [[Get]] 一致。
                if state
                    .gc
                    .heap()
                    .array_length(handle)
                    .is_ok_and(|length| index < length)
                {
                    let _ = state.gc.heap().set_element(
                        handle,
                        index,
                        u64::from_ne_bytes(stored.to_ne_bytes()),
                    );
                }
                return Ok(SetCompletion::Written);
            }
        }
        // OrdinarySet 步骤 2–3：元素不存在（越界或 hole）且数组不可扩展时
        // 拒绝创建；已有元素的更新不受 [[PreventExtensions]] 影响。
        if state.non_extensible_objects.contains(&handle) {
            let length = state
                .gc
                .heap()
                .array_length(handle)
                .map_err(|_| fail_dispatch(ctx))?;
            let exists = index < length
                && matches!(
                    state.gc.heap().get_element(handle, index),
                    Ok(Some(element)) if !value::is_array_hole(element as i64)
                );
            if !exists {
                return Ok(SetCompletion::Failed(SetFailure::NotExtensible));
            }
        }
        return state
            .gc
            .heap()
            .set_element(handle, index, u64::from_ne_bytes(stored.to_ne_bytes()))
            .map(|()| SetCompletion::Written)
            .or_else(|error| match error {
                wjsm_gc::HeapAccessV2Error::NativeTlabNeedsMaterialization { .. } => {
                    state
                        .gc
                        .flush_native_tlab(ctx)
                        .map_err(|_| fail_dispatch(ctx))?;
                    state
                        .gc
                        .heap()
                        .set_element(handle, index, u64::from_ne_bytes(stored.to_ne_bytes()))
                        .map(|()| SetCompletion::Written)
                        .map_err(|_| fail_dispatch(ctx))
                }
                _ => Err(fail_dispatch(ctx)),
            });
    }
    let Some(key) = property_key(state, index) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_callable(object) {
        // callable 接收者的完整 [[Set]]：链上 setter / 可写性拒绝 / 自有属性写入。
        return callable_chain::set_with_receiver(ctx, state, object, key, stored, object);
    }
    if value::is_array(object) {
        return property_write::set_array_named_property(ctx, state, object, key, stored);
    }
    ordinary_set(
        ctx,
        state,
        object,
        encoded_property_key(key),
        stored,
        object,
    )
}

/// 数组 length 赋值的 [[Set]]（OrdinarySet → ArraySetLength）：
/// 不可写 length 先于取值比较拒绝（写相同值也失败，与 V8 一致）；收缩遇
/// 不可配置元素时按 ArraySetLength 步骤 19 停在最高被阻塞下标 + 1 并报告
/// 失败（strict 抛 "Cannot delete property"）。收缩同步清除被删下标的字典
/// 覆盖层条目，保持覆盖层与元素存储一致。
pub(super) fn set_array_length_completion(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
    stored: i64,
) -> property_write::SetResult {
    use property_write::{SetCompletion, SetFailure};
    let Some(new_length) = array_length(state, stored) else {
        return Err(range_error(ctx, state, "Invalid array length"));
    };
    let handle = value::decode_handle(array);
    if state.array_fixed_length.contains(&handle) {
        return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
    }
    let mut final_length = new_length;
    let mut blocked = None;
    let shrinking = state
        .gc
        .heap()
        .array_length(handle)
        .is_ok_and(|old_length| new_length < old_length);
    if shrinking
        && state.gc.heap().array_kind(handle).ok()
            == Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
    {
        if let Some(index) = highest_non_configurable_element(state, handle, new_length) {
            final_length = index + 1;
            blocked = Some(SetFailure::NonDeletableElement(index));
        }
        remove_overlay_elements_from(state, handle, final_length);
    }
    state
        .gc
        .heap()
        .set_array_length(handle, final_length)
        .map_err(|_| fail_dispatch(ctx))?;
    match blocked {
        Some(failure) => Ok(SetCompletion::Failed(failure)),
        None => Ok(SetCompletion::Written),
    }
}

/// 字典覆盖层中下标 ≥ `from` 且不可配置的最大元素下标（数据或访问器条目）。
fn highest_non_configurable_element(
    state: &NativeAgentState,
    handle: u32,
    from: u32,
) -> Option<u32> {
    let configurable = constants::FLAG_CONFIGURABLE as u32;
    let data = state
        .array_property_flags
        .iter()
        .filter(|((owner, _), flags)| *owner == handle && *flags & configurable == 0)
        .filter_map(|((_, key), _)| array_index(state, encoded_property_key(*key)));
    let accessors = state
        .array_accessors
        .iter()
        .filter(|((owner, _), (_, _, flags))| *owner == handle && *flags & configurable == 0)
        .filter_map(|((_, key), _)| array_index(state, encoded_property_key(*key)));
    data.chain(accessors).filter(|index| *index >= from).max()
}

/// 移除字典覆盖层中下标 ≥ `from` 的全部元素条目（值 / 特性 / 访问器 /
/// 枚举顺序），配合 length 收缩防止覆盖层残留已删除下标的旧值。
fn remove_overlay_elements_from(state: &mut NativeAgentState, handle: u32, from: u32) {
    let removed: Vec<PropertyKey> = state
        .array_property_flags
        .keys()
        .chain(state.array_properties.keys())
        .chain(state.array_accessors.keys())
        .filter(|(owner, _)| *owner == handle)
        .map(|(_, key)| *key)
        .filter(|key| {
            array_index(state, encoded_property_key(*key)).is_some_and(|index| index >= from)
        })
        .collect();
    for key in removed {
        state.array_properties.remove(&(handle, key));
        state.array_property_flags.remove(&(handle, key));
        state.array_accessors.remove(&(handle, key));
        state.forget_array_property(handle, key);
    }
}

/// 值是否为 ECMAScript 基元（含 null / undefined）。
fn is_primitive_value(encoded: i64) -> bool {
    value::is_null(encoded)
        || value::is_undefined(encoded)
        || value::is_string(encoded)
        || value::is_f64(encoded)
        || value::is_bool(encoded)
        || value::is_symbol(encoded)
        || value::is_bigint(encoded)
}

/// PutValue 步骤 3.a：ToObject 对 null/undefined 基座直接抛 TypeError（与
/// strict 无关），且先于 ToPropertyKey——后者可能执行用户代码，其副作用不得
/// 在本 TypeError 之前发生。返回 `None` 表示基座不是 null/undefined。
fn set_on_nullish_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Option<i64> {
    if !value::is_null(object) && !value::is_undefined(object) {
        return None;
    }
    let base = if value::is_null(object) {
        "null"
    } else {
        "undefined"
    };
    let message = format!(
        "Cannot set properties of {base} (setting '{}')",
        render_value(state, key)
    );
    Some(type_error(ctx, state, &message))
}

/// PutValue 对基元 base 的 [[Set]] 终局（OrdinarySetWithOwnDescriptor 步骤
/// 3.d.iv：Receiver 非对象时数据属性写入必然失败）：
/// - null / undefined base：ToObject 直接抛 TypeError（与 strict 无关）；
/// - 其余基元（string / number / boolean / symbol / bigint）：sloppy 返回
///   stored（静默 no-op），strict 抛 TypeError。字符串奇异对象的 in-range
///   下标与 length 是自有不可写数据属性，错误措辞区分 read only 与 create。
///
/// 返回 `None` 表示接收者不是基元，调用方继续走对象路径。
///
/// 当前引擎的基元原型链只暴露内建方法（`primitive_property`），用户对
/// String.prototype 等的扩展在 [[Get]] 路径同样不可见，因此这里不存在可
/// 命中的用户 accessor setter；若未来打通用户可扩展基元原型，Get/Set 两侧
/// 需一并补上原型链 accessor 查找。
fn set_on_primitive_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    stored: i64,
    strict: bool,
) -> Option<i64> {
    if let Some(exception) = set_on_nullish_receiver(ctx, state, object, key) {
        return Some(exception);
    }
    let type_name = if value::is_string(object) {
        "string"
    } else if value::is_f64(object) {
        "number"
    } else if value::is_bool(object) {
        "boolean"
    } else if value::is_symbol(object) {
        "symbol"
    } else if value::is_bigint(object) {
        "bigint"
    } else {
        return None;
    };
    if !strict {
        return Some(stored);
    }
    let key_text = render_value(state, key);
    let rendered = render_value(state, object);
    // 字符串自有 in-range 下标 / length 是不可写数据属性；其余键在基元
    // 接收者上创建数据属性必然失败（Receiver 非对象）。
    let own_readonly = value::is_string(object)
        && (state.text_matches(key, "length")
            || array_index(state, key)
                .is_some_and(|index| (index as usize) < state.string_len(object).unwrap_or(0)));
    let message = if own_readonly {
        format!("Cannot assign to read only property '{key_text}' of string '{rendered}'")
    } else {
        format!("Cannot create property '{key_text}' on {type_name} '{rendered}'")
    };
    Some(type_error(ctx, state, &message))
}

/// 写满整个 32 字节 IC 槽；未用字段清零，防止残留旧值参与后续比较。
///
/// # Safety
/// `slot` 必须指向当前 image IC 区内某个 32 字节槽的首个 u32，且仅本 owner 线程写。
unsafe fn write_ic_slot(
    slot: *mut u32,
    shape_id: u32,
    value_index: u32,
    kind: u32,
    proto_generation: u32,
    holder_handle: u32,
    expected_proto: u32,
) {
    // SAFETY: 调用方保证 slot 指向 8 个连续 u32；std::ptr::write 只写不读。
    unsafe {
        std::ptr::write(slot, shape_id);
        std::ptr::write(slot.add(1), value_index);
        std::ptr::write(slot.add(2), kind);
        std::ptr::write(slot.add(3), proto_generation);
        std::ptr::write(slot.add(4), holder_handle);
        std::ptr::write(slot.add(5), expected_proto);
        std::ptr::write(slot.add(6), 0);
        std::ptr::write(slot.add(7), 0);
    }
}

/// SAFETY: 同 [`write_ic_slot`]。
unsafe fn write_ic_slot_megamorphic(slot: *mut u32) {
    // SAFETY: 调用方保证 slot 有效；退化槽 shape/value 清零后只留 kind。
    // trio 规划槽保留 site marker，后续 miss 仍按三键一次回填，避免共享槽被写成单键 OWN_DATA。
    let keep_trio = unsafe { ic_slot_is_trio_site(slot) };
    unsafe { write_ic_slot(slot, 0, 0, constants::IC_KIND_MEGAMORPHIC, 0, 0, 0) };
    if keep_trio {
        unsafe { std::ptr::write(slot.add(6), constants::IC_SLOT_TRIO_SITE_MARKER) };
    }
}

unsafe fn ic_slot_is_trio_site(slot: *mut u32) -> bool {
    // SAFETY: 与 write_ic_slot 相同的槽指针契约；只读 kind 与 reserved1。
    let kind = unsafe { std::ptr::read(slot.add(2)) };
    let marker = unsafe { std::ptr::read(slot.add(6)) };
    kind == constants::IC_KIND_OWN_DATA_TRIO || marker == constants::IC_SLOT_TRIO_SITE_MARKER
}

fn backfill_trio_ic_slot(state: &mut NativeAgentState, handle: u32, slot: *mut u32) -> bool {
    let Some(name_key) = state.intern_property_string("name".into()) else {
        return false;
    };
    let Some(value_key) = state.intern_property_string("value".into()) else {
        return false;
    };
    let Some(length_key) = state.intern_property_string("length".into()) else {
        return false;
    };
    let Ok(Some((shape_name, idx_name))) =
        state.gc.heap().own_data_property_index(handle, name_key)
    else {
        return false;
    };
    let Ok(Some((shape_value, idx_value))) =
        state.gc.heap().own_data_property_index(handle, value_key)
    else {
        return false;
    };
    let Ok(Some((shape_length, idx_length))) =
        state.gc.heap().own_data_property_index(handle, length_key)
    else {
        return false;
    };
    if shape_name != shape_value || shape_value != shape_length {
        return false;
    }
    // SAFETY: 同 write_ic_slot；trio 布局复用 holder/expected_proto 存放 value/length 下标。
    unsafe {
        write_ic_slot(
            slot,
            shape_name,
            idx_name,
            constants::IC_KIND_OWN_DATA_TRIO,
            0,
            idx_value,
            idx_length,
        );
    }
    true
}

/// SetPropIc 的 miss 回填：仅在写入真实成功后，若属性已成为接收者自己的数据
/// 属性，回填 `(shape_id, value_index)`（shape 迁移由 heap 在 `set_property`
/// 内完成，这里读的是写入后的新 shape）；写失败（如不可写自有数据属性——
/// 其 own_data_property_index 仍可命中，训练后快路径会绕过可写性检查直接改
/// 值）/ accessor / proxy / 数组 / 字典 shape / 异常一律永久退化
/// MEGAMORPHIC，此后每次写入都走宿主完整 [[Set]]。
fn backfill_set_prop_ic(
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    success: bool,
    ic_slot_ptr: i64,
) {
    let ic_slot_ptr = ic_slot_ptr as *mut u32;
    if !success || !value::is_object(object) {
        // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算。
        unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        return;
    }
    let handle = value::decode_object_handle(object);
    // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算。
    if unsafe { ic_slot_is_trio_site(ic_slot_ptr) } {
        if !backfill_trio_ic_slot(state, handle, ic_slot_ptr) {
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        }
        return;
    }
    let Some(name_id) = property_key(state, key) else {
        // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算。
        unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        return;
    };
    match state.gc.heap().own_data_property_index(handle, name_id) {
        Ok(Some((shape_id, value_index))) => {
            // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算，
            // IC 区基址 16 字节对齐、槽内 8 个 u32 不越界；只在本 owner 线程写。
            unsafe {
                write_ic_slot(
                    ic_slot_ptr,
                    shape_id,
                    value_index,
                    constants::IC_KIND_OWN_DATA,
                    0,
                    0,
                    0,
                )
            };
        }
        _ => {
            // SAFETY: 退化槽覆盖整个 32 字节，kind 重写清除残留命中。
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        }
    }
}

fn super_constructor(state: &mut NativeAgentState) -> Option<i64> {
    let home_object = state.activations.last()?.home_object?;
    let constructor = match home_object {
        wjsm_ir::HomeObject::Prototype(function) | wjsm_ir::HomeObject::Constructor(function) => {
            state.materialize_function(function.0)?
        }
    };
    state
        .callable_prototypes
        .get(&value::strip_gc_color(constructor))
        .copied()
}

fn super_base(state: &mut NativeAgentState) -> Option<i64> {
    let activation = state.activations.last()?;
    let Some(home_object) = activation.home_object else {
        let environment = activation.environment;
        let home_key = state.intern_property_string("home".into())?;
        let home = state
            .gc
            .heap()
            .get_property(object_handle(environment)?, home_key)
            .ok()?? as i64;
        let prototype = state.gc.heap().prototype(object_handle(home)?).ok()?;
        return Some(if prototype == u32::MAX {
            value::encode_null()
        } else {
            value::encode_object_handle(prototype)
        });
    };
    let constructor = match home_object {
        wjsm_ir::HomeObject::Prototype(function) | wjsm_ir::HomeObject::Constructor(function) => {
            state.materialize_function(function.0)?
        }
    };
    match home_object {
        wjsm_ir::HomeObject::Constructor(_) => state
            .callable_prototypes
            .get(&value::strip_gc_color(constructor))
            .copied(),
        wjsm_ir::HomeObject::Prototype(_) => {
            let prototype_key = state.intern_property_string("prototype".into())?;
            let home = state.callable_property(constructor, prototype_key)?;
            let prototype = state.gc.heap().prototype(value::decode_handle(home)).ok()?;
            Some(if prototype == u32::MAX {
                value::encode_null()
            } else {
                value::encode_object_handle(prototype)
            })
        }
    }
}

pub(super) fn is_constructor_value(state: &NativeAgentState, encoded: i64) -> bool {
    // Proxy 的 [[Construct]] 在 target 可构造时存在（ProxyCreate 10.5.12）。
    if value::is_proxy(encoded) {
        return super::proxy::is_constructor_proxy(state, encoded);
    }
    if !value::is_callable(encoded) {
        return false;
    }
    match state.native_callable_kind(encoded) {
        Some(crate::NativeCallableKind::Intl(kind)) => super::intl::is_constructor(kind),
        Some(crate::NativeCallableKind::DateMethod(_)) => false,
        Some(crate::NativeCallableKind::FunctionPrototype) => false,
        Some(_) => true,
        None => state
            .callable_function(encoded)
            .is_some_and(|function| function.needs_prototype),
    }
}

pub(super) fn object_handle(encoded: i64) -> Option<u32> {
    (value::is_object(encoded) || value::is_array(encoded)).then(|| value::decode_handle(encoded))
}

fn heap_prototype_value(state: &NativeAgentState, object: i64) -> Result<Option<i64>, ()> {
    let handle = object_handle(object).ok_or(())?;
    let prototype = state.gc.heap().prototype(handle).map_err(|_| ())?;
    if prototype == wjsm_gc::PROTO_NULL_SENTINEL {
        return Ok(None);
    }
    if prototype & 0x8000_0000 != 0 {
        return Ok(Some(value::encode_proxy_handle(prototype & 0x7fff_ffff)));
    }
    let encoded = if state.gc.heap().object_type(prototype).ok()
        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
    {
        value::encode_handle(value::TAG_ARRAY, prototype)
    } else {
        value::encode_object_handle(prototype)
    };
    Ok(Some(encoded))
}

pub(super) fn ordinary_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: i64,
    stored: i64,
    receiver: i64,
) -> property_write::SetResult {
    let key = property_key(state, key).ok_or_else(|| fail_dispatch(ctx))?;
    ordinary_set_key(ctx, state, target, key, stored, receiver)
}

pub(super) fn ordinary_set_key(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: PropertyKey,
    stored: i64,
    receiver: i64,
) -> property_write::SetResult {
    use property_write::{SetCompletion, SetFailure};
    // callable 目标（如 Reflect.set 直接传函数、或对象链上出现 callable
    // 原型）：自有属性在宿主侧表，链行走走 callable 语义。
    if value::is_callable(target) {
        return callable_chain::set_with_receiver(ctx, state, target, key, stored, receiver);
    }
    let target_handle = object_handle(target).ok_or_else(|| fail_dispatch(ctx))?;
    let own = state
        .gc
        .heap()
        .get_property_slot(target_handle, key)
        .map_err(|_| fail_dispatch(ctx))?;
    if own.is_none() {
        let prototype = state
            .gc
            .heap()
            .prototype(target_handle)
            .map_err(|_| fail_dispatch(ctx))?;
        if prototype != wjsm_gc::PROTO_NULL_SENTINEL {
            if prototype & 0x8000_0000 != 0 {
                return super::proxy::set(
                    ctx,
                    state,
                    value::encode_proxy_handle(prototype & 0x7fff_ffff),
                    encoded_property_key(key),
                    stored,
                    receiver,
                );
            }
            return ordinary_set_key(
                ctx,
                state,
                value::encode_object_handle(prototype),
                key,
                stored,
                receiver,
            );
        }
    }
    if let Some(descriptor) = own {
        if descriptor.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
            let setter = descriptor.setter as i64;
            if !value::is_callable(setter) {
                return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
            }
            let result = state
                .invoke_callable(ctx, setter, receiver, &[stored])
                .ok_or_else(|| fail_dispatch(ctx))?;
            return if value::is_exception(result) {
                Err(result)
            } else {
                Ok(SetCompletion::Written)
            };
        }
        if descriptor.flags & constants::FLAG_WRITABLE as u32 == 0 {
            return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
        }
    }
    assign_data_property_to_receiver(ctx, state, receiver, key, stored)
}

/// GetPropIc 的 miss 回填：按「自有数据 → 原型链数据 → accessor」优先级回填
/// CLIF 快路径；proxy / 字典 shape / 数组 / 缺失 / 非 callable accessor 一律
/// 永久退化 MEGAMORPHIC（此后每次访问都走宿主完整 [[Get]]）。
fn backfill_get_prop_ic(state: &mut NativeAgentState, object: i64, key: i64, ic_slot_ptr: i64) {
    if !value::is_object(object) {
        return;
    }
    let ic_slot_ptr = ic_slot_ptr as *mut u32;
    let handle = value::decode_object_handle(object);
    // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算。
    if unsafe { ic_slot_is_trio_site(ic_slot_ptr) } {
        if !backfill_trio_ic_slot(state, handle, ic_slot_ptr) {
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        }
        return;
    }
    let Some(name_id) = property_key(state, key) else {
        return;
    };
    // 优先级 1：自有数据属性（最常见；单 load 快路径）。
    match state.gc.heap().own_data_property_index(handle, name_id) {
        Ok(Some((shape_id, value_index))) => {
            // SAFETY: ic_slot_ptr 由生成代码以 `ic_slots_base + slot * 32` 计算，
            // IC 区基址 16 字节对齐、槽内 8 个 u32 不越界；只在本 owner 线程写。
            unsafe {
                write_ic_slot(
                    ic_slot_ptr,
                    shape_id,
                    value_index,
                    constants::IC_KIND_OWN_DATA,
                    0,
                    0,
                    0,
                )
            };
            return;
        }
        Ok(None) => {}
        Err(_) => {
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
            return;
        }
    }
    // 优先级 2：原型链上的数据/accessor 属性。接收者 shape 和直接原型共同
    // 决定解析结果，链上形状变化另由 proto 世代覆盖。
    let shape_id = match state.gc.heap().shape_id(handle) {
        Ok(shape_id) => shape_id,
        Err(_) => {
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
            return;
        }
    };
    let expected_proto = match state.gc.heap().prototype(handle) {
        Ok(prototype) => prototype,
        Err(_) => {
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
            return;
        }
    };
    let generation = state.gc.heap().shapes().proto_generation();
    match state
        .gc
        .heap()
        .get_property_slot_on_proto_chain_for_ic(handle, name_id)
    {
        Ok(Some((holder_handle, value_slot_index, property))) => {
            // 命中链尾 %Object.prototype% 且宿主家族合成认领该名（Date 的
            // toString 等）时不可缓存 PROTO_DATA/ACCESSOR：快路径会绕过
            // 合成层直读 holder 槽，永久退化 MEGAMORPHIC 走完整 [[Get]]。
            if state.object_prototype.map(value::decode_handle) == Some(holder_handle)
                && state.primitive_property(object, key).is_some()
            {
                unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
                return;
            }
            if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                let getter = property.getter as i64;
                if value::is_callable(getter) {
                    // SAFETY: 与 OWN_DATA 回填相同的 slot 写条件。
                    unsafe {
                        write_ic_slot(
                            ic_slot_ptr,
                            shape_id,
                            value_slot_index,
                            constants::IC_KIND_ACCESSOR,
                            generation,
                            holder_handle,
                            expected_proto,
                        )
                    };
                } else {
                    unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
                }
            } else if holder_handle == handle {
                // 理论上 own_data_property_index 已覆盖；保留防御分支。
                unsafe {
                    write_ic_slot(
                        ic_slot_ptr,
                        shape_id,
                        value_slot_index,
                        constants::IC_KIND_OWN_DATA,
                        0,
                        0,
                        0,
                    )
                };
            } else {
                // SAFETY: 与 OWN_DATA 回填相同的 slot 写条件。
                unsafe {
                    write_ic_slot(
                        ic_slot_ptr,
                        shape_id,
                        value_slot_index,
                        constants::IC_KIND_PROTO_DATA,
                        generation,
                        holder_handle,
                        expected_proto,
                    )
                };
            }
        }
        Ok(None) | Err(_) => {
            // SAFETY: 退化槽覆盖整个 32 字节，残留旧命中由 kind 重写清除。
            unsafe { write_ic_slot_megamorphic(ic_slot_ptr) };
        }
    }
}

pub(super) fn property_key(state: &mut NativeAgentState, encoded: i64) -> Option<PropertyKey> {
    if value::is_inline_string(encoded) {
        return PropertyKey::inline_string(encoded);
    }
    if value::is_symbol(encoded) {
        return Some(PropertyKey::symbol(value::decode_handle(encoded)));
    }
    if value::is_string(encoded) {
        let text = state
            .string_owned(encoded)
            .unwrap_or_else(|| RuntimeString::from(render_value(state, encoded)));
        return state.intern_property_string(text);
    }
    state.intern_property_string(RuntimeString::from(render_value(state, encoded)))
}

pub(crate) fn encoded_property_key(key: PropertyKey) -> i64 {
    key.to_value()
}

pub(super) fn get_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
) -> Result<i64, ()> {
    get_property_with_receiver(ctx, state, object, key, object)
}

pub(super) fn get_property_with_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    receiver: i64,
) -> Result<i64, ()> {
    if value::is_null(object) || value::is_undefined(object) {
        return Ok(value::encode_undefined());
    }
    if let Some(property) = state.global_property(object, key) {
        return Ok(property);
    }
    // WHATWG URL 全局：惰性加载 node:url，与模块导出共享同一构造器。
    if state.global_object == Some(object) || super::node_vm::is_context(state, object) {
        let name = state.string_owned(key).and_then(|text| text.to_utf8());
        if let Some(name) = name.as_deref()
            && matches!(name, "URL" | "URLSearchParams")
            && let Some(property) = super::modules::ensure_url_global(ctx, state, name)
        {
            return Ok(property);
        }
    }
    if value::is_string(object)
        && let Some(index) = array_index(state, key)
    {
        let unit = state.string_code_unit(object, index as usize);
        return Ok(match unit {
            Some(unit) => intern_string_with_gc_retry(
                ctx,
                state,
                wjsm_host::RuntimeString::from_utf16_units(vec![unit]),
            ),
            None => value::encode_undefined(),
        });
    }
    if state
        .typed_arrays
        .contains_key(&value::decode_handle(object))
    {
        let property_name = state
            .string_owned(key)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
        if property_name == "buffer"
            && let Some(buffer) = state
                .typed_arrays
                .get(&value::decode_handle(object))
                .and_then(|array| array.buffer_object)
        {
            return Ok(buffer);
        }
        if let Some(builtin) = super::typedarray::typed_array_builtin(state, object, &property_name)
            && matches!(
                builtin,
                wjsm_ir::Builtin::TypedArrayProtoLength
                    | wjsm_ir::Builtin::TypedArrayProtoByteLength
                    | wjsm_ir::Builtin::TypedArrayProtoByteOffset
            )
        {
            return Ok(
                super::typedarray::dispatch_typed_array(ctx, state, builtin, &[object])
                    .unwrap_or_else(|| fail_dispatch(ctx)),
            );
        }
    }
    if state
        .array_buffers
        .contains_key(&value::decode_handle(object))
        || state.data_views.contains_key(&value::decode_handle(object))
    {
        let property_name = state
            .string_owned(key)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
        if state
            .array_buffers
            .contains_key(&value::decode_handle(object))
            && property_name == "byteLength"
        {
            return Ok(super::buffers::dispatch_buffer(
                ctx,
                state,
                wjsm_ir::Builtin::ArrayBufferProtoByteLength,
                &[object],
            )
            .unwrap_or_else(|| fail_dispatch(ctx)));
        }
        if let Some(view) = state.data_views.get(&value::decode_handle(object)).cloned() {
            return match property_name.as_str() {
                "byteLength" => Ok(value::encode_f64(view.length as f64)),
                "byteOffset" => Ok(value::encode_f64(view.offset as f64)),
                "buffer" => Ok(value::encode_object_handle(view.buffer)),
                _ => Ok(state
                    .primitive_property(object, key)
                    .unwrap_or_else(value::encode_undefined)),
            };
        }
    }
    if let Some(_sab) = state
        .shared_array_buffers
        .get(&value::decode_handle(object))
    {
        let property_name = state
            .string_owned(key)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
        let builtin = match property_name.as_str() {
            "byteLength" => Some(wjsm_ir::Builtin::SharedArrayBufferProtoByteLength),
            "growable" => Some(wjsm_ir::Builtin::SharedArrayBufferProtoGrowable),
            "maxByteLength" => Some(wjsm_ir::Builtin::SharedArrayBufferProtoMaxByteLength),
            _ => None,
        };
        if let Some(builtin) = builtin {
            return Ok(super::sab::dispatch_sab(ctx, state, builtin, &[object])
                .unwrap_or_else(|| fail_dispatch(ctx)));
        }
    }
    if value::is_proxy(object) {
        return Ok(super::proxy::get(ctx, state, object, key, receiver));
    }
    if value::is_regexp(object) {
        if let Some(property) = super::regexp::get_property(ctx, state, object, key) {
            return Ok(property);
        }
        // 旁挂标志位与合成方法未命中：沿 %RegExp.prototype%（堆对象，父为
        // %Object.prototype%）上行，使 hasOwnProperty 等继承成员可见。
        if let Some(prototype) = state.regexp_prototype {
            return get_property_with_receiver(ctx, state, prototype, key, receiver);
        }
        return Ok(value::encode_undefined());
    }
    if value::is_string(object) && state.text_matches(key, "length") {
        return state
            .string_len(object)
            .map(|length| value::encode_f64(length as f64))
            .ok_or(());
    }
    if value::is_array(object) && state.text_matches(key, "length") {
        return state
            .gc
            .heap()
            .array_length(value::decode_handle(object))
            .map(|length| value::encode_f64(f64::from(length)))
            .map_err(|_| ());
    }
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        let encoded_key = key;
        let key = property_key(state, encoded_key).ok_or(())?;
        if let Some((getter, _, _)) = state.array_accessors.get(&(handle, key)).copied() {
            return if value::is_callable(getter) {
                state.invoke_callable(ctx, getter, receiver, &[]).ok_or(())
            } else {
                Ok(value::encode_undefined())
            };
        }
        if let Some(stored) = state.array_properties.get(&(handle, key)).copied() {
            return Ok(stored);
        }
        if let Some(index) = array_index(state, encoded_key) {
            match state.gc.heap().get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => {
                    return Ok(element as i64);
                }
                Ok(_) => {}
                Err(_) => {
                    if state.collect_garbage(ctx).is_ok() {
                        let _ = state.gc.heap().finish_relocation_epoch();
                        let _ = state.gc.heap().advance_epoch_and_reclaim();
                        if let Ok(Some(element)) = state.gc.heap().get_element(handle, index)
                            && !value::is_array_hole(element as i64)
                        {
                            return Ok(element as i64);
                        }
                    }
                    return Err(());
                }
            }
        }
        if state.array_prototype == Some(object)
            && let Some(property) = state.primitive_property(object, encoded_key)
        {
            return Ok(property);
        }
        if let Some(prototype) = heap_prototype_value(state, object)? {
            return get_property_with_receiver(ctx, state, prototype, encoded_key, receiver);
        }
        return Ok(state
            .primitive_property(object, encoded_key)
            .unwrap_or_else(value::encode_undefined));
    }
    if value::is_bool(object)
        || value::is_f64(object)
        || value::is_string(object)
        || value::is_bigint(object)
        || value::is_symbol(object)
    {
        return Ok(state
            .primitive_property(object, key)
            .unwrap_or_else(value::encode_undefined));
    }
    let encoded_key = key;
    let key = property_key(state, key).ok_or(())?;
    if value::is_callable(object) {
        // OrdinaryGet：沿 callable 原型链逐层查自有访问器/数据属性（含子类
        // 构造器继承基类静态属性）；非 callable 原型递归对象路径，显式 null
        // 终止，链尾隐式 Function.prototype 由 primitive_property 合成内建。
        return match callable_chain::resolve(state, object, key) {
            CallableChainHit::Accessor { getter, .. } => {
                if value::is_callable(getter) {
                    state.invoke_callable(ctx, getter, receiver, &[]).ok_or(())
                } else {
                    Ok(value::encode_undefined())
                }
            }
            CallableChainHit::Data { stored, .. } => Ok(stored),
            CallableChainHit::Object { prototype } => {
                get_property_with_receiver(ctx, state, prototype, encoded_key, receiver)
            }
            CallableChainHit::Null => Ok(value::encode_undefined()),
            CallableChainHit::Implicit { tail } => {
                if let Some(property) = state.primitive_property(tail, encoded_key) {
                    return Ok(property);
                }
                // 隐式 %Function.prototype% 的自有 constructor（§20.2.3.1）。
                if state.text_matches(encoded_key, "constructor") {
                    return Ok(state
                        .native_callable(crate::NativeCallableKind::FunctionConstructor)
                        .unwrap_or_else(value::encode_undefined));
                }
                // %Function.prototype% 的 [[Prototype]] 是 %Object.prototype%
                // （§20.2.3）：链尾继续上行，使继承成员对 callable 可见。
                let Some(prototype) = state.object_prototype else {
                    return Ok(value::encode_undefined());
                };
                get_property_with_receiver(ctx, state, prototype, encoded_key, receiver)
            }
        };
    }
    let handle = object_handle(object).ok_or(())?;
    let lookup = state
        .gc
        .heap()
        .get_property_slot_on_proto_chain(handle, key);
    match lookup {
        Ok(Some((holder, property))) => {
            // holder 为链尾 %Object.prototype% 时先让宿主家族合成认领：
            // Date / Error / Buffer / TypedArray 等的"原型层"在实例与
            // %Object.prototype% 之间（规范中是真实的中间原型对象），其
            // toString / valueOf / toLocaleString 必须遮蔽 %Object.prototype%
            // 的自有属性；更近层的用户属性 holder 不同，不受影响。
            if state.object_prototype.map(value::decode_handle) == Some(holder)
                && let Some(synthesized) = state.primitive_property(object, encoded_key)
            {
                return Ok(synthesized);
            }
            if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                let getter = property.getter as i64;
                return if value::is_callable(getter) {
                    state.invoke_callable(ctx, getter, receiver, &[]).ok_or(())
                } else {
                    Ok(value::encode_undefined())
                };
            }
            Ok(property.value as i64)
        }
        Ok(None) => {
            if let Some(property) = state.primitive_property(object, encoded_key) {
                return Ok(property);
            }
            // 包装对象（boxed primitive）：普通对象链未命中后回退到原语本身
            // （字符串 length/索引等 exotic own 属性与原语方法）。
            if let Some(primitive) = boxed_primitive_value(state, object) {
                return get_property_with_receiver(ctx, state, primitive, encoded_key, receiver);
            }
            Ok(value::encode_undefined())
        }
        Err(wjsm_gc::HeapAccessV2Error::ProxyPrototype { handle }) => Ok(super::proxy::get(
            ctx,
            state,
            value::encode_proxy_handle(handle & 0x7fff_ffff),
            encoded_key,
            receiver,
        )),
        Err(_) => Err(()),
    }
}

/// OrdinarySetWithOwnDescriptor 步骤 2.b-2.e / 3.d.iv 的 receiver 侧终局：
/// 链上命中可写数据属性（或未命中按缺省数据属性）后，在 receiver 上更新或
/// 创建自有数据属性；receiver 自有访问器或不可写数据属性拒绝写入。
pub(super) fn assign_data_property_to_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: PropertyKey,
    stored: i64,
) -> property_write::SetResult {
    use property_write::{SetCompletion, SetFailure};
    if value::is_proxy(receiver) {
        return super::proxy::set_receiver_value(
            ctx,
            state,
            receiver,
            encoded_property_key(key),
            stored,
        );
    }
    if value::is_callable(receiver) {
        return Ok(assign_to_callable_receiver(state, receiver, key, stored));
    }
    // OrdinarySetWithOwnDescriptor 步骤 3.d.iv：Receiver 为基元（如
    // Reflect.set 显式传入基元 receiver）时数据属性写入返回 false。
    if is_primitive_value(receiver) {
        return Ok(SetCompletion::Failed(SetFailure::Receiver));
    }
    let receiver_handle = object_handle(receiver).ok_or_else(|| fail_dispatch(ctx))?;
    if let Some(receiver_descriptor) = state
        .gc
        .heap()
        .get_property_slot(receiver_handle, key)
        .map_err(|_| fail_dispatch(ctx))?
    {
        if receiver_descriptor.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
            return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
        }
        if receiver_descriptor.flags & constants::FLAG_WRITABLE as u32 == 0 {
            return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
        }
    } else if state.non_extensible_objects.contains(&receiver_handle) {
        return Ok(SetCompletion::Failed(SetFailure::NotExtensible));
    }
    set_property_or_out_of_memory(ctx, state, receiver_handle, key, stored as u64)?;
    Ok(SetCompletion::Written)
}

/// callable receiver 上的自有数据属性写入：自有访问器或不可写自有数据属性
/// 拒绝；更新既有属性保留原特性，新建属性取缺省可写/可枚举/可配置。写前
/// 先触发 name / length / prototype 的惰性物化，使其不可写特性对赋值可见。
fn assign_to_callable_receiver(
    state: &mut NativeAgentState,
    receiver: i64,
    key: PropertyKey,
    stored: i64,
) -> property_write::SetCompletion {
    use property_write::{SetCompletion, SetFailure};
    let receiver = value::strip_gc_color(receiver);
    if state.callable_accessors.contains_key(&(receiver, key)) {
        return SetCompletion::Failed(SetFailure::GetterOnly);
    }
    let _ = state.callable_property(receiver, key);
    if state
        .callable_property_flags
        .get(&(receiver, key))
        .is_some_and(|flags| flags & constants::FLAG_WRITABLE as u32 == 0)
    {
        return SetCompletion::Failed(SetFailure::ReadOnly);
    }
    if !state.callable_properties.contains_key(&(receiver, key))
        && state.non_extensible_callables.contains(&receiver)
    {
        return SetCompletion::Failed(SetFailure::NotExtensible);
    }
    state.callable_properties.insert((receiver, key), stored);
    state
        .callable_property_flags
        .entry((receiver, key))
        .or_insert(ASSIGNED_PROPERTY_FLAGS);
    SetCompletion::Written
}

pub(super) fn delete_property(
    state: &mut NativeAgentState,
    object: i64,
    encoded_key: i64,
) -> Result<bool, ()> {
    let key = property_key(state, encoded_key).ok_or(())?;
    let configurable = constants::FLAG_CONFIGURABLE as u32;
    if value::is_callable(object) {
        // callable 侧表键统一为去色规范形，与写入/查找路径一致。
        let object = value::strip_gc_color(object);
        if state
            .callable_property_flags
            .get(&(object, key))
            .is_some_and(|flags| flags & configurable == 0)
        {
            return Ok(false);
        }
        state.callable_properties.remove(&(object, key));
        state.callable_accessors.remove(&(object, key));
        state.callable_property_flags.remove(&(object, key));
        return Ok(true);
    }
    if value::is_array(object) {
        if state.text_matches(encoded_key, "length") {
            return Ok(false);
        }
        let handle = value::decode_handle(object);
        if let Some(index) = array_index(state, encoded_key) {
            // 字典数组的下标条目携带特性：不可配置（seal/freeze 或
            // defineProperty 设置）按 [[Delete]] 拒绝，删除成功则同步清除
            // 覆盖层条目，防止残留旧值。
            if state.gc.heap().array_kind(handle).ok() == Some(constants::ARRAY_KIND_DICTIONARY) {
                let element_key =
                    property_key(state, value::encode_f64(f64::from(index))).ok_or(())?;
                if state
                    .array_property_flags
                    .get(&(handle, element_key))
                    .is_some_and(|flags| flags & configurable == 0)
                    || state
                        .array_accessors
                        .get(&(handle, element_key))
                        .is_some_and(|(_, _, flags)| flags & configurable == 0)
                {
                    return Ok(false);
                }
                state.array_properties.remove(&(handle, element_key));
                state.array_property_flags.remove(&(handle, element_key));
                state.array_accessors.remove(&(handle, element_key));
                state.forget_array_property(handle, element_key);
            }
            let length = state.gc.heap().array_length(handle).map_err(|_| ())?;
            if index < length {
                state
                    .gc
                    .heap()
                    .set_element(handle, index, value::encode_array_hole() as u64)
                    .map_err(|_| ())?;
            }
            return Ok(true);
        }
        if state
            .array_property_flags
            .get(&(handle, key))
            .is_some_and(|flags| flags & configurable == 0)
            || state
                .array_accessors
                .get(&(handle, key))
                .is_some_and(|(_, _, flags)| flags & configurable == 0)
        {
            return Ok(false);
        }
        state.array_properties.remove(&(handle, key));
        state.array_accessors.remove(&(handle, key));
        state.array_property_flags.remove(&(handle, key));
        state.forget_array_property(handle, key);
        return Ok(true);
    }
    let handle = object_handle(object).ok_or(())?;
    if state
        .gc
        .heap()
        .get_property_slot(handle, key)
        .map_err(|_| ())?
        .is_some_and(|property| property.flags & configurable == 0)
    {
        return Ok(false);
    }
    // mapped arguments（ES §10.4.4.4）：删除前先取属性值（映射期间即形参绑定
    // 真值），删除成功后解除映射并把该值快照进绑定槽。
    let mapped_slot = super::arguments::live_mapped_index(state, handle, key).map(|index| {
        let previous = state
            .gc
            .heap()
            .get_property_slot(handle, key)
            .ok()
            .flatten()
            .map_or_else(value::encode_undefined, |property| property.value as i64);
        (index, previous)
    });
    let removed = state
        .gc
        .heap()
        .delete_property(handle, key)
        .map_err(|_| ())?;
    if removed && let Some((index, previous)) = mapped_slot {
        super::arguments::after_delete_property(state, handle, index, previous);
    }
    Ok(removed)
}
pub(super) fn has_property(state: &mut NativeAgentState, object: i64, encoded_key: i64) -> bool {
    if value::is_array(object) && state.text_matches(encoded_key, "length") {
        return true;
    }
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        if let Some(index) = array_index(state, encoded_key) {
            match state.gc.heap().get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        let Some(key) = property_key(state, encoded_key) else {
            return false;
        };
        if state.array_properties.contains_key(&(handle, key))
            || state.array_accessors.contains_key(&(handle, key))
            || state.primitive_property(object, encoded_key).is_some()
        {
            return true;
        }
        // 数组合成方法未命中：沿堆原型链上行（%Array.prototype% →
        // %Object.prototype%），使 hasOwnProperty 等继承成员对 in 可见。
        return heap_prototype_value(state, object)
            .ok()
            .flatten()
            .is_some_and(|prototype| has_property(state, prototype, encoded_key));
    }
    let Some(key) = property_key(state, encoded_key) else {
        return false;
    };
    if value::is_callable(object) {
        // HasProperty 与 [[Get]] 同链：逐层自有属性 → 非 callable 原型递归
        // 对象路径 → 显式 null 缺失 → 链尾隐式 Function.prototype 内建，
        // 再沿 %Object.prototype% 上行（§20.2.3）。
        return match callable_chain::resolve(state, object, key) {
            CallableChainHit::Accessor { .. } | CallableChainHit::Data { .. } => true,
            CallableChainHit::Object { prototype } => has_property(state, prototype, encoded_key),
            CallableChainHit::Null => false,
            CallableChainHit::Implicit { tail } => {
                state.primitive_property(tail, encoded_key).is_some()
                    || state.text_matches(encoded_key, "constructor")
                    || state
                        .object_prototype
                        .is_some_and(|prototype| has_property(state, prototype, encoded_key))
            }
        };
    }
    let Some(handle) = object_handle(object) else {
        return false;
    };
    state
        .gc
        .heap()
        .get_property_slot_on_proto_chain(handle, key)
        .ok()
        .flatten()
        .is_some()
        || state.primitive_property(object, encoded_key).is_some()
        || boxed_primitive_value(state, object)
            .is_some_and(|primitive| boxed_primitive_has(state, primitive, encoded_key))
}

/// 包装对象（boxed primitive）承载的原语值；非包装对象为 None。
fn boxed_primitive_value(state: &NativeAgentState, object: i64) -> Option<i64> {
    if !value::is_js_object(object) {
        return None;
    }
    state
        .boxed_primitives
        .get(&value::decode_handle(object))
        .copied()
}

/// 包装对象在普通对象链未命中后按原语回退的 HasProperty：
/// 字符串的 length / 有效索引为 exotic own 属性，其余委托原语方法解析。
fn boxed_primitive_has(state: &mut NativeAgentState, primitive: i64, encoded_key: i64) -> bool {
    if value::is_string(primitive) {
        if state.text_matches(encoded_key, "length") {
            return true;
        }
        if let Some(index) = array_index(state, encoded_key)
            && state
                .string_len(primitive)
                .is_some_and(|length| (index as usize) < length)
        {
            return true;
        }
    }
    state.primitive_property(primitive, encoded_key).is_some()
}

fn primitive_string(state: &NativeAgentState, source: i64) -> Option<i64> {
    if value::is_string(source) {
        return Some(source);
    }
    if value::is_js_object(source) {
        return state
            .boxed_primitives
            .get(&value::decode_handle(source))
            .copied()
            .filter(|value| value::is_string(*value));
    }
    None
}

fn intrinsic_iterator_source(
    state: &NativeAgentState,
    source: i64,
    method: i64,
) -> Option<(
    super::super::NativeIteratorSource,
    super::super::NativeIteratorKind,
)> {
    let (builtin, _) = state.native_callable_builtin(method)?;
    let handle = value::decode_handle(source);
    match builtin {
        wjsm_ir::Builtin::IteratorFrom if value::is_array(source) => Some((
            super::super::NativeIteratorSource::Array(handle),
            super::super::NativeIteratorKind::Values,
        )),
        wjsm_ir::Builtin::IteratorFrom | wjsm_ir::Builtin::StringIterator
            if let Some(text) = primitive_string(state, source) =>
        {
            Some((
                super::super::NativeIteratorSource::String(text),
                super::super::NativeIteratorKind::Values,
            ))
        }
        wjsm_ir::Builtin::IteratorFrom
            if value::is_js_object(source)
                && state
                    .gc
                    .heap()
                    .object_type(handle)
                    .is_ok_and(|kind| kind == u32::from(wjsm_ir::HEAP_TYPE_ARGUMENTS)) =>
        {
            Some((
                super::super::NativeIteratorSource::ArrayLike(handle),
                super::super::NativeIteratorKind::Values,
            ))
        }
        wjsm_ir::Builtin::TypedArrayProtoValues if state.typed_arrays.contains_key(&handle) => {
            Some((
                super::super::NativeIteratorSource::TypedArray(handle),
                super::super::NativeIteratorKind::Values,
            ))
        }
        wjsm_ir::Builtin::MapSetEntries if state.maps.contains_key(&handle) => Some((
            super::super::NativeIteratorSource::Map(handle),
            super::super::NativeIteratorKind::Entries,
        )),
        wjsm_ir::Builtin::MapSetValues if state.sets.contains_key(&handle) => Some((
            super::super::NativeIteratorSource::Set(handle),
            super::super::NativeIteratorKind::Values,
        )),
        _ => None,
    }
}

pub(super) fn iterator_from(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let symbol = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ITERATOR);
    let Ok(method) = get_property(ctx, state, source, symbol) else {
        return type_error(ctx, state, "value is not iterable");
    };
    if value::is_exception(method) {
        return method;
    }
    iterator_from_method(ctx, state, source, method)
}

pub(super) fn iterator_from_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
    method: i64,
) -> i64 {
    if !value::is_callable(method) {
        return type_error(ctx, state, "value is not iterable");
    }
    if let Some((source_kind, iterator_kind)) = intrinsic_iterator_source(state, source, method) {
        let Ok(iterator) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
            return fail_dispatch(ctx);
        };
        state.array_iterators.insert(
            value::decode_handle(iterator),
            super::super::NativeArrayIterator {
                source: source_kind,
                kind: iterator_kind,
                index: 0,
                current: None,
                done: false,
            },
        );
        return iterator;
    }
    let Some(iterator) = state.invoke_callable(ctx, method, source, &[]) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(iterator) {
        return iterator;
    }
    if !value::is_js_object(iterator) {
        return type_error(ctx, state, "iterator method did not return an object");
    }
    let handle = value::decode_handle(iterator);
    state
        .array_iterators
        .entry(handle)
        .or_insert(super::super::NativeArrayIterator {
            source: super::super::NativeIteratorSource::Custom(iterator),
            kind: super::super::NativeIteratorKind::Values,
            index: 0,
            current: None,
            done: false,
        });
    iterator
}

pub(crate) fn array_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
    kind: super::super::NativeIteratorKind,
) -> i64 {
    if !value::is_array(source) {
        return type_error(ctx, state, "Array iterator receiver is not an object");
    }
    let Ok(iterator) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    state.array_iterators.insert(
        value::decode_handle(iterator),
        super::super::NativeArrayIterator {
            source: super::super::NativeIteratorSource::Array(value::decode_handle(source)),
            kind,
            index: 0,
            current: None,
            done: false,
        },
    );
    iterator
}

pub(super) fn iterator_done(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    if args
        .first()
        .is_some_and(|iterator| value::is_exception(*iterator))
    {
        return args[0];
    }
    let Some(handle) = args.first().map(|iterator| value::decode_handle(*iterator)) else {
        return fail_dispatch(ctx);
    };
    if let Err(exception) = ensure_custom_current(ctx, state, handle) {
        return exception;
    }
    let Some(iterator) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    let done = match iterator.source {
        super::super::NativeIteratorSource::Array(source) => state
            .gc
            .heap()
            .array_length(source)
            .is_ok_and(|length| iterator.index >= length),
        super::super::NativeIteratorSource::ArrayLike(source) => {
            iterator.index >= array_like_length(state, source).unwrap_or(0)
        }
        super::super::NativeIteratorSource::String(source) => state
            .string_owned(source)
            .is_none_or(|text| iterator.index as usize >= text.utf16_len()),
        super::super::NativeIteratorSource::TypedArray(source) => state
            .typed_arrays
            .get(&source)
            .is_none_or(|array| iterator.index as usize >= array.length),
        super::super::NativeIteratorSource::Map(source) => state
            .maps
            .get(&source)
            .is_none_or(|entries| iterator.index as usize >= entries.len()),
        super::super::NativeIteratorSource::Set(source) => state
            .sets
            .get(&source)
            .is_none_or(|values| iterator.index as usize >= values.len()),
        super::super::NativeIteratorSource::Custom(_) => iterator.done,
    };
    value::encode_bool(done)
}

pub(super) fn iterator_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    advance: bool,
) -> i64 {
    if args
        .first()
        .is_some_and(|iterator| value::is_exception(*iterator))
    {
        return args[0];
    }
    let Some(handle) = args.first().map(|iterator| value::decode_handle(*iterator)) else {
        return fail_dispatch(ctx);
    };
    if let Err(exception) = ensure_custom_current(ctx, state, handle) {
        return exception;
    }
    let Some(iterator) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    if iterator.done
        && !matches!(
            iterator.source,
            super::super::NativeIteratorSource::Custom(_)
        )
    {
        return value::encode_undefined();
    }
    let (result, step) = match iterator.source {
        super::super::NativeIteratorSource::Array(source) => {
            let result = match state.gc.heap().get_element(source, iterator.index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => element as i64,
                Ok(_) => value::encode_undefined(),
                Err(_) => return fail_dispatch(ctx),
            };
            (result, 1)
        }
        super::super::NativeIteratorSource::ArrayLike(source) => {
            let Some(key) = state.intern_text(iterator.index.to_string(), value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            let object = value::encode_object_handle(source);
            let Ok(result) = get_property(ctx, state, object, key) else {
                return fail_dispatch(ctx);
            };
            (result, 1)
        }
        super::super::NativeIteratorSource::String(source) => {
            let index = iterator.index as usize;
            let Some(width) = state
                .with_string_units(source, |units| {
                    wjsm_host::code_point_at(units, index)
                        .map(|code_point| usize::from(code_point > 0xffff) + 1)
                })
                .flatten()
            else {
                return value::encode_undefined();
            };
            let Some(units) =
                state.with_string_units(source, |units| units[index..index + width].to_vec())
            else {
                return fail_dispatch(ctx);
            };
            let result = intern_string_with_gc_retry(
                ctx,
                state,
                wjsm_host::RuntimeString::from_utf16_units(units),
            );
            (result, width as u32)
        }
        super::super::NativeIteratorSource::TypedArray(source) => {
            let index = usize::try_from(iterator.index).unwrap_or(usize::MAX);
            (
                super::typedarray::get_element_intern(
                    state,
                    value::encode_object_handle(source),
                    index,
                )
                .unwrap_or_else(value::encode_undefined),
                1,
            )
        }
        super::super::NativeIteratorSource::Map(source) => {
            let Some((key, stored)) = state
                .maps
                .get(&source)
                .and_then(|entries| entries.get(iterator.index as usize))
                .copied()
            else {
                return value::encode_undefined();
            };
            let Ok(entry) = state.allocate_array_values_with_gc_retry(ctx, &[key, stored]) else {
                return fail_dispatch(ctx);
            };
            (entry, 1)
        }
        super::super::NativeIteratorSource::Set(source) => {
            let Some(stored) = state
                .sets
                .get(&source)
                .and_then(|values| values.get(iterator.index as usize))
                .copied()
            else {
                return value::encode_undefined();
            };
            (stored, 1)
        }
        super::super::NativeIteratorSource::Custom(_) => {
            let Some(result) = iterator.current else {
                return fail_dispatch(ctx);
            };
            let Some(key) = state.intern_text("value".into(), value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            let Ok(value) = get_property(ctx, state, result, key) else {
                return fail_dispatch(ctx);
            };
            (value, 1)
        }
    };
    let result = match iterator.kind {
        super::super::NativeIteratorKind::Values => result,
        super::super::NativeIteratorKind::Keys => value::encode_f64(f64::from(iterator.index)),
        super::super::NativeIteratorKind::Entries => {
            let Ok(entry) = state.allocate_array_values_with_gc_retry(
                ctx,
                &[value::encode_f64(f64::from(iterator.index)), result],
            ) else {
                return fail_dispatch(ctx);
            };
            entry
        }
    };
    if advance {
        let iterator = state
            .array_iterators
            .get_mut(&handle)
            .expect("iterator entry was resolved above");
        iterator.index = iterator.index.saturating_add(step);
        if matches!(
            iterator.source,
            super::super::NativeIteratorSource::Custom(_)
        ) {
            iterator.current = None;
        }
    }
    result
}

pub(super) fn iterator_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let result = iterator_value(ctx, state, args, true);
    if value::is_exception(result) {
        result
    } else {
        value::encode_undefined()
    }
}

pub(crate) fn create_iterator_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: i64,
    done: bool,
) -> i64 {
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    for (name, stored) in [("value", result), ("done", value::encode_bool(done))] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(value::decode_handle(object), key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    object
}

pub(crate) fn iterator_next_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: u32,
) -> i64 {
    let encoded = value::encode_object_handle(iterator);
    let done = iterator_done(ctx, state, &[encoded]);
    if value::is_exception(done) {
        return done;
    }
    let is_done = is_truthy(state, done);
    let result = if is_done {
        value::encode_undefined()
    } else {
        iterator_value(ctx, state, &[encoded], true)
    };
    if value::is_exception(result) {
        return result;
    }
    create_iterator_result(ctx, state, result, is_done)
}

fn ensure_custom_current(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator_handle: u32,
) -> Result<(), i64> {
    let Some(iterator) = state.array_iterators.get(&iterator_handle).copied() else {
        return Err(fail_dispatch(ctx));
    };
    let super::super::NativeIteratorSource::Custom(object) = iterator.source else {
        return Ok(());
    };
    if iterator.done || iterator.current.is_some() {
        return Ok(());
    }
    let Some(next_key) = state.intern_text("next".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let next = get_property(ctx, state, object, next_key).map_err(|()| fail_dispatch(ctx))?;
    if !value::is_callable(next) {
        return Err(type_error(ctx, state, "iterator.next is not callable"));
    }
    let result = state
        .invoke_callable(ctx, next, object, &[])
        .ok_or_else(|| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(result);
    }
    if !value::is_js_object(result) {
        return Err(type_error(ctx, state, "iterator result is not an object"));
    }
    let Some(done_key) = state.intern_text("done".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let done = get_property(ctx, state, result, done_key).map_err(|()| fail_dispatch(ctx))?;
    let done = is_truthy(state, done);
    let iterator = state
        .array_iterators
        .get_mut(&iterator_handle)
        .expect("iterator entry was resolved above");
    iterator.done = done;
    iterator.current = Some(result);
    Ok(())
}

fn array_like_length(state: &mut NativeAgentState, source: u32) -> Option<u32> {
    let key = state.intern_property_string("length".into())?;
    let stored = state.gc.heap().get_property(source, key).ok().flatten()? as i64;
    to_number(state, stored).and_then(|length| {
        (length.is_finite() && length >= 0.0 && length <= u32::MAX as f64)
            .then_some(length.floor() as u32)
    })
}

pub(super) fn iterator_close(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [iterator, completion] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(*iterator) {
        return *iterator;
    }
    let handle = value::decode_handle(*iterator);
    let Some(entry) = state.array_iterators.remove(&handle) else {
        return *completion;
    };
    let super::super::NativeIteratorSource::Custom(object) = entry.source else {
        return *completion;
    };
    let Some(return_key) = state.intern_text("return".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(method) = get_property(ctx, state, object, return_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_undefined(method) || value::is_null(method) {
        return *completion;
    }
    if !value::is_callable(method) {
        return type_error(ctx, state, "iterator.return is not callable");
    }
    let Some(result) = state.invoke_callable(ctx, method, object, &[]) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(result) {
        return result;
    }
    if !value::is_js_object(result) {
        return type_error(ctx, state, "iterator.return did not return an object");
    }
    *completion
}

fn named_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: &str,
) -> i64 {
    super::modules::named_error_object(state, name, message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) fn type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    named_error(ctx, state, "TypeError", message)
}

pub(super) fn syntax_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    named_error(ctx, state, "SyntaxError", message)
}

pub(super) fn reference_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    named_error(ctx, state, "ReferenceError", message)
}

pub(super) fn range_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    named_error(ctx, state, "RangeError", message)
}

/// licm elem-guard 的 pre-header 一次性校验（只读、不分配、不执行用户代码）：
///
/// `array` 当前必须是 PACKED 普通数组，全部元素为 shape 等于模板烘焙 shape 的
/// 普通对象，且每个元素的所有值槽均非对象（含 regexp）。三个条件合起来保证：
/// 循环体内 `GetPropGuarded` 可以跳过逐迭代 shape 检查直读模板槽偏移，且读出
/// 的值参与协变运算（ToPrimitive）时不可能回调用户代码。任一条件不满足时返回
/// false，同循环的 Guarded 指令全部退回通用路径，语义不变。
fn elem_shape_guard_holds(state: &NativeAgentState, array: i64, template_index: u32) -> bool {
    if !value::is_array(array) {
        return false;
    }
    let Some(entry_start) = usize::try_from(template_index)
        .ok()
        .and_then(|index: usize| index.checked_mul(constants::OBJECT_TEMPLATE_META_WORDS as usize))
    else {
        return false;
    };
    let entry_end = entry_start + constants::OBJECT_TEMPLATE_META_WORDS as usize;
    let Some(meta) = state.object_template_meta.get(entry_start..entry_end) else {
        return false;
    };
    let baked_shape = meta[0];
    let slot_count = meta[1];
    let handle = value::decode_array_handle(array);
    let heap = state.gc.heap();
    if !matches!(heap.array_kind(handle), Ok(constants::ARRAY_KIND_PACKED)) {
        return false;
    }
    let Ok(length) = heap.array_length(handle) else {
        return false;
    };
    (0..length).all(|index| elem_conforms(heap, handle, index, baked_shape, slot_count))
}

/// 单个数组元素的守卫条件：非洞、TAG_OBJECT、shape 命中烘焙模板、值槽全部非对象。
fn elem_conforms(
    heap: &wjsm_gc::HeapAccessV2<wjsm_gc::NativeHeapMemory>,
    array_handle: u32,
    index: u32,
    baked_shape: u32,
    slot_count: u32,
) -> bool {
    let Ok(Some(element)) = heap.get_element(array_handle, index) else {
        return false;
    };
    let element = element as i64;
    if !value::is_object(element) {
        return false;
    }
    let element_handle = value::decode_object_handle(element);
    if !matches!(heap.shape_id(element_handle), Ok(shape) if shape == baked_shape) {
        return false;
    }
    (0..slot_count).all(|slot| match heap.value_slot(element_handle, slot) {
        Ok(stored) => {
            let stored = stored as i64;
            !value::is_js_object(stored) && !value::is_regexp(stored)
        }
        Err(_) => false,
    })
}

fn init_object_literal_or_fail(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    template_index: u32,
    values: &[i64],
) -> Option<i64> {
    let entry_start =
        usize::try_from(template_index.checked_mul(constants::OBJECT_TEMPLATE_META_WORDS)?).ok()?;
    let entry_end = entry_start.checked_add(constants::OBJECT_TEMPLATE_META_WORDS as usize)?;
    let meta = state.object_template_meta.get(entry_start..entry_end)?;
    let prop_count = meta[3] as usize;
    if values.len() != prop_count {
        return None;
    }
    let capacity = meta[2];
    let shape_id = meta[0];
    let properties: Vec<(u32, u64)> = (0..prop_count)
        .map(|index| (meta[4 + index], values[index] as u64))
        .collect();
    let object = allocate_object_or_out_of_memory(ctx, state, capacity, false);
    if !value::is_object(object) {
        return Some(object);
    }
    let handle = value::decode_object_handle(object);
    match state
        .gc
        .heap()
        .write_baked_object_literal_properties(handle, shape_id, &properties)
    {
        Ok(()) => Some(object),
        Err(HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state
                .gc
                .flush_native_tlab(ctx)
                .map_err(|_| fail_dispatch(ctx))
                .ok()?;
            match state.gc.heap().write_baked_object_literal_properties(
                handle,
                shape_id,
                &properties,
            ) {
                Ok(()) => Some(object),
                Err(HeapAccessV2Error::HeapExhausted { .. }) => {
                    state.collect_garbage(ctx).ok()?;
                    state
                        .gc
                        .heap()
                        .write_baked_object_literal_properties(handle, shape_id, &properties)
                        .ok()?;
                    Some(object)
                }
                Err(_) => None,
            }
        }
        Err(HeapAccessV2Error::HeapExhausted { .. }) => {
            state.collect_garbage(ctx).ok()?;
            state
                .gc
                .heap()
                .write_baked_object_literal_properties(handle, shape_id, &properties)
                .ok()?;
            Some(object)
        }
        Err(_) => None,
    }
}

/// 属性槽扩容会 reserve 新对象；堆页耗尽时先 STW 回收再重试。
fn set_property_or_out_of_memory(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    key: PropertyKey,
    stored: u64,
) -> Result<(), i64> {
    match state.gc.heap().set_property(handle, key, stored) {
        Ok(()) => Ok(()),
        Err(HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state
                .gc
                .flush_native_tlab(ctx)
                .map_err(|_| fail_dispatch(ctx))?;
            match state.gc.heap().set_property(handle, key, stored) {
                Ok(()) => Ok(()),
                Err(HeapAccessV2Error::HeapExhausted { .. }) => {
                    state.collect_garbage(ctx).map_err(|_| fail_dispatch(ctx))?;
                    state
                        .gc
                        .heap()
                        .set_property(handle, key, stored)
                        .map_err(|_| fail_dispatch(ctx))
                }
                Err(_) => Err(fail_dispatch(ctx)),
            }
        }
        Err(HeapAccessV2Error::HeapExhausted { .. }) => {
            state.collect_garbage(ctx).map_err(|_| fail_dispatch(ctx))?;
            match state.gc.heap().set_property(handle, key, stored) {
                Ok(()) => Ok(()),
                Err(HeapAccessV2Error::HeapExhausted { .. }) => Err(state
                    .out_of_memory_exception()
                    .unwrap_or_else(|| fail_dispatch(ctx))),
                Err(_) => Err(fail_dispatch(ctx)),
            }
        }
        Err(_) => Err(fail_dispatch(ctx)),
    }
}

pub(super) fn allocate_object_or_out_of_memory(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    capacity: u32,
    array: bool,
) -> i64 {
    match state.allocate_object_with_gc_retry(ctx, capacity, array) {
        Ok(value) => value,
        Err(NativeRuntimeError::Heap(HeapAccessV2Error::HeapExhausted { .. }))
        | Err(NativeRuntimeError::Gc(NativeGcError::Heap(HeapAccessV2Error::HeapExhausted {
            ..
        }))) => state
            .out_of_memory_exception()
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Err(_) => fail_dispatch(ctx),
    }
}

pub(super) fn array_index(state: &NativeAgentState, encoded: i64) -> Option<u32> {
    let text = if value::is_string(encoded) || value::is_bigint(encoded) {
        state.string_owned(encoded)?.to_utf8()?
    } else if value::is_f64(encoded) {
        render_value(state, encoded)
    } else {
        return None;
    };
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let index = text.parse::<u32>().ok()?;
    (index != u32::MAX).then_some(index)
}

pub(super) fn array_length(state: &NativeAgentState, encoded: i64) -> Option<u32> {
    let number = to_number(state, encoded)?;
    (number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number <= u32::MAX as f64)
        .then_some(number as u32)
}

fn numeric_or_bigint(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    bigint_builtin: wjsm_ir::Builtin,
    number_operation: impl FnOnce(f64, f64) -> f64,
) -> i64 {
    let [left, right] = args else {
        return fail_dispatch(ctx);
    };
    let left = match to_primitive(ctx, state, *left, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let right = match to_primitive(ctx, state, *right, PrimitiveHint::Number) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    if value::is_bigint(left) || value::is_bigint(right) {
        super::bigint::dispatch_bigint(ctx, state, bigint_builtin, &[left, right])
            .expect("BigInt builtin is handled")
    } else {
        binary_number(ctx, state, &[left, right], number_operation)
    }
}

fn binary_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: impl FnOnce(f64, f64) -> f64,
) -> i64 {
    let [left, right] = args else {
        return fail_dispatch(ctx);
    };
    let left = match to_number_coerced(ctx, state, *left) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    let right = match to_number_coerced(ctx, state, *right) {
        Ok(number) => number,
        Err(exception) => return exception,
    };
    value::encode_f64(operation(left, right))
}

pub(super) fn to_uint32(number: f64) -> u32 {
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    number.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub(super) fn to_int32(number: f64) -> i32 {
    to_uint32(number) as i32
}

fn unary_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    operation: impl FnOnce(f64) -> f64,
) -> i64 {
    let [input] = args else {
        return fail_dispatch(ctx);
    };
    match to_number_coerced(ctx, state, *input) {
        Ok(number) => value::encode_f64(operation(number)),
        Err(exception) => exception,
    }
}

#[derive(Clone, Copy)]
pub(super) enum PrimitiveHint {
    Default,
    Number,
    String,
}

pub(super) fn to_primitive(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
    hint: PrimitiveHint,
) -> Result<i64, i64> {
    if value::is_exception(encoded) {
        return Err(encoded);
    }
    if !value::is_js_object(encoded) && !value::is_regexp(encoded) {
        return Ok(encoded);
    }
    if let Some(primitive) = state
        .boxed_primitives
        .get(&value::decode_handle(encoded))
        .copied()
    {
        return Ok(primitive);
    }

    let to_primitive = value::encode_symbol_handle(wjsm_ir::wk_symbol::TO_PRIMITIVE);
    let exotic = get_property(ctx, state, encoded, to_primitive).map_err(|_| fail_dispatch(ctx))?;
    if value::is_exception(exotic) {
        return Err(exotic);
    }
    if !value::is_undefined(exotic) {
        if !value::is_callable(exotic) {
            return Err(type_error(ctx, state, "@@toPrimitive is not callable"));
        }
        let hint = match hint {
            PrimitiveHint::String => "string",
            PrimitiveHint::Default => "default",
            PrimitiveHint::Number => "number",
        };
        let hint = state
            .intern_text(hint.into(), value::TAG_STRING)
            .ok_or_else(|| fail_dispatch(ctx))?;
        let result = state
            .invoke_callable(ctx, exotic, encoded, &[hint])
            .ok_or_else(|| fail_dispatch(ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        if !value::is_js_object(result) && !value::is_regexp(result) {
            return Ok(result);
        }
        return Err(type_error(
            ctx,
            state,
            "@@toPrimitive must return a primitive value",
        ));
    }

    let names = if matches!(hint, PrimitiveHint::String) {
        ["toString", "valueOf"]
    } else {
        ["valueOf", "toString"]
    };
    for name in names {
        let key = state
            .intern_text(name.into(), value::TAG_STRING)
            .ok_or_else(|| fail_dispatch(ctx))?;
        let method = get_property(ctx, state, encoded, key).map_err(|_| fail_dispatch(ctx))?;
        if value::is_exception(method) {
            return Err(method);
        }
        if !value::is_callable(method) {
            continue;
        }
        let result = state
            .invoke_callable(ctx, method, encoded, &[])
            .ok_or_else(|| fail_dispatch(ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        if !value::is_js_object(result) && !value::is_regexp(result) {
            return Ok(result);
        }
    }
    Err(type_error(
        ctx,
        state,
        "cannot convert object to primitive value",
    ))
}

/// ECMAScript ToPropertyKey（§7.1.19）的值域版本：对象键经 ToPrimitive(string)
/// 再入用户转换（`Symbol.toPrimitive` / `toString` / `valueOf`），Symbol 结果保留
/// 为 symbol 键，其余原始值交由下游 `property_key` / `array_index` 统一处理
/// （数字保持数字以保留索引快路径）；非对象输入零开销原样返回。
pub(super) fn to_property_key_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<i64, i64> {
    if value::is_exception(encoded) {
        return Err(encoded);
    }
    if !value::is_js_object(encoded) && !value::is_regexp(encoded) {
        return Ok(encoded);
    }
    to_primitive(ctx, state, encoded, PrimitiveHint::String)
}

pub(super) fn to_number_coerced(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<f64, i64> {
    let primitive = to_primitive(ctx, state, encoded, PrimitiveHint::Number)?;
    if value::is_bigint(primitive) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert a BigInt value to a number",
        ));
    }
    if value::is_symbol(primitive) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert a Symbol value to a number",
        ));
    }
    to_number(state, primitive).ok_or_else(|| fail_dispatch(ctx))
}

pub(super) fn to_runtime_string_coerced(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<RuntimeString, i64> {
    let primitive = to_primitive(ctx, state, encoded, PrimitiveHint::String)?;
    primitive_to_runtime_string(ctx, state, primitive)
}

fn primitive_to_runtime_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    primitive: i64,
) -> Result<RuntimeString, i64> {
    if value::is_symbol(primitive) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert a Symbol value to a string",
        ));
    }
    if value::is_string(primitive) || value::is_bigint(primitive) {
        return state
            .string_owned(primitive)
            .ok_or_else(|| fail_dispatch(ctx));
    }
    // number 是字符串拼接里最热的来源，直连整数快路径而非绕道 `String`。
    if value::is_f64(primitive) {
        return Ok(RuntimeString::from_number(value::decode_f64(primitive)));
    }
    Ok(RuntimeString::from(render_value(state, primitive)))
}

pub(crate) fn to_string_coerced(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<String, i64> {
    let primitive = to_primitive(ctx, state, encoded, PrimitiveHint::String)?;
    if value::is_symbol(primitive) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert a Symbol value to a string",
        ));
    }
    Ok(render_value(state, primitive))
}

pub(crate) fn to_number(state: &NativeAgentState, encoded: i64) -> Option<f64> {
    if value::is_f64(encoded) {
        Some(value::decode_f64(encoded))
    } else if value::is_bool(encoded) {
        Some(if value::decode_bool(encoded) {
            1.0
        } else {
            0.0
        })
    } else if value::is_null(encoded) {
        Some(0.0)
    } else if value::is_undefined(encoded) {
        Some(f64::NAN)
    } else if value::is_string(encoded) {
        let text = state.string_owned(encoded)?.to_utf8_lossy();
        let text = text.trim();
        if text.is_empty() {
            return Some(0.0);
        }
        if text == "Infinity" || text == "+Infinity" {
            return Some(f64::INFINITY);
        }
        if text == "-Infinity" {
            return Some(f64::NEG_INFINITY);
        }
        let (negative, digits) = if let Some(rest) = text.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix('+') {
            (false, rest)
        } else {
            (false, text)
        };
        let value = if let Some(hex) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            u64::from_str_radix(hex, 16).ok().map(|value| value as f64)
        } else if let Some(bin) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            u64::from_str_radix(bin, 2).ok().map(|value| value as f64)
        } else if let Some(oct) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            u64::from_str_radix(oct, 8).ok().map(|value| value as f64)
        } else {
            digits.parse().ok()
        };
        Some(match value {
            Some(value) if negative => -value,
            Some(value) => value,
            None => f64::NAN,
        })
    } else {
        None
    }
}

pub(super) fn is_truthy(state: &NativeAgentState, encoded: i64) -> bool {
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        number != 0.0 && !number.is_nan()
    } else if value::is_bool(encoded) {
        value::decode_bool(encoded)
    } else if value::is_null(encoded) || value::is_undefined(encoded) {
        false
    } else if value::is_bigint(encoded) {
        super::bigint::read(state, encoded).is_some_and(|number| !number.is_zero())
    } else if value::is_string(encoded) {
        state.string_len(encoded).is_some_and(|length| length != 0)
    } else {
        true
    }
}

pub(super) fn strict_equal(state: &NativeAgentState, left: i64, right: i64) -> bool {
    let left = value::strip_gc_color(left);
    let right = value::strip_gc_color(right);
    if value::is_f64(left) && value::is_f64(right) {
        value::decode_f64(left) == value::decode_f64(right)
    } else if value::is_string(left) && value::is_string(right) {
        left == right
            || state
                .with_string_units(left, |left_units| {
                    state
                        .with_string_units(right, |right_units| left_units == right_units)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
    } else if value::is_bigint(left) && value::is_bigint(right) {
        super::bigint::read(state, left) == super::bigint::read(state, right)
    } else {
        left == right
    }
}

pub(super) fn abstract_equal(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    left: i64,
    right: i64,
) -> Result<bool, i64> {
    if strict_equal(state, left, right) {
        return Ok(true);
    }
    if (value::is_null(left) && value::is_undefined(right))
        || (value::is_undefined(left) && value::is_null(right))
    {
        return Ok(true);
    }
    if value::is_bool(left) {
        return abstract_equal(
            ctx,
            state,
            value::encode_f64(f64::from(u8::from(value::decode_bool(left)))),
            right,
        );
    }
    if value::is_bool(right) {
        return abstract_equal(
            ctx,
            state,
            left,
            value::encode_f64(f64::from(u8::from(value::decode_bool(right)))),
        );
    }
    if (value::is_f64(left) && value::is_string(right))
        || (value::is_string(left) && value::is_f64(right))
    {
        return Ok(to_number(state, left)
            .zip(to_number(state, right))
            .is_some_and(|(left, right)| left == right));
    }
    if value::is_bigint(left) && value::is_string(right) {
        let right = state
            .string_owned(right)
            .and_then(|text| text.to_utf8())
            .and_then(|text| text.trim().parse::<BigInt>().ok());
        return Ok(super::bigint::read(state, left)
            .zip(right)
            .is_some_and(|(left, right)| left == right));
    }
    if value::is_string(left) && value::is_bigint(right) {
        return abstract_equal(ctx, state, right, left);
    }
    if value::is_bigint(left) && value::is_f64(right) {
        let number = value::decode_f64(right);
        return Ok(number.is_finite()
            && number.fract() == 0.0
            && super::bigint::read(state, left)
                .zip(BigInt::from_f64(number))
                .is_some_and(|(left, right)| left == right));
    }
    if value::is_f64(left) && value::is_bigint(right) {
        return abstract_equal(ctx, state, right, left);
    }
    let left_object = value::is_js_object(left) || value::is_regexp(left);
    let right_object = value::is_js_object(right) || value::is_regexp(right);
    if left_object && !right_object {
        let left = to_primitive(ctx, state, left, PrimitiveHint::Default)?;
        return abstract_equal(ctx, state, left, right);
    }
    if !left_object && right_object {
        let right = to_primitive(ctx, state, right, PrimitiveHint::Default)?;
        return abstract_equal(ctx, state, left, right);
    }
    Ok(false)
}

pub(crate) fn array_to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(key) = state.intern_text("join".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(join) = get_property(ctx, state, receiver, key) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(join) {
        return join;
    }
    if value::is_callable(join) {
        return state
            .invoke_callable(ctx, join, receiver, &[])
            .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let tag = if value::is_array(receiver) {
        "[object Array]"
    } else {
        "[object Object]"
    };
    state
        .intern_text(tag.into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn error_to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    if !value::is_js_object(receiver) {
        return type_error(ctx, state, "Error.prototype.toString called on non-object");
    }
    let Some(name_key) = state.intern_text("name".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let name = get_property(ctx, state, receiver, name_key).unwrap_or_else(|()| fail_dispatch(ctx));
    if value::is_exception(name) {
        return name;
    }
    let name = if value::is_undefined(name) {
        "Error".to_owned()
    } else {
        match to_string_coerced(ctx, state, name) {
            Ok(name) => name,
            Err(exception) => return exception,
        }
    };
    let Some(message_key) = state.intern_text("message".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let message =
        get_property(ctx, state, receiver, message_key).unwrap_or_else(|()| fail_dispatch(ctx));
    if value::is_exception(message) {
        return message;
    }
    let message = if value::is_undefined(message) {
        String::new()
    } else {
        match to_string_coerced(ctx, state, message) {
            Ok(message) => message,
            Err(exception) => return exception,
        }
    };
    let text = if name.is_empty() {
        message
    } else if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    };
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn render_value(state: &NativeAgentState, encoded: i64) -> String {
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        if number.is_nan() {
            "NaN".into()
        } else if number.is_infinite() {
            if number > 0.0 {
                "Infinity".into()
            } else {
                "-Infinity".into()
            }
        } else if number.fract() == 0.0 {
            if number == 0.0 {
                "0".into()
            } else {
                number.to_string()
            }
        } else {
            number.to_string()
        }
    } else if value::is_undefined(encoded) {
        "undefined".into()
    } else if value::is_null(encoded) {
        "null".into()
    } else if value::is_bool(encoded) {
        value::decode_bool(encoded).to_string()
    } else if value::is_string(encoded) || value::is_bigint(encoded) {
        state
            .string_owned(encoded)
            .map(|text| text.to_utf8_lossy())
            .unwrap_or_default()
    } else if value::is_regexp(encoded) {
        state.regexp(encoded).map_or_else(String::new, |regexp| {
            let source = if regexp.pattern.is_empty() {
                "(?:)"
            } else {
                &regexp.pattern
            };
            format!("/{source}/{}", regexp.flags)
        })
    } else if value::is_symbol(encoded) {
        state.symbol_description(encoded).map_or_else(
            || "Symbol()".into(),
            |description| format!("Symbol({})", description.to_utf8_lossy()),
        )
    } else if value::is_js_object(encoded)
        && state.error_objects.contains(&value::decode_handle(encoded))
    {
        let name = state
            .property_value_by_name(encoded, "name")
            .and_then(|name| state.string_owned(name))
            .map(|text| text.to_utf8_lossy())
            .unwrap_or_else(|| "Error".into());
        let message = state
            .property_value_by_name(encoded, "message")
            .and_then(|message| state.string_owned(message))
            .map(|text| text.to_utf8_lossy())
            .unwrap_or_default();
        if name.is_empty() {
            message
        } else if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        }
    } else if value::is_array(encoded) {
        let handle = value::decode_handle(encoded);
        let Ok(length) = state.gc.heap().array_length(handle) else {
            return "[array]".into();
        };
        let mut elements = Vec::with_capacity(length as usize);
        for index in 0..length {
            let rendered = match state.gc.heap().get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => {
                    render_value(state, element as i64)
                }
                Ok(_) => "?".into(),
                Err(_) => return "[array]".into(),
            };
            elements.push(rendered);
        }
        format!("[{}]", elements.join(", "))
    } else {
        "[object Object]".into()
    }
}

#[track_caller]
pub(crate) fn fail_dispatch(ctx: &mut NativeVmContext) -> i64 {
    if std::env::var_os("WJSM_TRACE_INVARIANT").is_some() {
        eprintln!(
            "native invariant caller: {}",
            std::panic::Location::caller()
        );
    }
    ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
    value::encode_handle(value::TAG_EXCEPTION, 0)
}

/// 发布字符串失败（典型为 zgc 年代耗尽需 mutator 先推进 GC 再分配）时的统一
/// 重试：全量收集 + 推进搬迁/回收 epoch 后重试一次，仍失败才判 invariant。
/// 所有在循环内产生新字符串的分派路径都必须经此入口，裸
/// `intern_runtime_string(..).unwrap_or_else(fail_dispatch)` 会在 GC 压力下误报。
pub(crate) fn intern_string_with_gc_retry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    string: wjsm_host::RuntimeString,
) -> i64 {
    if state.gc.take_pacing_poll_request() {
        let _ = state.poll_gc(ctx);
    }
    if let Some(encoded) = state.intern_runtime_string(string.clone(), value::TAG_STRING) {
        return encoded;
    }
    if state.collect_garbage(ctx).is_ok() {
        let _ = state.gc.heap().finish_relocation_epoch();
        let _ = state.gc.heap().advance_epoch_and_reclaim();
        if let Some(encoded) = state.intern_runtime_string(string, value::TAG_STRING) {
            return encoded;
        }
    }
    fail_dispatch(ctx)
}
