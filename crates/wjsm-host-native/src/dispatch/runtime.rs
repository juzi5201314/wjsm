use num_bigint::BigInt;
use num_traits::{FromPrimitive, Zero};
use wjsm_gc::HeapAccessV2Error;

use crate::gc::NativeGcError;
use wjsm_host::RuntimeString;
use wjsm_ir::{Constant, constants, value};
use wjsm_native_abi::{
    COOPERATIVE_POLL_BUDGET, NativeFeedbackSlot, NativeRuntimeOp, NativeVmContext,
    PendingExceptionKind,
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
            let [
                function_id,
                block_id,
                instruction,
                env,
                this_value,
                live_count,
            ] = args
            else {
                return fail_dispatch(ctx);
            };
            let Ok(function_id) = u32::try_from(*function_id) else {
                return fail_dispatch(ctx);
            };
            let Ok(block_id) = u32::try_from(*block_id) else {
                return fail_dispatch(ctx);
            };
            let Ok(instruction) = u32::try_from(*instruction) else {
                return fail_dispatch(ctx);
            };
            ctx.resume_function_id = function_id;
            ctx.resume_block_plus_one = block_id.saturating_add(1);
            ctx.resume_instruction_index = instruction;
            ctx.resume_frame_count = 1;
            let _ = live_count;
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
                    Constant::String(_) | Constant::Utf16String(_) => state
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
        NativeRuntimeOp::GuardElementsKind => {
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
            // 链式求值（`a.b.c`）不插表达式级分叉：上一跳的异常以 TAG_EXCEPTION
            // 流入本 op，原样透传（与 Binary / PrivateHas 同一约定），不得再
            // 触发 ToObject TypeError 或键转换的用户代码。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*key) {
                return *key;
            }
            // GetValue 步骤 3.a：ToObject(base) 先于 ToPropertyKey，null/undefined
            // 基座须在键转换（可能执行用户代码）之前抛 TypeError。
            if let Some(exception) = get_on_nullish_base(ctx, state, *object, *key) {
                return exception;
            }
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
            // 内联 IC 快路径只放行 TAG_OBJECT，异常与 nullish 基座必然 miss
            // 到此：异常原样透传，nullish 与 GetProp 同口径抛 ToObject
            // TypeError（键为编译期常量字符串）。
            if value::is_exception(*object) {
                return *object;
            }
            if let Some(exception) = get_on_nullish_base(ctx, state, *object, *key) {
                return exception;
            }
            let result =
                get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx));
            backfill_get_prop_ic(state, *object, *key, *ic_slot_ptr, feedback_slot);
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
        NativeRuntimeOp::SetProp | NativeRuntimeOp::SetPropStrict => {
            let [object, key, stored] = args else {
                return fail_dispatch(ctx);
            };
            // 链式/复合求值的 TAG_EXCEPTION 操作数原样透传：复合赋值的读取
            // 异常（`null.p += 1` 经 Binary 透传流入 stored）必须先于本 op 的
            // ToObject TypeError 传播（读在写前，§13.15.2）。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*key) {
                return *key;
            }
            if value::is_exception(*stored) {
                return *stored;
            }
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
            // 链式/复合求值的 TAG_EXCEPTION 操作数原样透传（键为编译期常量）。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*stored) {
                return *stored;
            }
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
        NativeRuntimeOp::DeleteProp | NativeRuntimeOp::DeletePropStrict => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            // 链式求值的 TAG_EXCEPTION 基座/键原样透传（`delete a.b.c`）。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*key) {
                return *key;
            }
            let strict = operation == NativeRuntimeOp::DeletePropStrict;
            delete_property_operator(ctx, state, *object, *key, strict)
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
                || value::is_regexp(*proto)
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
        NativeRuntimeOp::TypedArrayGetElem => {
            let [object, index] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*index) {
                return *index;
            }
            let Some(index) = array_index(state, *index) else {
                return value::encode_uninitialized();
            };
            match super::typedarray::get_element_intern(state, *object, index as usize) {
                Some(stored) => stored,
                None => value::encode_uninitialized(),
            }
        }
        NativeRuntimeOp::TypedArraySetElem => {
            let [object, index, stored] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*index) {
                return *index;
            }
            if value::is_exception(*stored) {
                return *stored;
            }
            let Some(index) = array_index(state, *index) else {
                return value::encode_uninitialized();
            };
            match super::typedarray::set_element(state, *object, index as usize, *stored) {
                Some(written) => written,
                None => value::encode_uninitialized(),
            }
        }
        NativeRuntimeOp::LoadEnvSlot => {
            let [env, slot, key] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_exception(*env) {
                return *env;
            }
            if value::is_exception(*key) {
                return *key;
            }
            load_env_slot(ctx, state, *env, *slot, *key)
                .unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::StoreEnvSlot | NativeRuntimeOp::StoreEnvSlotStrict => {
            let [env, slot, stored, key] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_exception(*env) {
                return *env;
            }
            if value::is_exception(*stored) {
                return *stored;
            }
            if value::is_exception(*key) {
                return *key;
            }
            let strict = matches!(operation, NativeRuntimeOp::StoreEnvSlotStrict);
            store_env_slot(ctx, state, *env, *slot, *stored, *key, strict)
                .unwrap_or_else(|()| fail_dispatch(ctx))
        }
        NativeRuntimeOp::GetSuperBase => super_base(state).unwrap_or_else(value::encode_undefined),
        NativeRuntimeOp::GetSuperConstructor => {
            super_constructor(state).unwrap_or_else(value::encode_undefined)
        }
        NativeRuntimeOp::GetElem => {
            let [object, index] = args else {
                return fail_dispatch(ctx);
            };
            // 链式求值的 TAG_EXCEPTION 基座/键原样透传（与 GetProp 同约定）。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*index) {
                return *index;
            }
            // GetValue 步骤 3.a：null/undefined 基座先于 ToPropertyKey 抛 TypeError。
            if let Some(exception) = get_on_nullish_base(ctx, state, *object, *index) {
                return exception;
            }
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
                if let Some(feedback) = feedback_slot {
                    record_elem_kind_feedback(state, feedback, *object);
                }
                return stored;
            }
            if value::is_array(*object)
                && let Some(index) = array_index(state, *index)
            {
                let handle = value::decode_handle(*object);
                if let Some(feedback) = feedback_slot {
                    record_elem_kind_feedback(state, feedback, *object);
                }
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
            // 链式/复合求值的 TAG_EXCEPTION 操作数原样透传（读异常先于写检查）。
            if value::is_exception(*object) {
                return *object;
            }
            if value::is_exception(*index) {
                return *index;
            }
            if value::is_exception(*stored) {
                return *stored;
            }
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
                    feedback_slot,
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
                    feedback_slot,
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
                    feedback_slot,
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
pub(super) fn create_data_property_impl(
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
pub(super) fn set_element_completion(
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
            if index as usize >= super::typedarray::view_length(state, &array).unwrap_or(0) {
                // IntegerIndexedElementSet：越界写入静默成功（strict 亦不抛），
                // detach / resize 越界视图同路径。
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

/// GetValue 步骤 3.a（§6.2.5.5）：属性引用的 ToObject(base) 对 null/undefined
/// 抛 TypeError，且先于 ToPropertyKey——键转换可能执行用户代码，其副作用不得
/// 在本 TypeError 之前发生。返回 `None` 表示基座不是 null/undefined。
///
/// 文案对齐 V8 ThrowLoadFromNullOrUndefined 三态：
/// - 键恰为 %Symbol.iterator%：kNotIterableNoSymbolLoad，「<callsite> is not
///   iterable (cannot read property Symbol(Symbol.iterator))」，callsite 按
///   BuildDefaultCallSite 渲染（typeof 前缀：null → "object null"，undefined
///   → "undefined"）；
/// - 键为基元：无副作用渲染进「(reading '<key>')」后缀；
/// - 键为对象：ToPropertyKey 尚未执行，无法无副作用取键名，省略后缀（V8 对
///   带用户 toString 的对象与数组同样省略；纯对象的 "#<Object>" 渲染未实现）。
pub(super) fn get_on_nullish_base(
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
    if value::is_symbol(key) && value::decode_handle(key) == wjsm_ir::wk_symbol::ITERATOR {
        let callsite = if value::is_null(object) {
            "object null"
        } else {
            "undefined"
        };
        return Some(type_error(
            ctx,
            state,
            &format!("{callsite} is not iterable (cannot read property Symbol(Symbol.iterator))"),
        ));
    }
    let message = match render_key_no_side_effects(state, key) {
        Some(rendered) => format!("Cannot read properties of {base} (reading '{rendered}')"),
        None => format!("Cannot read properties of {base}"),
    };
    Some(type_error(ctx, state, &message))
}

/// 无副作用的属性键渲染（V8 Object::NoSideEffectsToMaybeString 的基元子集）：
/// 数字按 JS Number::toString 精确格式化，其余基元复用 `render_value`（字符串
/// 原文 / Symbol(desc) / BigInt 十进制 / 布尔与 null/undefined 字面量）。对象
/// 键需要 ToPropertyKey（可执行用户代码）才能得到键名，返回 `None` 表示不可
/// 无副作用渲染。
fn render_key_no_side_effects(state: &NativeAgentState, key: i64) -> Option<String> {
    if value::is_f64(key) {
        return Some(wjsm_builtins::number_format::format_number_js(
            value::decode_f64(key),
        ));
    }
    (value::is_string(key)
        || value::is_symbol(key)
        || value::is_bigint(key)
        || value::is_bool(key)
        || value::is_null(key)
        || value::is_undefined(key))
    .then(|| render_value(state, key))
}

/// V8 BuildDefaultCallSite：`typeof` 前缀加有限值渲染（string 截断 100 单元
/// 带引号 / null / true / false / number），其余类型（undefined / symbol /
/// bigint / object / function）只保留 typeof 名。GetIterator 家族无源文本时
/// 的回退 callsite 渲染（CallPrinter 的源文本渲染未实现）。
pub(super) fn default_call_site(state: &NativeAgentState, encoded: i64) -> String {
    let type_name = super::operator::type_of_name(state, encoded);
    if value::is_string(encoded) {
        let text = render_value(state, encoded);
        // V8 kMaxPrintedStringLength = 100：超长字符串截断加省略号。
        let rendered: String = if text.chars().count() <= 100 {
            text
        } else {
            let mut truncated: String = text.chars().take(100).collect();
            truncated.push_str("...");
            truncated
        };
        format!("{type_name} \"{rendered}\"")
    } else if value::is_f64(encoded) {
        // 数字按 JS Number::toString 精确格式化（-0 渲染为 "0"，与 V8 一致）。
        format!(
            "{type_name} {}",
            wjsm_builtins::number_format::format_number_js(value::decode_f64(encoded))
        )
    } else if value::is_null(encoded) || value::is_bool(encoded) {
        format!("{type_name} {}", render_value(state, encoded))
    } else {
        type_name.to_string()
    }
}

/// GetIterator（§7.4.3）源缺少可调用 @@iterator 方法时的 TypeError：V8
/// kNotIterableNoSymbolLoad 回退形态。
pub(super) fn not_iterable_type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
) -> i64 {
    let callsite = default_call_site(state, source);
    type_error(
        ctx,
        state,
        &format!("{callsite} is not iterable (cannot read property Symbol(Symbol.iterator))"),
    )
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

/// 当前激活是否为对该 builtin 本体的 [[Construct]] 调用（`new Symbol()` /
/// `Reflect.construct(Symbol, ..)` / 子类 super()）：new.target 已定义且
/// callee 正是该 builtin 的 native callable。直连 CallBuiltin 站点（如
/// BigInt 字面量物化）复用外层 JS 激活，其 callee 为用户函数，不会误判。
pub(super) fn is_builtin_construct_call(
    state: &NativeAgentState,
    builtin: wjsm_ir::Builtin,
) -> bool {
    state.activations.last().is_some_and(|activation| {
        !value::is_undefined(activation.new_target)
            && state
                .native_callable_builtin(activation.callee)
                .is_some_and(|(callee_builtin, _)| callee_builtin == builtin)
    })
}

pub(crate) fn is_constructor_value(state: &NativeAgentState, encoded: i64) -> bool {
    // Proxy 的 [[Construct]] 在 target 可构造时存在（ProxyCreate 10.5.12）。
    if value::is_proxy(encoded) {
        return super::proxy::is_constructor_proxy(state, encoded);
    }
    if !value::is_callable(encoded) {
        return false;
    }
    match state.native_callable_kind(encoded) {
        // builtin 值按 §7.2.4 分类：构造器白名单见 is_constructor_builtin。
        Some(crate::NativeCallableKind::Builtin(builtin, _)) => {
            crate::builtin_metadata::is_constructor_builtin(builtin)
        }
        // bound 函数的 [[Construct]] 存在当且仅当 target 可构造
        // （§10.4.1.2 BoundFunctionCreate 步骤 7）。
        Some(crate::NativeCallableKind::Bound(index)) => state
            .bound_functions
            .get(usize::try_from(index).unwrap_or(usize::MAX))
            .and_then(|bound| bound.as_ref())
            .is_some_and(|bound| is_constructor_value(state, bound.target)),
        Some(crate::NativeCallableKind::Intl(kind)) => super::intl::is_constructor(kind),
        Some(crate::NativeCallableKind::DateMethod(_)) => false,
        Some(crate::NativeCallableKind::FunctionPrototype) => false,
        Some(crate::NativeCallableKind::SpeciesGetter) => false,
        // %TypedArray% 本体是构造器（[[Construct]] 抛错在调用期），但
        // from / of 与 @@toStringTag getter 是普通方法 / 访问器函数。
        Some(
            crate::NativeCallableKind::TypedArrayFrom
            | crate::NativeCallableKind::TypedArrayOf
            | crate::NativeCallableKind::TypedArrayToStringTag,
        ) => false,
        // %Iterator% 本体是构造器（抽象性在调用期抛错），其余 Iterator
        // Helper 家族函数（from / 原型方法 / 访问器 / next / return）皆非。
        Some(
            crate::NativeCallableKind::IteratorStaticFrom
            | crate::NativeCallableKind::IteratorProto(_)
            | crate::NativeCallableKind::IteratorProtoIterator
            | crate::NativeCallableKind::IteratorConstructorGetter
            | crate::NativeCallableKind::IteratorConstructorSetter
            | crate::NativeCallableKind::IteratorToStringTagGetter
            | crate::NativeCallableKind::IteratorToStringTagSetter
            | crate::NativeCallableKind::IteratorHelperNext
            | crate::NativeCallableKind::IteratorHelperReturn
            | crate::NativeCallableKind::IteratorWrapNext
            | crate::NativeCallableKind::IteratorWrapReturn,
        ) => false,
        // ECMA 层可见的方法 / 迭代器 / 宿主函数值：无 [[Construct]]。
        Some(
            crate::NativeCallableKind::ArrayToString
            | crate::NativeCallableKind::RegExpToString
            | crate::NativeCallableKind::ErrorToString
            | crate::NativeCallableKind::ArrayIterator(_)
            | crate::NativeCallableKind::IteratorFamilyNext(_)
            | crate::NativeCallableKind::ArgumentsStrictCallee
            | crate::NativeCallableKind::BufferMethod(_)
            | crate::NativeCallableKind::BufferStatic(_)
            | crate::NativeCallableKind::BufferTranscode
            | crate::NativeCallableKind::Fetch(_)
            | crate::NativeCallableKind::CjsRequire(_)
            | crate::NativeCallableKind::CjsResolve(_)
            | crate::NativeCallableKind::CjsResolvePaths(_)
            | crate::NativeCallableKind::ImportMetaResolve(_)
            | crate::NativeCallableKind::PromiseResolve(_)
            | crate::NativeCallableKind::PromiseReject(_)
            | crate::NativeCallableKind::ProxyRevoke(_)
            | crate::NativeCallableKind::ProcessExit
            | crate::NativeCallableKind::ProcessWrite(_)
            | crate::NativeCallableKind::ProcessStreamEnd(_)
            | crate::NativeCallableKind::ProcessStreamReturnThis
            | crate::NativeCallableKind::ProcessStdin(_)
            | crate::NativeCallableKind::ProcessHrtime
            | crate::NativeCallableKind::ProcessHrtimeBigInt
            | crate::NativeCallableKind::ProcessUptime
            | crate::NativeCallableKind::ProcessMemoryUsage
            | crate::NativeCallableKind::ProcessCpuUsage
            | crate::NativeCallableKind::ProcessCwd
            | crate::NativeCallableKind::ProcessOn
            | crate::NativeCallableKind::ProcessNextTick
            | crate::NativeCallableKind::SetImmediate
            | crate::NativeCallableKind::Gc,
        ) => false,
        // Node 内建模块桥（globalThis.__wjsm_node_* 上的宿主方法）与
        // test262 agent 桥：内部方法值，无 [[Construct]]。
        Some(
            crate::NativeCallableKind::NodeNet(_)
            | crate::NativeCallableKind::NodeTls(_)
            | crate::NativeCallableKind::NodeZlib(_)
            | crate::NativeCallableKind::NodeFs(_)
            | crate::NativeCallableKind::NodeCrypto(_)
            | crate::NativeCallableKind::NodeDgram(_)
            | crate::NativeCallableKind::NodeAsyncHooks(_)
            | crate::NativeCallableKind::NodeOs(_)
            | crate::NativeCallableKind::NodeTty(_)
            | crate::NativeCallableKind::Idna(_)
            | crate::NativeCallableKind::NodeVm(_)
            | crate::NativeCallableKind::NodeChildProcess(_)
            | crate::NativeCallableKind::NodePerfHooks(_)
            | crate::NativeCallableKind::NodeWorkerThreads(_)
            | crate::NativeCallableKind::Test262Agent(_),
        ) => false,
        // Streams / EventTarget 家族值全部是原型方法与访问器（构造器
        // 本体以 Builtin 形态分类），无 [[Construct]]。
        Some(crate::NativeCallableKind::Stream(_) | crate::NativeCallableKind::Events(_)) => false,
        // WebEncoding 家族按变体分类：构造器本体与 atob / btoa（Node
        // 口径）为构造器，原型方法与 getter 皆非。
        Some(crate::NativeCallableKind::WebEncoding(kind)) => {
            super::web_encoding::is_constructor(kind)
        }
        // Proxy 调用跳板等价于 proxy 本体：[[Construct]] 存在性沿
        // target 链判定（§10.5.13）。
        Some(
            crate::NativeCallableKind::ProxyCall(index)
            | crate::NativeCallableKind::ProxyConstruct(index),
        ) => super::proxy::is_constructor_proxy(state, value::encode_proxy_handle(index)),
        // 携带 [[Construct]] 的构造器本体：全局构造器、Node Timeout /
        // Immediate（Node 里是普通函数声明）与 %TypedArray% / %Iterator%
        // 抽象构造器（IsConstructor 为真，构造期自抛）。
        Some(
            crate::NativeCallableKind::ObjectConstructor
            | crate::NativeCallableKind::ArrayConstructor
            | crate::NativeCallableKind::RealmArrayConstructor(_)
            | crate::NativeCallableKind::BufferConstructor
            | crate::NativeCallableKind::AggregateErrorConstructor
            | crate::NativeCallableKind::StringConstructor
            | crate::NativeCallableKind::FunctionConstructor
            | crate::NativeCallableKind::TimerConstructor(_)
            | crate::NativeCallableKind::TypedArrayConstructor
            | crate::NativeCallableKind::IteratorConstructor,
        ) => true,
        None => state
            .callable_function(encoded)
            .is_some_and(|function| function.needs_prototype),
    }
}

pub(super) fn object_handle(encoded: i64) -> Option<u32> {
    (value::is_object(encoded) || value::is_array(encoded)).then(|| value::decode_handle(encoded))
}

/// ECMAScript Type(V) 为 Object（§6.1）：普通堆对象 / 数组 / callable /
/// Proxy / RegExp 等宿主对象表示均计入；基元与 null/undefined 为 false。
/// Object 静态方法族与 Reflect 的「called on non-object」入口校验共用。
pub(super) fn is_language_object(encoded: i64) -> bool {
    value::is_object(encoded)
        || value::is_array(encoded)
        || value::is_callable(encoded)
        || value::is_proxy(encoded)
        || value::is_regexp(encoded)
}

/// IsArray（§7.2.2）：Proxy 沿 [[ProxyTarget]] 链穿透判定；revoked proxy
/// 返回 None（调用方按规范抛 TypeError）。
pub(super) fn is_array_value(state: &NativeAgentState, encoded: i64) -> Option<bool> {
    if value::is_proxy(encoded) {
        return super::proxy::is_array_target(state, encoded);
    }
    Some(value::is_array(encoded))
}

/// Construct(F, argumentsList, newTarget)（§7.3.15）：Proxy 走 [[Construct]]
/// trap；其余按 OrdinaryCreateFromConstructor 预建 this（newTarget 的
/// `prototype` 为对象时作为 [[Prototype]]），构造器返回对象以之为结果，否则
/// 用预建 this。调用方须先通过 IsConstructor 校验 F 与 newTarget。
pub(super) fn construct_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    constructor: i64,
    arguments: &[i64],
    new_target: i64,
) -> i64 {
    if value::is_proxy(constructor) {
        return super::proxy::construct(ctx, state, constructor, arguments, new_target);
    }
    let Ok(this_value) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    // 预建 this 在 prototype 读取（可再入 getter / Proxy trap）与构造调用
    // 期间需锚根；构造器实参由调用方保证存活。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(this_value);
    let res = (|| {
        let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        let prototype = get_property(ctx, state, new_target, prototype_key)
            .unwrap_or_else(|()| fail_dispatch(ctx));
        if value::is_exception(prototype) {
            return prototype;
        }
        if let Some(prototype) = object_handle(prototype)
            && state
                .gc
                .heap()
                .set_prototype(value::decode_handle(this_value), prototype)
                .is_err()
        {
            return fail_dispatch(ctx);
        }
        let result = state
            .invoke_constructor(ctx, constructor, new_target, this_value, arguments)
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(result) {
            return result;
        }
        if value::is_js_object(result) {
            result
        } else {
            this_value
        }
    })();
    state.temporary_roots.truncate(initial_temp_roots);
    res
}

/// %RegExp.prototype% 访问器族（§22.2.6）的键名：命中返回 brand 检查错误
/// 消息所用的 getter 名（generic 的 flags getter 首个内部读是 hasIndices，
/// 与 V8 报错口径一致）。
fn regexp_accessor_name(state: &mut NativeAgentState, key: i64) -> Option<&'static str> {
    for name in [
        "source",
        "global",
        "ignoreCase",
        "multiline",
        "sticky",
        "unicode",
        "unicodeSets",
        "dotAll",
        "hasIndices",
    ] {
        if state.text_matches(key, name) {
            return Some(name);
        }
    }
    state.text_matches(key, "flags").then_some("hasIndices")
}

/// proto 槽 u32 → 编码值：null 哨兵为 None；Proxy / RegExp 标记位还原为
/// 对应宿主 tag；其余按堆对象类型编码 object / array。
pub(crate) fn decode_proto_slot(state: &NativeAgentState, prototype: u32) -> Option<i64> {
    if prototype == wjsm_gc::PROTO_NULL_SENTINEL {
        return None;
    }
    if prototype & wjsm_gc::PROTO_PROXY_FLAG != 0 {
        return Some(value::encode_proxy_handle(
            prototype & !wjsm_gc::PROTO_PROXY_FLAG,
        ));
    }
    if prototype & wjsm_gc::PROTO_REGEXP_FLAG != 0 {
        return Some(value::encode_regexp_handle(
            prototype & !wjsm_gc::PROTO_REGEXP_FLAG,
        ));
    }
    let encoded = if state.gc.heap().object_type(prototype).ok()
        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
    {
        value::encode_handle(value::TAG_ARRAY, prototype)
    } else {
        value::encode_object_handle(prototype)
    };
    Some(encoded)
}

/// 编码值 → proto 槽 u32：null 写哨兵，Proxy / RegExp 置对应标记位，普通
/// 堆对象 / 数组取句柄；callable 等无法进 proto 槽的值返回 None。
pub(super) fn encode_proto_slot(prototype: i64) -> Option<u32> {
    if value::is_null(prototype) {
        return Some(wjsm_gc::PROTO_NULL_SENTINEL);
    }
    if value::is_proxy(prototype) {
        return Some(value::decode_proxy_handle(prototype) | wjsm_gc::PROTO_PROXY_FLAG);
    }
    if value::is_regexp(prototype) {
        return Some(value::decode_regexp_handle(prototype) | wjsm_gc::PROTO_REGEXP_FLAG);
    }
    object_handle(prototype)
}

fn heap_prototype_value(state: &NativeAgentState, object: i64) -> Result<Option<i64>, ()> {
    let handle = object_handle(object).ok_or(())?;
    let prototype = state.gc.heap().prototype(handle).map_err(|_| ())?;
    Ok(decode_proto_slot(state, prototype))
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
    // Module Namespace Exotic Object 的 [[Set]] 恒返回 false（§10.4.6.5）。
    // 失败原因决定 strict TypeError 文案（与 V8 一致）：既有导出/@@toStringTag
    // 按只读属性（"Cannot assign to read only property ... of object
    // '[object Module]'"），新键按不可扩展。
    if state.module_namespace_objects.contains(&target_handle) {
        let failure = if own.is_some() {
            SetFailure::ReadOnly
        } else {
            SetFailure::NotExtensible
        };
        return Ok(SetCompletion::Failed(failure));
    }
    if own.is_none() {
        let prototype = state
            .gc
            .heap()
            .prototype(target_handle)
            .map_err(|_| fail_dispatch(ctx))?;
        if let Some(prototype) = decode_proto_slot(state, prototype) {
            if value::is_proxy(prototype) {
                return super::proxy::set(
                    ctx,
                    state,
                    prototype,
                    encoded_property_key(key),
                    stored,
                    receiver,
                );
            }
            if value::is_regexp(prototype) {
                // 链上 RegExp 层：自有 lastIndex 是可写数据属性，合成方法皆非
                // 访问器——两者对 OrdinarySet 都归结为「在 receiver 上创建数据
                // 属性」；其余键继续沿 %RegExp.prototype% 堆链找访问器。
                if state.text_matches(encoded_property_key(key), "lastIndex") {
                    return assign_data_property_to_receiver(ctx, state, receiver, key, stored);
                }
                if let Some(regexp_prototype) = state.regexp_prototype {
                    return ordinary_set_key(ctx, state, regexp_prototype, key, stored, receiver);
                }
                return assign_data_property_to_receiver(ctx, state, receiver, key, stored);
            }
            return ordinary_set_key(ctx, state, prototype, key, stored, receiver);
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

fn record_elem_kind_feedback(
    state: &NativeAgentState,
    feedback: ValidatedFeedbackSlot,
    object: i64,
) {
    let mut slot = NativeAgentState::load_feedback_slot(feedback);
    if let Some(array) = state.typed_arrays.get(&value::decode_handle(object)) {
        slot.flags |= wjsm_ir::constants::FEEDBACK_FLAG_TYPED_ARRAY;
        slot.slot_or_kind = u32::from(array.kind.as_code());
    } else if value::is_array(object) {
        let handle = value::decode_handle(object);
        if let Ok(kind) = state.gc.heap().array_kind(handle) {
            slot.slot_or_kind = kind;
            if slot.poly_key[3] == 0 {
                slot.poly_key[3] = kind.saturating_add(1);
            }
        }
        slot.flags &= !wjsm_ir::constants::FEEDBACK_FLAG_TYPED_ARRAY;
    }
    NativeAgentState::store_feedback_slot(feedback, slot);
}

fn record_poly_shape(slot: &mut NativeFeedbackSlot, shape_id: u32) {
    if shape_id == 0 {
        return;
    }
    if slot.poly_len == 0 {
        slot.poly_key[0] = shape_id;
        slot.poly_len = 1;
        return;
    }
    let taken = slot.poly_len as usize;
    if slot.poly_key[..taken].contains(&shape_id) || slot.shape_id == shape_id {
        slot.poly_len = slot.poly_len.max(1);
        return;
    }
    if slot.poly_len >= wjsm_ir::constants::FEEDBACK_POLY_MEGAMORPHIC {
        slot.poly_len = wjsm_ir::constants::FEEDBACK_POLY_MEGAMORPHIC;
        return;
    }
    let index = slot.poly_len as usize;
    if index < slot.poly_key.len() {
        slot.poly_key[index] = shape_id;
        slot.poly_len += 1;
    } else {
        slot.poly_len = wjsm_ir::constants::FEEDBACK_POLY_MEGAMORPHIC;
    }
}

/// GetPropIc 的 miss 回填：按「自有数据 → 原型链数据 → accessor」优先级回填
/// CLIF 快路径；proxy / 字典 shape / 数组 / 缺失 / 非 callable accessor 一律
/// 永久退化 MEGAMORPHIC（此后每次访问都走宿主完整 [[Get]]）。
fn backfill_get_prop_ic(
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    ic_slot_ptr: i64,
    feedback_slot: Option<ValidatedFeedbackSlot>,
) {
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
            if let Some(feedback) = feedback_slot {
                // SAFETY: 当前 image 反馈槽，owner 线程唯一写入。
                let mut slot = unsafe { feedback.slot().read_unaligned() };
                slot.shape_id = shape_id;
                slot.slot_or_kind = value_index;
                slot.flags |= constants::FEEDBACK_FLAG_OWN_DATA;
                record_poly_shape(&mut slot, shape_id);
                unsafe { feedback.slot().write_unaligned(slot) };
            }
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

pub(crate) fn property_key(state: &mut NativeAgentState, encoded: i64) -> Option<PropertyKey> {
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
    // 本函数是宿主内部 Get(O, P)（§7.3.2，规范前提 O 为对象）；成员访问的
    // ToObject TypeError 由 GetProp / GetPropIc / GetElem 入口的
    // `get_on_nullish_base` 负责。内部调用点传入 nullish 时按安全网返回
    // undefined，不得在此抛错——否则选项包缺省读取等合法路径会被误伤。
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
    // ArrayBuffer / DataView / SharedArrayBuffer 实例无早退拦截：实例创建
    // 即接线各自 prototype，byteLength 等访问器沿真实原型链以 receiver 为
    // this 分派（brand 检查在 getter 内完成）。
    if value::is_proxy(object) {
        return Ok(super::proxy::get(ctx, state, object, key, receiver));
    }
    if value::is_regexp(object) {
        // 链上 holder 为 RegExp 且 receiver 非 RegExp（对象以 RegExp 为原型）
        // 时，访问器族（source / flags / 各标志位，§22.2.6）以 receiver 为
        // this 作 brand 检查：receiver 无 [[OriginalSource]]/[[OriginalFlags]]
        // 内部槽即抛 TypeError（自有数据属性 lastIndex 与 exec 等方法不受
        // 影响）。receiver 为 RegExp 时按 this=receiver 求值。
        if let Some(name) = regexp_accessor_name(state, key) {
            if value::is_regexp(receiver) {
                if let Some(property) = super::regexp::get_property(ctx, state, receiver, key) {
                    return Ok(property);
                }
            } else {
                return Ok(type_error(
                    ctx,
                    state,
                    &format!("RegExp.prototype.{name} getter called on non-RegExp object"),
                ));
            }
        }
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
        // %Array.prototype% 已物化（CreateGlobalObject 引导时完成）后，数组
        // proto 槽为 null 哨兵只能来自显式 `Object.setPrototypeOf(arr, null)`
        // ——分配即接线真实句柄。链已断，不得回退兜底合成复活方法；未物化的
        // 引导期内部数组保持兜底合成。
        if state.array_prototype.is_some() {
            return Ok(value::encode_undefined());
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
        if let Some(property) = state.primitive_property(object, key) {
            return Ok(property);
        }
        // OrdinaryGet 经 ToObject（§7.1.18）：合成方法未命中后沿基元包装对象
        // 的真实 [[Prototype]] 堆链上行，%Object.prototype% 的自有属性
        // （hasOwnProperty / __proto__ 访问器等）对基元可见，receiver 保持基元。
        let Some(prototype) = state.primitive_wrapper_prototype(object) else {
            return Ok(value::encode_undefined());
        };
        return get_property_with_receiver(ctx, state, prototype, key, receiver);
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
                // 隐式链尾的父层是 %Function.prototype%（§20.2.3）：其自有
                // name（""）与 length（+0）在 receiver own 层缺失（删除落
                // 墓碑）后仍须继承可见，经真实 FunctionPrototype callable
                // 解析以尊重其上的覆盖与删除；tail 为其自身时继续上行。
                if (state.text_matches(encoded_key, "name")
                    || state.text_matches(encoded_key, "length"))
                    && let Some(prototype) =
                        state.native_callable(crate::NativeCallableKind::FunctionPrototype)
                    && value::strip_gc_color(prototype) != tail
                {
                    return get_property_with_receiver(
                        ctx,
                        state,
                        prototype,
                        encoded_key,
                        receiver,
                    );
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
    // String exotic 包装对象的自有 "length" 与在界索引（§10.4.3）先于原型
    // 链解析：%String.prototype% 的自有 "length"（+0）不得遮蔽实例值。
    if let Some(primitive) = boxed_primitive_value(state, object)
        && value::is_string(primitive)
        && (state.text_matches(encoded_key, "length")
            || array_index(state, encoded_key).is_some_and(|index| {
                state
                    .string_len(primitive)
                    .is_some_and(|length| (index as usize) < length)
            }))
    {
        return get_property_with_receiver(ctx, state, primitive, encoded_key, receiver);
    }
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
            // （字符串 length/索引等 exotic own 属性与原语方法）。固有原型
            // 自身携带 [[StringData]]（%String.prototype%，§22.1.3）时不
            // 回退：原语的包装原型即本对象，递归不会带来新的属性来源。
            if let Some(primitive) = boxed_primitive_value(state, object)
                && state.primitive_wrapper_prototype(primitive) != Some(object)
            {
                return get_property_with_receiver(ctx, state, primitive, encoded_key, receiver);
            }
            Ok(value::encode_undefined())
        }
        Err(wjsm_gc::HeapAccessV2Error::ExoticPrototype { slot }) => {
            // 链上出现宿主 exotic 原型（Proxy / RegExp）：解码标记位后继续
            // [[Get]]——Proxy 走 get trap，RegExp 递归自身分支（自有
            // lastIndex / 合成方法 / %RegExp.prototype% 上行）。
            let Some(prototype) = decode_proto_slot(state, slot) else {
                return Err(());
            };
            if value::is_proxy(prototype) {
                return Ok(super::proxy::get(
                    ctx,
                    state,
                    prototype,
                    encoded_key,
                    receiver,
                ));
            }
            get_property_with_receiver(ctx, state, prototype, encoded_key, receiver)
        }
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
    // 命名空间 receiver（如 Reflect.set(target, k, v, ns)）：其
    // [[DefineOwnProperty]] 不允许经 OrdinarySet 创建/改写数据属性，
    // [[Set]] 结果恒 false（§10.4.6.5 / §10.4.6.6）。
    if state.module_namespace_objects.contains(&receiver_handle) {
        let own = state
            .gc
            .heap()
            .get_property_slot(receiver_handle, key)
            .map_err(|_| fail_dispatch(ctx))?;
        let failure = if own.is_some() {
            SetFailure::ReadOnly
        } else {
            SetFailure::NotExtensible
        };
        return Ok(SetCompletion::Failed(failure));
    }
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

/// delete 操作符对属性引用的求值（§13.5.5.9 步骤 5）：ToObject 先于键
/// 转换拒绝 nullish 基座，proxy 走 [[Delete]] trap（strict 时 falsish 抛
/// proxy 专属 TypeError），基元接收者按新建包装对象的自有属性判定，
/// 其余对象走通用 [[Delete]]；deleteStatus 为 false 时 strict 抛
/// TypeError（步骤 5.d），sloppy 返回 false。
pub(super) fn delete_property_operator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    strict: bool,
) -> i64 {
    if value::is_null(object) || value::is_undefined(object) {
        return type_error(ctx, state, "Cannot convert undefined or null to object");
    }
    let key = match to_property_key_value(ctx, state, key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    if value::is_proxy(object) {
        return super::proxy::delete_for_operator(ctx, state, object, key, strict);
    }
    delete_property_operator_with_key(ctx, state, object, key, strict)
}

/// [[Delete]] 已转换属性键的非 proxy 收口：供 delete 操作符与 proxy
/// 无 trap 下钻到最终 target 时复用（strict 位随调用链透传）。
pub(super) fn delete_property_operator_with_key(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    strict: bool,
) -> i64 {
    if let Some(result) = delete_on_primitive_receiver(ctx, state, object, key, strict) {
        return result;
    }
    match delete_property(state, object, key) {
        Ok(true) => value::encode_bool(true),
        Ok(false) if !strict => value::encode_bool(false),
        Ok(false) => strict_delete_failure_error(ctx, state, object, key),
        Err(()) => fail_dispatch(ctx),
    }
}

/// delete 操作符对基元 base 的终局：ToObject（§13.5.5.9 步骤 5.b）的包装
/// 对象是新建实例，除字符串的 length 与在界索引（自有不可配置，§10.4.3）
/// 外不存在自有属性，[[Delete]] 恒为 true；不可配置命中时 sloppy 返回
/// false、strict 抛 TypeError。返回 `None` 表示接收者不是基元。
fn delete_on_primitive_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: i64,
    strict: bool,
) -> Option<i64> {
    let is_primitive = value::is_string(object)
        || value::is_f64(object)
        || value::is_bool(object)
        || value::is_symbol(object)
        || value::is_bigint(object);
    if !is_primitive {
        return None;
    }
    let undeletable = value::is_string(object)
        && (state.text_matches(key, "length")
            || array_index(state, key)
                .is_some_and(|index| (index as usize) < state.string_len(object).unwrap_or(0)));
    if !undeletable {
        return Some(value::encode_bool(true));
    }
    if !strict {
        return Some(value::encode_bool(false));
    }
    Some(strict_delete_failure_error(ctx, state, object, key))
}

/// strict delete 失败的 TypeError，消息与 V8 对齐（Node 同口径）：
/// `Cannot delete property 'k' of #<Object> / [object Array] / [object
/// String] / 函数源文本`。
pub(super) fn strict_delete_failure_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: i64,
) -> i64 {
    let owner = if value::is_string(receiver) {
        "[object String]".to_string()
    } else {
        property_write::render_receiver_brief(state, receiver)
    };
    let message = format!(
        "Cannot delete property '{}' of {owner}",
        render_value(state, key)
    );
    type_error(ctx, state, &message)
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
        // 静态成员会被惰性合成复活（String.raw 等）：删除即落墓碑。
        state.record_intrinsic_tombstone_after_delete(object, encoded_key);
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
        // %Array.prototype% 的方法按 receiver 惰性合成：在原型对象上删除
        // 即落墓碑禁止复活；普通数组实例删除缺失自有属性不影响原型可见性。
        if state.array_prototype.map(value::strip_gc_color) == Some(value::strip_gc_color(object)) {
            state.record_intrinsic_tombstone_after_delete(object, encoded_key);
        }
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
    // realm 全局对象上仍会被惰性内建合成的名字（parseInt / console 等在
    // Node 中是可配置自有属性）：删除即落墓碑禁止复活，且即使尚未物化
    // 自有槽位也视为删除成功。自有属性优先规则保证此处 Some 即纯合成态。
    if state.global_property(object, encoded_key).is_some() {
        state
            .intrinsic_tombstones
            .insert((value::strip_gc_color(object), key));
        return Ok(true);
    }
    Ok(removed)
}
/// HasProperty（§7.3.11，含原型链）：Proxy（顶层或链中）走 has trap，
/// trap 异常以 `Err` 携带宿主异常值上抛；RegExp 归约自有 lastIndex 与
/// 合成方法后沿 %RegExp.prototype% 上行。
pub(super) fn has_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    encoded_key: i64,
) -> Result<bool, i64> {
    if value::is_proxy(object) {
        let result = super::proxy::has(ctx, state, &[object, encoded_key]);
        if value::is_exception(result) {
            return Err(result);
        }
        return Ok(is_truthy(state, result));
    }
    if value::is_regexp(object) {
        if state.text_matches(encoded_key, "lastIndex")
            || super::regexp::get_property(ctx, state, object, encoded_key).is_some()
        {
            return Ok(true);
        }
        let Some(prototype) = state.regexp_prototype else {
            return Ok(false);
        };
        return has_property(ctx, state, prototype, encoded_key);
    }
    if value::is_array(object) && state.text_matches(encoded_key, "length") {
        return Ok(true);
    }
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        if let Some(index) = array_index(state, encoded_key) {
            match state.gc.heap().get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => return Ok(true),
                Ok(_) => {}
                Err(_) => return Ok(false),
            }
        }
        let Some(key) = property_key(state, encoded_key) else {
            return Ok(false);
        };
        if state.array_properties.contains_key(&(handle, key))
            || state.array_accessors.contains_key(&(handle, key))
        {
            return Ok(true);
        }
        // 合成方法在语义上属于 %Array.prototype% 层：receiver 即原型对象或
        // 引导期未物化时就地合成，否则交给真实堆链上行解析（原型被替换 /
        // 置 null 的数组不得越过链直接命中合成方法）。
        if (state.array_prototype == Some(object) || state.array_prototype.is_none())
            && state.primitive_property(object, encoded_key).is_some()
        {
            return Ok(true);
        }
        let Ok(Some(prototype)) = heap_prototype_value(state, object) else {
            return Ok(false);
        };
        return has_property(ctx, state, prototype, encoded_key);
    }
    let Some(key) = property_key(state, encoded_key) else {
        return Ok(false);
    };
    if value::is_callable(object) {
        // HasProperty 与 [[Get]] 同链：逐层自有属性 → 非 callable 原型递归
        // 对象路径 → 显式 null 缺失 → 链尾隐式 Function.prototype 内建，
        // 再沿 %Object.prototype% 上行（§20.2.3）。
        return match callable_chain::resolve(state, object, key) {
            CallableChainHit::Accessor { .. } | CallableChainHit::Data { .. } => Ok(true),
            CallableChainHit::Object { prototype } => {
                has_property(ctx, state, prototype, encoded_key)
            }
            CallableChainHit::Null => Ok(false),
            CallableChainHit::Implicit { tail } => {
                if state.primitive_property(tail, encoded_key).is_some()
                    || state.text_matches(encoded_key, "constructor")
                {
                    return Ok(true);
                }
                // 与 [[Get]] 同构：own 层删除后 name/length 沿隐式
                // %Function.prototype% 的自有属性继续可见。
                if (state.text_matches(encoded_key, "name")
                    || state.text_matches(encoded_key, "length"))
                    && let Some(prototype) =
                        state.native_callable(crate::NativeCallableKind::FunctionPrototype)
                    && value::strip_gc_color(prototype) != tail
                {
                    return has_property(ctx, state, prototype, encoded_key);
                }
                let Some(prototype) = state.object_prototype else {
                    return Ok(false);
                };
                has_property(ctx, state, prototype, encoded_key)
            }
        };
    }
    let Some(handle) = object_handle(object) else {
        return Ok(false);
    };
    match state
        .gc
        .heap()
        .get_property_slot_on_proto_chain(handle, key)
    {
        Ok(Some(_)) => Ok(true),
        // realm 全局对象的惰性内建（parseInt / console 等）对 HasProperty
        // 可见（`"parseInt" in globalThis` 与 Node 一致），墓碑与自有属性
        // 优先规则由 `global_property` 内部处理。
        Ok(None) => Ok(state.global_property(object, encoded_key).is_some()
            || state.primitive_property(object, encoded_key).is_some()
            || boxed_primitive_value(state, object)
                .is_some_and(|primitive| boxed_primitive_has(state, primitive, encoded_key))),
        // 链上出现宿主 exotic 原型（Proxy / RegExp）：解码标记位后递归继续。
        Err(wjsm_gc::HeapAccessV2Error::ExoticPrototype { slot }) => {
            let Some(prototype) = decode_proto_slot(state, slot) else {
                return Ok(false);
            };
            has_property(ctx, state, prototype, encoded_key)
        }
        Err(_) => Ok(false),
    }
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

/// 字符串原语或 boxed String 包装对象的 [[StringData]]；其余为 None。
/// 字符串方法对 this 做 ToString 前先经此解箱（ToPrimitive 对包装对象
/// 命中原语，§7.1.17），使 `String.prototype.slice.call(new String(...))`
/// 与 Node 对齐。
pub(super) fn primitive_string(state: &NativeAgentState, source: i64) -> Option<i64> {
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
    // Array.prototype.{values,keys,entries} / 数组与 arguments 的 @@iterator：
    // CreateArrayIterator 快速路径，receiver 分类与 array_iterator 一致。
    if let Some(crate::NativeCallableKind::ArrayIterator(kind)) = state.native_callable_kind(method)
    {
        return array_iterator_source(state, source).map(|iterator_source| (iterator_source, kind));
    }
    let (builtin, _) = state.native_callable_builtin(method)?;
    let handle = value::decode_handle(source);
    match builtin {
        wjsm_ir::Builtin::StringIterator if let Some(text) = primitive_string(state, source) => {
            Some((
                super::super::NativeIteratorSource::String(text),
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
    // GetIterator（§7.4.3）经 GetMethod → GetV 对 nullish 做 ToObject：文案与
    // 成员访问同源（V8 kNotIterableNoSymbolLoad 回退形态，CallPrinter 的源
    // 文本渲染未实现）。
    if let Some(exception) = get_on_nullish_base(ctx, state, source, symbol) {
        return exception;
    }
    let Ok(method) = get_property(ctx, state, source, symbol) else {
        return not_iterable_type_error(ctx, state, source);
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
    // 方法缺失或非可调用：V8 对 sync GetIterator 统一按 kNotIterableNoSymbolLoad
    // 渲染源值（非「is not a function」形态）。
    if !value::is_callable(method) {
        return not_iterable_type_error(ctx, state, source);
    }
    if let Some((source_kind, iterator_kind)) = intrinsic_iterator_source(state, source, method) {
        // 家族原型先于实例物化：attach 内部不再有 GC 重试分配，未根化的
        // 新实例不会被移动。
        let Some(family) = super::iterator_prototypes::family_of_source(source_kind) else {
            return fail_dispatch(ctx);
        };
        if super::iterator_prototypes::ensure_prototype(state, family).is_none() {
            return fail_dispatch(ctx);
        }
        let Ok(iterator) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
            return fail_dispatch(ctx);
        };
        if let Err(exception) = super::iterator_prototypes::attach(ctx, state, iterator, family) {
            return exception;
        }
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
    // §7.4.2 GetIteratorFromMethod 步骤 2：调用结果非对象抛 TypeError，
    // 文案对齐 V8 kSymbolIteratorInvalid。
    if !value::is_js_object(iterator) {
        return type_error(
            ctx,
            state,
            "Result of the Symbol.iterator method is not an object",
        );
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

/// CreateArrayIterator（§23.1.5.1）的 receiver 分类：数组走 exotic 长度，
/// 字符串（原语与 boxed）保持既有 [[StringData]] 迭代路径，其余对象按
/// array-like 读 length / 索引属性；nullish 与其余原语按 ToObject 失败拒绝。
fn array_iterator_source(
    state: &NativeAgentState,
    source: i64,
) -> Option<super::super::NativeIteratorSource> {
    if value::is_array(source) {
        Some(super::super::NativeIteratorSource::Array(
            value::decode_handle(source),
        ))
    } else if let Some(text) = primitive_string(state, source) {
        Some(super::super::NativeIteratorSource::String(text))
    } else if value::is_js_object(source) {
        Some(super::super::NativeIteratorSource::ArrayLike(
            value::decode_handle(source),
        ))
    } else {
        None
    }
}

pub(crate) fn array_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
    kind: super::super::NativeIteratorKind,
) -> i64 {
    let Some(iterator_source) = array_iterator_source(state, source) else {
        return type_error(ctx, state, "Array iterator receiver is not an object");
    };
    let Some(family) = super::iterator_prototypes::family_of_source(iterator_source) else {
        return fail_dispatch(ctx);
    };
    if super::iterator_prototypes::ensure_prototype(state, family).is_none() {
        return fail_dispatch(ctx);
    }
    let Ok(iterator) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    if let Err(exception) = super::iterator_prototypes::attach(ctx, state, iterator, family) {
        return exception;
    }
    state.array_iterators.insert(
        value::decode_handle(iterator),
        super::super::NativeArrayIterator {
            source: iterator_source,
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
    if let Err(exception) = ensure_current(ctx, state, handle) {
        return exception;
    }
    let Some(iterator) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(iterator.done)
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
    if let Err(exception) = ensure_current(ctx, state, handle) {
        return exception;
    }
    let Some(iterator) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    if let super::super::NativeIteratorSource::Custom(_) = iterator.source {
        // [[Done]] 为 true 时步进语义（advance = IteratorStepValue，§7.4.8）
        // 直接返回 DONE（映射为 undefined），不得读取迭代结果的 value 属性
        // ——done 结果对象的 value getter 不可观察。非步进的 IteratorValue
        // 是对当前 result 的普通 Get（§7.4.5）：yield* 委托在 done 后仍须
        // 读取最终 result.value 作为委托表达式的值（§27.5.3.7 步骤 7.a.iii）。
        if iterator.done && advance {
            return value::encode_undefined();
        }
        let Some(result) = iterator.current else {
            return fail_dispatch(ctx);
        };
        let Some(key) = state.intern_text("value".into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        let Ok(stored) = get_property(ctx, state, result, key) else {
            return fail_dispatch(ctx);
        };
        // value 读取抛出（getter）：§7.4.8 步骤 8.b 置 [[Done]] 为 true
        // 再传播，后续 IteratorClose 不得再调用 return()。
        if value::is_exception(stored) {
            return mark_custom_done(state, handle, stored);
        }
        if advance {
            let iterator = state
                .array_iterators
                .get_mut(&handle)
                .expect("iterator entry was resolved above");
            iterator.index = iterator.index.saturating_add(1);
            iterator.current = None;
        }
        return stored;
    }
    // 内建源：预取值在 current（ensure_current 已推进 index）；耗尽一律
    // 映射为 undefined（IteratorStepValue 的 DONE 哨兵，§7.4.8）。
    if iterator.done {
        return value::encode_undefined();
    }
    let Some(result) = iterator.current else {
        return fail_dispatch(ctx);
    };
    if advance {
        state
            .array_iterators
            .get_mut(&handle)
            .expect("iterator entry was resolved above")
            .current = None;
    }
    result
}

/// 内建源的迭代预取：把规范 next()「推进并取值」的时序前移到 done 检查点
/// ——`current` 缓存预取结果、`index` 已指向下一位置，语义层 done/value/next
/// 三段式与真实 next() 的可观察状态因此一致：循环中途退出（break / body
/// 抛出 / 解构收尾）后实例位置停在已消费元素之后，后续对原型 `next` 的
/// 手动调用与 Node 一致地续走。Custom 源沿用 `ensure_custom_current`
/// （用户迭代器的 next() 本就在 done 检查点被调用）。
fn ensure_current(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator_handle: u32,
) -> Result<(), i64> {
    let Some(iterator) = state.array_iterators.get(&iterator_handle).copied() else {
        return Err(fail_dispatch(ctx));
    };
    if matches!(
        iterator.source,
        super::super::NativeIteratorSource::Custom(_)
    ) {
        return ensure_custom_current(ctx, state, iterator_handle);
    }
    if iterator.done || iterator.current.is_some() {
        return Ok(());
    }
    let exhausted = match iterator.source {
        super::super::NativeIteratorSource::Array(source) => {
            state
                .gc
                .heap()
                .array_length(source)
                .map_err(|_| fail_dispatch(ctx))?
                <= iterator.index
        }
        super::super::NativeIteratorSource::ArrayLike(source) => {
            iterator.index >= array_like_length(state, source).unwrap_or(0)
        }
        super::super::NativeIteratorSource::String(source) => state
            .string_owned(source)
            .is_none_or(|text| iterator.index as usize >= text.utf16_len()),
        super::super::NativeIteratorSource::TypedArray(source) => {
            state.typed_arrays.get(&source).is_none_or(|array| {
                iterator.index as usize >= super::typedarray::view_length(state, array).unwrap_or(0)
            })
        }
        super::super::NativeIteratorSource::Map(source) => state
            .maps
            .get(&source)
            .is_none_or(|entries| iterator.index as usize >= entries.len()),
        super::super::NativeIteratorSource::Set(source) => state
            .sets
            .get(&source)
            .is_none_or(|values| iterator.index as usize >= values.len()),
        super::super::NativeIteratorSource::Custom(_) => unreachable!("custom 源已提前分流"),
    };
    if exhausted {
        // 耗尽后 [[Done]] 粘住（§23.1.5.2.1 步骤 8.a 将 [[IteratedArrayLike]]
        // 置 undefined）：此后底层集合再增长也不复活。
        state
            .array_iterators
            .get_mut(&iterator_handle)
            .expect("iterator entry was resolved above")
            .done = true;
        return Ok(());
    }
    // `indexed` 标记索引族（数组/类数组/字符串/TypedArray）：kind 的 index
    // 包装只对它们生效；Map/Set 是键值集合（§24.1.5.1 CreateMapIterator），
    // kind 直接选择 key / value / entry，不做 index 包装。
    let (result, step, indexed) = match iterator.source {
        super::super::NativeIteratorSource::Array(source) => {
            let result = match state.gc.heap().get_element(source, iterator.index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => element as i64,
                Ok(_) => value::encode_undefined(),
                Err(_) => return Err(fail_dispatch(ctx)),
            };
            (result, 1, true)
        }
        super::super::NativeIteratorSource::ArrayLike(source) => {
            let Some(key) = state.intern_text(iterator.index.to_string(), value::TAG_STRING) else {
                return Err(fail_dispatch(ctx));
            };
            let object = value::encode_object_handle(source);
            let result = get_property(ctx, state, object, key).map_err(|()| fail_dispatch(ctx))?;
            // 属性读取抛出（getter）：不推进不缓存，异常直接传播。
            if value::is_exception(result) {
                return Err(result);
            }
            (result, 1, true)
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
                return Err(fail_dispatch(ctx));
            };
            let Some(units) =
                state.with_string_units(source, |units| units[index..index + width].to_vec())
            else {
                return Err(fail_dispatch(ctx));
            };
            let result = intern_string_with_gc_retry(
                ctx,
                state,
                wjsm_host::RuntimeString::from_utf16_units(units),
            );
            (result, width as u32, true)
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
                true,
            )
        }
        super::super::NativeIteratorSource::Map(source) => {
            let Some((key, stored)) = state
                .maps
                .get(&source)
                .and_then(|entries| entries.get(iterator.index as usize))
                .copied()
            else {
                return Err(fail_dispatch(ctx));
            };
            let result = match iterator.kind {
                super::super::NativeIteratorKind::Keys => key,
                super::super::NativeIteratorKind::Values => stored,
                super::super::NativeIteratorKind::Entries => state
                    .allocate_array_values_with_gc_retry(ctx, &[key, stored])
                    .map_err(|_| fail_dispatch(ctx))?,
            };
            (result, 1, false)
        }
        super::super::NativeIteratorSource::Set(source) => {
            let Some(stored) = state
                .sets
                .get(&source)
                .and_then(|values| values.get(iterator.index as usize))
                .copied()
            else {
                return Err(fail_dispatch(ctx));
            };
            let result = match iterator.kind {
                super::super::NativeIteratorKind::Keys
                | super::super::NativeIteratorKind::Values => stored,
                // Set entries 的 [v, v] 形态（§24.2.5.1 CreateSetIterator）。
                super::super::NativeIteratorKind::Entries => state
                    .allocate_array_values_with_gc_retry(ctx, &[stored, stored])
                    .map_err(|_| fail_dispatch(ctx))?,
            };
            (result, 1, false)
        }
        super::super::NativeIteratorSource::Custom(_) => unreachable!("custom 源已提前分流"),
    };
    let result = if indexed {
        match iterator.kind {
            super::super::NativeIteratorKind::Values => result,
            super::super::NativeIteratorKind::Keys => value::encode_f64(f64::from(iterator.index)),
            super::super::NativeIteratorKind::Entries => state
                .allocate_array_values_with_gc_retry(
                    ctx,
                    &[value::encode_f64(f64::from(iterator.index)), result],
                )
                .map_err(|_| fail_dispatch(ctx))?,
        }
    } else {
        result
    };
    let iterator = state
        .array_iterators
        .get_mut(&iterator_handle)
        .expect("iterator entry was resolved above");
    iterator.current = Some(result);
    iterator.index = iterator.index.saturating_add(step);
    Ok(())
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

/// 迭代步骤自身的 abrupt（next 抛出 / 结果非对象 / done 读取抛出）按
/// §7.4.7 IteratorStep / §7.4.8 IteratorStepValue 置 [[Done]] 为 true 后再
/// 传播：后续 IteratorClose（含语义层解构/for-of 的 abrupt 清理路径）据此
/// 跳过 return() 调用。
fn mark_custom_done(state: &mut NativeAgentState, iterator_handle: u32, exception: i64) -> i64 {
    if let Some(iterator) = state.array_iterators.get_mut(&iterator_handle) {
        iterator.done = true;
    }
    exception
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
    // next 为访问器且 getter 抛出：传播 getter 的异常而非误报 TypeError。
    if value::is_exception(next) {
        return Err(mark_custom_done(state, iterator_handle, next));
    }
    if !value::is_callable(next) {
        let exception = type_error(ctx, state, "iterator.next is not callable");
        return Err(mark_custom_done(state, iterator_handle, exception));
    }
    let result = state
        .invoke_callable(ctx, next, object, &[])
        .ok_or_else(|| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(mark_custom_done(state, iterator_handle, result));
    }
    if !value::is_js_object(result) {
        let exception = type_error(ctx, state, "iterator result is not an object");
        return Err(mark_custom_done(state, iterator_handle, exception));
    }
    let Some(done_key) = state.intern_text("done".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let done = get_property(ctx, state, result, done_key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(done) {
        return Err(mark_custom_done(state, iterator_handle, done));
    }
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

/// IteratorClose（ES §7.4.6）。规范所有调用点都以 iteratorRecord.[[Done]] 为
/// false 为前提；语义层为覆盖运行期才可知的 [[Done]] 状态（迭代器耗尽后 /
/// step 自身抛出后的 abrupt completion）无条件发射 close，此处以 `entry.done`
/// 复原该门禁——done 时不得调用 return()（可观察偏离）。
/// `completion_is_throw`：completion 为 throw completion 时（步骤 5）原始异常
/// 胜出，return 方法查找抛出、非 callable、调用抛出、返回非对象全部吞咽。
pub(super) fn iterator_close(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    completion_is_throw: bool,
) -> i64 {
    let [iterator, completion] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(*iterator) {
        return *iterator;
    }
    let handle = value::decode_handle(*iterator);
    let Some(entry) = state.array_iterators.get_mut(&handle) else {
        return *completion;
    };
    // 内建家族迭代器（数组/字符串/集合等）无 return 方法，close 是空操作；
    // 条目必须保留——实例在 for-of break 后仍可经原型 next 继续推进
    // （§23.1.5.2.1 对 [[IteratedArrayLike]] 的持续消费），死实例由
    // cleanup_retired_handles 随 GC 清理。预取值已被循环消费（bind 在
    // close 之前），清掉后位置正好停在已消费元素之后。
    let super::super::NativeIteratorSource::Custom(object) = entry.source else {
        entry.current = None;
        return *completion;
    };
    let entry = *entry;
    state.array_iterators.remove(&handle);
    if entry.done {
        return *completion;
    }
    let Some(return_key) = state.intern_text("return".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(method) = get_property(ctx, state, object, return_key) else {
        return fail_dispatch(ctx);
    };
    // GetMethod 的 abrupt（return 为访问器且 getter 抛出）：非 throw 完成时
    // 按步骤 6 传播，throw 完成时按步骤 5 吞咽。
    if value::is_exception(method) {
        return if completion_is_throw {
            *completion
        } else {
            method
        };
    }
    if value::is_undefined(method) || value::is_null(method) {
        return *completion;
    }
    if !value::is_callable(method) {
        if completion_is_throw {
            return *completion;
        }
        return type_error(ctx, state, "iterator.return is not callable");
    }
    let Some(result) = state.invoke_callable(ctx, method, object, &[]) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(result) {
        return if completion_is_throw {
            *completion
        } else {
            result
        };
    }
    if !value::is_js_object(result) {
        if completion_is_throw {
            return *completion;
        }
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
/// 循环体内闩锁 `GetProp` 可以跳过逐迭代 shape 检查直读模板槽偏移，且读出
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

fn env_layout_meta_for_function(state: &NativeAgentState, function_index: u32) -> (u32, u32) {
    use wjsm_backend_native::ENV_LAYOUT_META_WORDS;
    let base = usize::try_from(function_index)
        .ok()
        .and_then(|index| index.checked_mul(ENV_LAYOUT_META_WORDS));
    let Some(base) = base else {
        return (0, 0);
    };
    let shape = state.env_layout_meta.get(base).copied().unwrap_or(0);
    let count = state.env_layout_meta.get(base + 1).copied().unwrap_or(0);
    (shape, count)
}

fn current_native_function_index(state: &NativeAgentState) -> Option<u32> {
    state
        .activations
        .last()?
        .function
        .as_ref()
        .map(|function| function.function_index)
}

fn load_env_slot(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    env: i64,
    slot: i64,
    key: i64,
) -> Result<i64, ()> {
    let Ok(slot) = u32::try_from(slot) else {
        return Err(());
    };
    if let Some(function_index) = current_native_function_index(state) {
        let (expected_shape, slot_count) = env_layout_meta_for_function(state, function_index);
        if expected_shape != 0 && slot < slot_count {
            if let Some(handle) = object_handle(env) {
                if matches!(state.gc.heap().shape_id(handle), Ok(shape) if shape == expected_shape) {
                    if let Some(name_id) = property_key(state, key) {
                        if let Ok(Some((_, index))) =
                            state.gc.heap().own_data_property_index(handle, name_id)
                        {
                            if let Ok(stored) = state.gc.heap().value_slot(handle, index) {
                                return Ok(stored as i64);
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(exception) = get_on_nullish_base(ctx, state, env, key) {
        return Ok(exception);
    }
    get_property(ctx, state, env, key)
}

fn store_env_slot(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    env: i64,
    slot: i64,
    stored: i64,
    key: i64,
    strict: bool,
) -> Result<i64, ()> {
    let Ok(slot) = u32::try_from(slot) else {
        return Err(());
    };
    if let Some(function_index) = current_native_function_index(state) {
        let (expected_shape, slot_count) = env_layout_meta_for_function(state, function_index);
        if expected_shape != 0 && slot < slot_count {
            if let Some(handle) = object_handle(env) {
                if matches!(state.gc.heap().shape_id(handle), Ok(shape) if shape == expected_shape) {
                    if let Some(name_id) = property_key(state, key) {
                        if let Ok(Some((_, index))) =
                            state.gc.heap().own_data_property_index(handle, name_id)
                        {
                            if state
                                .gc
                                .heap()
                                .write_own_value_slot(handle, index, stored as u64)
                                .is_ok()
                            {
                                return Ok(stored);
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(result) = set_on_primitive_receiver(ctx, state, env, key, stored, strict) {
        return Ok(result);
    }
    let completion = set_property_completion(ctx, state, env, key, stored);
    Ok(property_write::finish_property_set(
        ctx, state, env, key, stored, strict, completion,
    ))
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
