use num_bigint::BigInt;
use num_traits::{FromPrimitive, Zero};
use wjsm_ir::{constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext, PendingExceptionKind};

use crate::{ASSIGNED_PROPERTY_FLAGS, NativeAgentState, NativeConstantMaterializeError};

pub(super) fn dispatch_runtime(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    operation: NativeRuntimeOp,
    args: &[i64],
) -> i64 {
    match operation {
        NativeRuntimeOp::CooperativePoll => {
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
            let result = usize::try_from(*slot)
                .ok()
                .and_then(|slot| state.variables.get(slot).copied())
                .unwrap_or_else(|| fail_dispatch(ctx));

            result
        }
        NativeRuntimeOp::IsTruthy => args
            .first()
            .map(|input| value::encode_bool(is_truthy(state, *input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        NativeRuntimeOp::MaterializeString
        | NativeRuntimeOp::MaterializeBigInt
        | NativeRuntimeOp::MaterializeRegExp => {
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
            let mut text = String::new();
            for part in args {
                match to_string_coerced(ctx, state, *part) {
                    Ok(part) => text.push_str(&part),
                    Err(exception) => return exception,
                }
            }
            state
                .intern_text(text, value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        NativeRuntimeOp::NewObject | NativeRuntimeOp::NewArray => {
            let [capacity] = args else {
                return fail_dispatch(ctx);
            };
            let Ok(capacity) = u32::try_from(*capacity) else {
                return fail_dispatch(ctx);
            };
            state
                .allocate_object(capacity, operation == NativeRuntimeOp::NewArray)
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        NativeRuntimeOp::GetProp => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            let result =
                get_property(ctx, state, *object, *key).unwrap_or_else(|()| fail_dispatch(ctx));
            if value::is_undefined(result)
                && std::env::var_os("WJSM_TRACE_UNDEFINED_PROP").is_some()
            {
                eprintln!(
                    "undefined property {} on {}",
                    render_value(state, *key),
                    render_value(state, *object)
                );
            }
            result
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
            if let Some(index) = array_index(state, *key) {
                if let Some(stored) = super::typedarray::get_element(state, *object, index as usize)
                {
                    return stored;
                }
                if value::is_array(*object) {
                    let handle = value::decode_handle(*object);
                    if state.heap.array_kind(handle).ok()
                        != Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
                    {
                        match state.heap.get_element(handle, index) {
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
        NativeRuntimeOp::SetProp => {
            let [object, key, stored] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_proxy(*object) {
                return super::proxy::set(ctx, state, *object, *key, *stored, *object);
            }
            if value::is_array(*object) && state.text_matches(*key, "length") {
                let Some(length) = array_length(state, *stored) else {
                    return range_error(ctx, state, "Invalid array length");
                };
                return state
                    .heap
                    .set_array_length(value::decode_handle(*object), length)
                    .map(|()| *stored)
                    .unwrap_or_else(|_| fail_dispatch(ctx));
            }
            if value::is_regexp(*object) && state.text_matches(*key, "lastIndex") {
                return super::regexp::set_last_index(ctx, state, &[*object, *stored]);
            }
            let Some(key) = property_key(state, *key) else {
                return fail_dispatch(ctx);
            };
            if value::is_array(*object) {
                let handle = value::decode_handle(*object);
                if let Some((_, setter, _)) = state.array_accessors.get(&(handle, key)).copied() {
                    if value::is_callable(setter) {
                        return state
                            .invoke_callable(ctx, setter, *object, &[*stored])
                            .map_or_else(|| fail_dispatch(ctx), |_| *stored);
                    }
                    return *stored;
                }
                state.note_array_property(handle, key);
                state.array_properties.insert((handle, key), *stored);
                state
                    .array_property_flags
                    .entry((handle, key))
                    .or_insert(ASSIGNED_PROPERTY_FLAGS);
                return *stored;
            }
            if value::is_callable(*object) {
                if let Some((_, setter)) = callable_accessor_on_chain(state, *object, key) {
                    if value::is_callable(setter) {
                        let result = state.invoke_callable(ctx, setter, *object, &[*stored]);
                        return result
                            .map(|_| *stored)
                            .unwrap_or_else(|| fail_dispatch(ctx));
                    }
                    return *stored;
                }
                state.callable_properties.insert((*object, key), *stored);
                state
                    .callable_property_flags
                    .entry((*object, key))
                    .or_insert(ASSIGNED_PROPERTY_FLAGS);
                return *stored;
            }
            let receiver = *object;
            match ordinary_set(
                ctx,
                state,
                receiver,
                encoded_property_key(key),
                *stored,
                receiver,
            ) {
                Ok(_) => *stored,
                Err(exception) => exception,
            }
        }
        NativeRuntimeOp::DeleteProp => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
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
            let result = super::object::dispatch_object(
                ctx,
                state,
                wjsm_ir::Builtin::ObjectSetPrototypeOf,
                args,
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                result
            } else {
                value::encode_undefined()
            }
        }
        NativeRuntimeOp::GetSuperBase => super_base(state).unwrap_or_else(value::encode_undefined),
        NativeRuntimeOp::GetSuperConstructor => {
            super_constructor(state).unwrap_or_else(value::encode_undefined)
        }
        NativeRuntimeOp::GetElem => {
            let [object, index] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_proxy(*object) {
                return super::proxy::get(ctx, state, *object, *index, *object);
            }
            if let Some(index) = array_index(state, *index) {
                if let Some(stored) = super::typedarray::get_element(state, *object, index as usize)
                {
                    return stored;
                }
            }
            if value::is_array(*object)
                && let Some(index) = array_index(state, *index)
            {
                let handle = value::decode_handle(*object);
                if state.heap.array_kind(handle).ok()
                    != Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
                {
                    match state.heap.get_element(handle, index) {
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
        NativeRuntimeOp::SetElem => {
            let [object, index, stored] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_proxy(*object) {
                return super::proxy::set(ctx, state, *object, *index, *stored, *object);
            }
            if let Some(array) = state
                .typed_arrays
                .get(&value::decode_handle(*object))
                .cloned()
            {
                if let Some(index) = array_index(state, *index) {
                    if index as usize >= array.length {
                        return *stored;
                    }
                    if array.kind.is_bigint() != value::is_bigint(*stored) {
                        return type_error(ctx, state, "Cannot convert value to a BigInt");
                    }
                    return super::typedarray::set_element(state, *object, index as usize, *stored)
                        .map_or_else(|| fail_dispatch(ctx), |_| *stored);
                }
                if value::is_f64(*index) {
                    return *stored;
                }
            }
            if value::is_array(*object)
                && let Some(index) = array_index(state, *index)
            {
                let handle = value::decode_handle(*object);
                if state.heap.array_kind(handle).ok()
                    == Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
                {
                    let Some(key) = property_key(state, value::encode_f64(f64::from(index))) else {
                        return fail_dispatch(ctx);
                    };
                    if let Some((_, setter, _)) = state.array_accessors.get(&(handle, key)).copied()
                    {
                        if value::is_callable(setter) {
                            return state
                                .invoke_callable(ctx, setter, *object, &[*stored])
                                .map_or_else(|| fail_dispatch(ctx), |_| *stored);
                        }
                        return *stored;
                    }
                    if state.array_properties.contains_key(&(handle, key)) {
                        if state
                            .array_property_flags
                            .get(&(handle, key))
                            .is_none_or(|flags| {
                                flags & wjsm_ir::constants::FLAG_WRITABLE as u32 != 0
                            })
                        {
                            state.array_properties.insert((handle, key), *stored);
                        }
                        return *stored;
                    }
                }
                return state
                    .heap
                    .set_element(handle, index, u64::from_ne_bytes(stored.to_ne_bytes()))
                    .map(|()| *stored)
                    .unwrap_or_else(|_| fail_dispatch(ctx));
            }
            let Some(key) = property_key(state, *index) else {
                return fail_dispatch(ctx);
            };
            if value::is_callable(*object) {
                if let Some((_, setter)) = state.callable_accessors.get(&(*object, key)).copied() {
                    if value::is_callable(setter) {
                        let result = state.invoke_callable(ctx, setter, *object, &[*stored]);
                        return result
                            .map(|_| *stored)
                            .unwrap_or_else(|| fail_dispatch(ctx));
                    }
                    return *stored;
                }
                state.callable_properties.insert((*object, key), *stored);
                state
                    .callable_property_flags
                    .entry((*object, key))
                    .or_insert(ASSIGNED_PROPERTY_FLAGS);
                return *stored;
            }
            if value::is_array(*object) {
                let handle = value::decode_handle(*object);
                if let Some((_, setter, _)) = state.array_accessors.get(&(handle, key)).copied() {
                    if value::is_callable(setter) {
                        let result = state.invoke_callable(ctx, setter, *object, &[*stored]);
                        return result.map_or_else(|| fail_dispatch(ctx), |_| *stored);
                    }
                    return *stored;
                }
                state.note_array_property(handle, key);
                state.array_properties.insert((handle, key), *stored);
                state
                    .array_property_flags
                    .entry((handle, key))
                    .or_insert(ASSIGNED_PROPERTY_FLAGS);
                return *stored;
            }
            match ordinary_set(
                ctx,
                state,
                *object,
                encoded_property_key(key),
                *stored,
                *object,
            ) {
                Ok(_) => *stored,
                Err(exception) => exception,
            }
        }
        NativeRuntimeOp::PrepareCall => state.prepare_call(ctx, args, false).unwrap_or_else(|| {
            state.prepare_rejected_call(
                ctx,
                args.first()
                    .copied()
                    .unwrap_or_else(value::encode_undefined),
                false,
            )
        }),
        NativeRuntimeOp::PrepareConstruct => {
            state.prepare_call(ctx, args, true).unwrap_or_else(|| {
                state.prepare_rejected_call(
                    ctx,
                    args.first()
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                    true,
                )
            })
        }
        NativeRuntimeOp::PrepareSuperCall | NativeRuntimeOp::PrepareSuperCallForward => state
            .prepare_super_call(
                ctx,
                args,
                operation == NativeRuntimeOp::PrepareSuperCallForward,
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
            .collect_rest_arguments(args)
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
            let left = match to_primitive(ctx, state, *left, PrimitiveHint::Default) {
                Ok(value) => value,
                Err(exception) => return exception,
            };
            let right = match to_primitive(ctx, state, *right, PrimitiveHint::Default) {
                Ok(value) => value,
                Err(exception) => return exception,
            };
            if value::is_string(left) || value::is_string(right) {
                let left = match to_string_coerced(ctx, state, left) {
                    Ok(text) => text,
                    Err(exception) => return exception,
                };
                let right = match to_string_coerced(ctx, state, right) {
                    Ok(text) => text,
                    Err(exception) => return exception,
                };
                state
                    .intern_text(format!("{left}{right}"), value::TAG_STRING)
                    .unwrap_or_else(|| fail_dispatch(ctx))
            } else if value::is_bigint(left) || value::is_bigint(right) {
                super::bigint::dispatch_bigint(
                    ctx,
                    state,
                    wjsm_ir::Builtin::BigIntAdd,
                    &[left, right],
                )
                .expect("BigIntAdd is handled")
            } else {
                binary_number(ctx, state, &[left, right], |left, right| left + right)
            }
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
        NativeRuntimeOp::UnaryNot => args
            .first()
            .map(|value| value::encode_bool(!is_truthy(state, *value)))
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
            let equal = strict_equal(state, *left, *right);
            value::encode_bool(if operation == NativeRuntimeOp::CompareStrictEq {
                equal
            } else {
                !equal
            })
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
    state.callable_prototypes.get(&constructor).copied()
}

fn super_base(state: &mut NativeAgentState) -> Option<i64> {
    let activation = state.activations.last()?;
    let Some(home_object) = activation.home_object else {
        let environment = activation.environment;
        let home_key = state.intern_text("home".into(), value::TAG_STRING)?;
        let home = state
            .heap
            .get_property(object_handle(environment)?, value::decode_handle(home_key))
            .ok()?? as i64;
        let prototype = state.heap.prototype(object_handle(home)?).ok()?;
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
        wjsm_ir::HomeObject::Constructor(_) => state.callable_prototypes.get(&constructor).copied(),
        wjsm_ir::HomeObject::Prototype(_) => {
            let prototype_key = state.intern_text("prototype".into(), value::TAG_STRING)?;
            let home = state.callable_property(constructor, value::decode_handle(prototype_key))?;
            let prototype = state.heap.prototype(value::decode_handle(home)).ok()?;
            Some(if prototype == u32::MAX {
                value::encode_null()
            } else {
                value::encode_object_handle(prototype)
            })
        }
    }
}

pub(super) fn object_handle(encoded: i64) -> Option<u32> {
    (value::is_object(encoded) || value::is_array(encoded)).then(|| value::decode_handle(encoded))
}

fn heap_prototype_value(state: &NativeAgentState, object: i64) -> Result<Option<i64>, ()> {
    let handle = object_handle(object).ok_or(())?;
    let prototype = state.heap.prototype(handle).map_err(|_| ())?;
    if prototype == wjsm_gc::PROTO_NULL_SENTINEL {
        return Ok(None);
    }
    if prototype & 0x8000_0000 != 0 {
        return Ok(Some(value::encode_proxy_handle(prototype & 0x7fff_ffff)));
    }
    let encoded =
        if state.heap.object_type(prototype).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
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
) -> Result<bool, i64> {
    let key = property_key(state, key).ok_or_else(|| fail_dispatch(ctx))?;
    ordinary_set_key(ctx, state, target, key, stored, receiver)
}

fn ordinary_set_key(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: u32,
    stored: i64,
    receiver: i64,
) -> Result<bool, i64> {
    let target_handle = object_handle(target).ok_or_else(|| fail_dispatch(ctx))?;
    let own = state
        .heap
        .get_property_slot(target_handle, key)
        .map_err(|_| fail_dispatch(ctx))?;
    if own.is_none() {
        let prototype = state
            .heap
            .prototype(target_handle)
            .map_err(|_| fail_dispatch(ctx))?;
        if prototype != wjsm_gc::PROTO_NULL_SENTINEL {
            if prototype & 0x8000_0000 != 0 {
                let result = super::proxy::set(
                    ctx,
                    state,
                    value::encode_proxy_handle(prototype & 0x7fff_ffff),
                    encoded_property_key(key),
                    stored,
                    receiver,
                );
                return if value::is_exception(result) {
                    Err(result)
                } else {
                    Ok(true)
                };
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
                return Ok(false);
            }
            let result = state
                .invoke_callable(ctx, setter, receiver, &[stored])
                .ok_or_else(|| fail_dispatch(ctx))?;
            return if value::is_exception(result) {
                Err(result)
            } else {
                Ok(true)
            };
        }
        if descriptor.flags & constants::FLAG_WRITABLE as u32 == 0 {
            return Ok(false);
        }
    }
    if value::is_proxy(receiver) {
        return super::proxy::set_receiver_value(
            ctx,
            state,
            receiver,
            encoded_property_key(key),
            stored,
        );
    }
    let receiver_handle = object_handle(receiver).ok_or_else(|| fail_dispatch(ctx))?;
    if let Some(receiver_descriptor) = state
        .heap
        .get_property_slot(receiver_handle, key)
        .map_err(|_| fail_dispatch(ctx))?
    {
        if receiver_descriptor.flags & constants::FLAG_IS_ACCESSOR as u32 != 0
            || receiver_descriptor.flags & constants::FLAG_WRITABLE as u32 == 0
        {
            return Ok(false);
        }
    } else if state.non_extensible_objects.contains(&receiver_handle) {
        return Ok(false);
    }
    state
        .heap
        .set_property(receiver_handle, key, stored as u64)
        .map_err(|_| fail_dispatch(ctx))?;
    Ok(true)
}

pub(super) const SYMBOL_PROPERTY_KEY_BIT: u32 = 1 << 31;

pub(super) fn property_key(state: &mut NativeAgentState, encoded: i64) -> Option<u32> {
    if value::is_string(encoded) {
        let text = state.string(encoded)?.clone();
        state
            .intern_runtime_string(text, value::TAG_STRING)
            .map(value::decode_handle)
    } else if value::is_symbol(encoded) {
        Some(value::decode_handle(encoded) | SYMBOL_PROPERTY_KEY_BIT)
    } else {
        let key = state.intern_text(render_value(state, encoded), value::TAG_STRING)?;
        Some(value::decode_handle(key))
    }
}

pub(super) fn encoded_property_key(key: u32) -> i64 {
    if key & SYMBOL_PROPERTY_KEY_BIT == 0 {
        value::encode_handle(value::TAG_STRING, key)
    } else {
        value::encode_handle(value::TAG_SYMBOL, key & !SYMBOL_PROPERTY_KEY_BIT)
    }
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
    if value::is_string(object)
        && let Some(index) = array_index(state, key)
    {
        let unit = state
            .string(object)
            .and_then(|text| text.as_utf16_units().get(index as usize).copied());
        return Ok(unit
            .and_then(|unit| {
                state.intern_runtime_string(
                    wjsm_host::RuntimeString::from_utf16_units(vec![unit]),
                    value::TAG_STRING,
                )
            })
            .unwrap_or_else(value::encode_undefined));
    }
    if state
        .typed_arrays
        .contains_key(&value::decode_handle(object))
    {
        let property_name = state
            .string(key)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
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
            .string(key)
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
            .string(key)
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
        return Ok(super::regexp::get_property(ctx, state, object, key)
            .unwrap_or_else(value::encode_undefined));
    }
    if value::is_string(object) && state.text_matches(key, "length") {
        return state
            .string(object)
            .map(|text| value::encode_f64(text.utf16_len() as f64))
            .ok_or(());
    }
    if value::is_array(object) && state.text_matches(key, "length") {
        return state
            .heap
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
            match state.heap.get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => {
                    return Ok(element as i64);
                }
                Ok(_) => {}
                Err(_) => return Err(()),
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
        if let Some((getter, _)) = callable_accessor_on_chain(state, object, key) {
            return if value::is_callable(getter) {
                state.invoke_callable(ctx, getter, receiver, &[]).ok_or(())
            } else {
                Ok(value::encode_undefined())
            };
        }
        return Ok(state
            .callable_property(object, key)
            .or_else(|| state.primitive_property(object, encoded_key))
            .unwrap_or_else(value::encode_undefined));
    }
    let handle = object_handle(object).ok_or(())?;
    match state.heap.get_property_slot_on_proto_chain(handle, key) {
        Ok(Some(property)) if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 => {
            let getter = property.getter as i64;
            if value::is_callable(getter) {
                state.invoke_callable(ctx, getter, receiver, &[]).ok_or(())
            } else {
                Ok(value::encode_undefined())
            }
        }
        Ok(Some(property)) => Ok(property.value as i64),
        Ok(None) => Ok(state
            .primitive_property(object, encoded_key)
            .unwrap_or_else(value::encode_undefined)),
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

fn callable_accessor_on_chain(
    state: &NativeAgentState,
    callable: i64,
    key: u32,
) -> Option<(i64, i64)> {
    let mut current = Some(callable);
    while let Some(candidate) = current {
        if let Some(accessor) = state.callable_accessors.get(&(candidate, key)).copied() {
            return Some(accessor);
        }
        current = state
            .callable_prototypes
            .get(&candidate)
            .copied()
            .filter(|prototype| value::is_callable(*prototype));
    }
    None
}

pub(super) fn delete_property(
    state: &mut NativeAgentState,
    object: i64,
    encoded_key: i64,
) -> Result<bool, ()> {
    let key = property_key(state, encoded_key).ok_or(())?;
    let configurable = constants::FLAG_CONFIGURABLE as u32;
    if value::is_callable(object) {
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
            let length = state.heap.array_length(handle).map_err(|_| ())?;
            if index < length {
                state
                    .heap
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
        .heap
        .get_property_slot(handle, key)
        .map_err(|_| ())?
        .is_some_and(|property| property.flags & configurable == 0)
    {
        return Ok(false);
    }
    state.heap.delete_property(handle, key).map_err(|_| ())
}
pub(super) fn has_property(state: &mut NativeAgentState, object: i64, encoded_key: i64) -> bool {
    if value::is_array(object) && state.text_matches(encoded_key, "length") {
        return true;
    }
    if value::is_array(object) {
        let handle = value::decode_handle(object);
        if let Some(index) = array_index(state, encoded_key) {
            match state.heap.get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        let Some(key) = property_key(state, encoded_key) else {
            return false;
        };
        return state.array_properties.contains_key(&(handle, key))
            || state.array_accessors.contains_key(&(handle, key))
            || state.primitive_property(object, encoded_key).is_some();
    }
    let Some(key) = property_key(state, encoded_key) else {
        return false;
    };
    if value::is_callable(object) {
        return state.callable_accessors.contains_key(&(object, key))
            || state.callable_property(object, key).is_some()
            || state.primitive_property(object, encoded_key).is_some();
    }
    let Some(handle) = object_handle(object) else {
        return false;
    };
    state
        .heap
        .get_property_slot_on_proto_chain(handle, key)
        .ok()
        .flatten()
        .is_some()
        || state.primitive_property(object, encoded_key).is_some()
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
        wjsm_ir::Builtin::IteratorFrom if value::is_string(source) => Some((
            super::super::NativeIteratorSource::String(source),
            super::super::NativeIteratorKind::Values,
        )),
        wjsm_ir::Builtin::IteratorFrom
            if value::is_js_object(source)
                && state
                    .heap
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
        let Ok(iterator) = state.allocate_object(0, false) else {
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
    let Ok(iterator) = state.allocate_object(0, false) else {
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
            .heap
            .array_length(source)
            .is_ok_and(|length| iterator.index >= length),
        super::super::NativeIteratorSource::ArrayLike(source) => {
            iterator.index >= array_like_length(state, source).unwrap_or(0)
        }
        super::super::NativeIteratorSource::String(source) => state
            .string(source)
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
            let result = match state.heap.get_element(source, iterator.index) {
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
            let Some(text) = state.string(source) else {
                return fail_dispatch(ctx);
            };
            let index = iterator.index as usize;
            let Some(code_point) = text.code_point_at(index) else {
                return value::encode_undefined();
            };
            let width = usize::from(code_point > 0xffff) + 1;
            let units = text.slice_units(index..index + width);
            let Some(result) = state.intern_runtime_string(units, value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            (result, width as u32)
        }
        super::super::NativeIteratorSource::TypedArray(source) => (
            super::typedarray::get_element(
                state,
                value::encode_object_handle(source),
                iterator.index as usize,
            )
            .unwrap_or_else(value::encode_undefined),
            1,
        ),
        super::super::NativeIteratorSource::Map(source) => {
            let Some((key, stored)) = state
                .maps
                .get(&source)
                .and_then(|entries| entries.get(iterator.index as usize))
                .copied()
            else {
                return value::encode_undefined();
            };
            let Ok(entry) = state.allocate_array_values(&[key, stored]) else {
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
            let Ok(entry) = state
                .allocate_array_values(&[value::encode_f64(f64::from(iterator.index)), result])
            else {
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
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    for (name, stored) in [("value", result), ("done", value::encode_bool(done))] {
        let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if state
            .heap
            .set_property(
                value::decode_handle(object),
                value::decode_handle(key),
                stored as u64,
            )
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
    let key = state.intern_text("length".into(), value::TAG_STRING)?;
    let stored = state
        .heap
        .get_property(source, value::decode_handle(key))
        .ok()
        .flatten()? as i64;
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

pub(super) fn range_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    named_error(ctx, state, "RangeError", message)
}

pub(super) fn array_index(state: &NativeAgentState, encoded: i64) -> Option<u32> {
    let text = if value::is_string(encoded) || value::is_bigint(encoded) {
        state.string(encoded)?.to_utf8()?
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

fn array_length(state: &NativeAgentState, encoded: i64) -> Option<u32> {
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
        let text = state.string(encoded)?.to_utf8_lossy();
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
        state.string(encoded).is_some_and(|text| !text.is_empty())
    } else {
        true
    }
}

pub(super) fn strict_equal(state: &NativeAgentState, left: i64, right: i64) -> bool {
    if value::is_f64(left) && value::is_f64(right) {
        value::decode_f64(left) == value::decode_f64(right)
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
            .string(right)
            .and_then(wjsm_host::RuntimeString::to_utf8)
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

pub(super) fn binary_f64(args: &[i64], operation: impl FnOnce(f64, f64) -> f64) -> i64 {
    match args {
        [left, right] => value::encode_f64(operation(
            value::decode_f64(*left),
            value::decode_f64(*right),
        )),
        _ => value::encode_handle(value::TAG_EXCEPTION, 0),
    }
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
            .string(encoded)
            .map(wjsm_host::RuntimeString::to_utf8_lossy)
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
            .and_then(|name| state.string(name))
            .map(wjsm_host::RuntimeString::to_utf8_lossy)
            .unwrap_or_else(|| "Error".into());
        let message = state
            .property_value_by_name(encoded, "message")
            .and_then(|message| state.string(message))
            .map(wjsm_host::RuntimeString::to_utf8_lossy)
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
        let Ok(length) = state.heap.array_length(handle) else {
            return "[array]".into();
        };
        let mut elements = Vec::with_capacity(length as usize);
        for index in 0..length {
            let rendered = match state.heap.get_element(handle, index) {
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
