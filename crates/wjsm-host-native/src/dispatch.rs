pub(crate) mod agent;
pub(crate) mod arguments;
mod array;
mod array_callbacks;
mod array_sort;
pub(crate) mod async_generator;
pub(crate) mod atomics;
mod bigint;
pub(crate) mod buffers;
pub(crate) mod collections;
pub(crate) mod date;
mod date_methods;
pub(crate) mod enumerator;
pub(crate) mod fetch;
mod function;
pub(crate) mod generator;
mod json;
mod math;
pub(crate) mod modules;
pub(crate) mod node_async_hooks;
pub(crate) mod node_buffer;
pub(crate) mod node_child_process;
pub(crate) mod node_crypto;
pub(crate) mod node_dgram;
pub(crate) mod node_fs;
pub(crate) mod node_net;
pub(crate) mod node_os;
pub(crate) mod node_perf_hooks;
pub(crate) mod node_tls;
pub(crate) mod node_vm;
pub(crate) mod node_worker_threads;
pub(crate) mod node_zlib;
mod object;
mod primitive;
mod private;
pub(crate) mod promise;
pub(crate) mod proxy;
pub(crate) mod regexp;
mod runtime;
pub(crate) mod sab;
pub(crate) mod streams;
mod string;
pub(crate) mod structured_clone;
mod symbol;
pub(crate) mod typedarray;
pub(crate) mod weak;
pub(crate) mod web_encoding;
use std::cmp::Ordering;

use num_bigint::BigInt;
use num_traits::FromPrimitive;

pub(crate) use self::array::construct as construct_array;
pub(crate) use self::object::construct_object;
pub(crate) use self::runtime::SYMBOL_PROPERTY_KEY_BIT;
pub(crate) use self::runtime::to_number as number_value;
use self::runtime::{
    PrimitiveHint, abstract_equal, binary_f64, dispatch_runtime, get_property, has_property,
    iterator_close, iterator_done, iterator_from, iterator_next, iterator_value, object_handle,
    property_key, strict_equal, to_number, to_primitive,
};
pub(crate) use self::runtime::{
    array_iterator, array_to_string, error_to_string, fail_dispatch, iterator_next_result,
    render_value, to_string_coerced,
};
pub(crate) use self::symbol::well_known_description;
use crate::NativeAgentState;
use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext, PendingExceptionKind};

pub(crate) fn store_bigint(state: &mut NativeAgentState, input: BigInt) -> Option<i64> {
    bigint::store(state, input)
}

pub(super) unsafe extern "C" fn native_host_operation(
    ctx: *mut NativeVmContext,
    operation: u32,
    args: *const i64,
    args_count: u32,
) -> i64 {
    // SAFETY: generated code passes its pinned vmctx pointer; dispatcher checks null before use and
    // does not retain the reference beyond this synchronous call.
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let count = match usize::try_from(args_count) {
        Ok(count) => count,
        Err(_) => return fail_dispatch(ctx),
    };
    let args = if count == 0 {
        &[]
    } else {
        if args.is_null() {
            return fail_dispatch(ctx);
        }
        // SAFETY: compiler creates a stack slot containing exactly `args_count` initialized i64
        // values and keeps it live for this synchronous dispatcher call.
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    // SAFETY: heap_state is initialized from the boxed owner state and remains valid/pinned for the
    // runtime lifetime; host thunks run only synchronously on the owner thread.
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };

    if operation <= u32::from(Builtin::last_wire_id()) {
        let builtin_id = match u16::try_from(operation) {
            Ok(id) => id,
            Err(_) => return fail_dispatch(ctx),
        };
        let Some(builtin) = Builtin::from_wire_id(builtin_id) else {
            return fail_dispatch(ctx);
        };
        return dispatch_builtin(ctx, state, builtin, args);
    }
    let Some(operation) = NativeRuntimeOp::from_id(operation) else {
        return fail_dispatch(ctx);
    };
    dispatch_runtime(ctx, state, operation, args)
}
fn rejected_call_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callee: i64,
    construct: bool,
) -> i64 {
    if ctx.pending_exception_kind == PendingExceptionKind::StackOverflow {
        ctx.pending_exception_kind = PendingExceptionKind::None;
        let message = "Maximum call stack size exceeded".to_owned();
        let Some(error) = modules::named_error_object(state, "RangeError", message.clone()) else {
            return fail_dispatch(ctx);
        };
        if let Some(trace) = state.pending_stack_trace.take() {
            let stack = format!("RangeError: {message}{trace}");
            if modules::set_error_stack(state, error, stack).is_none() {
                return fail_dispatch(ctx);
            }
        }
        return state
            .create_exception(error)
            .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let message = if value::is_proxy(callee) {
        if construct {
            "Proxy target must be a constructor".to_owned()
        } else {
            "Proxy target must be callable".to_owned()
        }
    } else if construct {
        format!(
            "{} is not a constructor",
            runtime::render_value(state, callee)
        )
    } else {
        format!("{} is not a function", runtime::render_value(state, callee))
    };
    modules::named_error_object(state, "TypeError", message)
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) unsafe extern "C" fn native_rejected_call(
    ctx: *mut NativeVmContext,
    callee: i64,
    _this_value: i64,
    _args_base: u32,
    _args_count: u32,
) -> i64 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    rejected_call_error(ctx, state, callee, false)
}

pub(super) unsafe extern "C" fn native_rejected_construct(
    ctx: *mut NativeVmContext,
    callee: i64,
    _this_value: i64,
    _args_base: u32,
    _args_count: u32,
) -> i64 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    rejected_call_error(ctx, state, callee, true)
}

pub(super) fn dispatch_builtin(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    if let Some(result) = modules::dispatch_module(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = bigint::dispatch_bigint(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = typedarray::dispatch_typed_array(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = promise::dispatch_promise(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = async_generator::dispatch_async_generator(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = generator::dispatch_generator(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = streams::dispatch_streams(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = fetch::dispatch_fetch(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = buffers::dispatch_buffer(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = sab::dispatch_sab(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = atomics::dispatch_atomics(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = enumerator::dispatch_enumerator(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = collections::dispatch_collection(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = array::dispatch_array(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = function::dispatch_function(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = array_callbacks::dispatch_array_callback(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = json::dispatch_json(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = date::dispatch_date(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = math::dispatch_math(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = object::dispatch_object(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = private::dispatch_private(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = regexp::dispatch_regexp(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = proxy::dispatch_proxy(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = primitive::dispatch_primitive(ctx, state, builtin, args) {
        return result;
    }
    if matches!(
        builtin,
        Builtin::CreateMappedArgumentsObject | Builtin::CreateUnmappedArgumentsObject
    ) {
        return arguments::create(
            ctx,
            state,
            builtin == Builtin::CreateMappedArgumentsObject,
            args,
        );
    }
    if let Some(result) = symbol::dispatch_symbol(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = string::dispatch_string(ctx, state, builtin, args) {
        return result;
    }
    if let Some(result) = weak::dispatch_weak(ctx, state, builtin, args) {
        return result;
    }
    if builtin == Builtin::PerformanceNow {
        return node_perf_hooks::performance_now(state);
    }
    match builtin {
        Builtin::ScopeRecordCreate => {
            modules::create_scope_record(state).unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ScopeRecordAddBinding => {
            let [record, key, stored, initialized, constant] = args else {
                return fail_dispatch(ctx);
            };
            value::encode_bool(modules::scope_record_add(
                state,
                *record,
                *key,
                *stored,
                *initialized,
                *constant,
            ))
        }
        Builtin::ScopeRecordSetMeta => {
            let [record, key, stored] = args else {
                return fail_dispatch(ctx);
            };
            if modules::scope_record_set_meta(state, *record, *key, *stored) {
                value::encode_undefined()
            } else {
                fail_dispatch(ctx)
            }
        }
        Builtin::ScopeRecordDestroy => {
            if let Some(record) = args.first() {
                modules::destroy_scope_record(state, *record);
            }
            value::encode_undefined()
        }
        Builtin::StructuredClone => structured_clone::structured_clone(ctx, state, args),
        Builtin::EvalIndirect => {
            let [code] = args else {
                return fail_dispatch(ctx);
            };
            let global = node_vm::current_context(state);
            if !node_vm::strings_enabled(state, global) {
                return modules::named_error_object(
                    state,
                    "EvalError",
                    "Code generation from strings disallowed for this context".into(),
                )
                .and_then(|error| state.create_exception(error))
                .unwrap_or_else(|| fail_dispatch(ctx));
            }
            let Some(source) = state
                .string(*code)
                .and_then(wjsm_host::RuntimeString::to_utf8)
            else {
                return *code;
            };
            let Some(environment) = modules::create_scope_record_with_outer(state, global) else {
                return fail_dispatch(ctx);
            };
            let result = modules::execute_eval_script(
                ctx,
                state,
                &source,
                environment,
                global,
                "eval:indirect",
            );
            modules::destroy_scope_record(state, environment);
            eval_execution_result(ctx, state, result)
        }
        Builtin::Eval => {
            let [code, environment] = args else {
                return fail_dispatch(ctx);
            };
            let global = node_vm::current_context(state);
            if !node_vm::strings_enabled(state, global) {
                return modules::named_error_object(
                    state,
                    "EvalError",
                    "Code generation from strings disallowed for this context".into(),
                )
                .and_then(|error| state.create_exception(error))
                .unwrap_or_else(|| fail_dispatch(ctx));
            }
            let Some(source) = state
                .string(*code)
                .and_then(wjsm_host::RuntimeString::to_utf8)
            else {
                return *code;
            };
            match modules::execute_eval_script(
                ctx,
                state,
                &source,
                *environment,
                global,
                "eval:dynamic",
            ) {
                result => eval_execution_result(ctx, state, result),
            }
        }
        Builtin::EvalGetBinding => eval_get_binding(ctx, state, args),
        Builtin::EvalSetBinding => eval_set_binding(ctx, state, args),
        Builtin::EvalHasBinding => {
            let [environment, key] = args else {
                return fail_dispatch(ctx);
            };
            value::encode_bool(eval_binding_exists(ctx, state, *environment, *key))
        }
        Builtin::EvalSuperBase => {
            let [environment] = args else {
                return fail_dispatch(ctx);
            };
            modules::scope_record_super_base(state, *environment)
                .unwrap_or_else(value::encode_undefined)
        }

        Builtin::ErrorConstructor
        | Builtin::EvalErrorConstructor
        | Builtin::RangeErrorConstructor
        | Builtin::ReferenceErrorConstructor
        | Builtin::SyntaxErrorConstructor
        | Builtin::TypeErrorConstructor
        | Builtin::URIErrorConstructor => {
            error_constructor(ctx, state, builtin, value::encode_undefined(), args)
        }
        Builtin::ConsoleLog
        | Builtin::ConsoleInfo
        | Builtin::ConsoleDebug
        | Builtin::ConsoleWarn
        | Builtin::ConsoleError
        | Builtin::ConsoleTrace => {
            let mut output = state.output.borrow_mut();
            match builtin {
                Builtin::ConsoleInfo => output.extend_from_slice(b"[info] "),
                Builtin::ConsoleDebug => output.extend_from_slice(b"[debug] "),
                Builtin::ConsoleWarn => output.extend_from_slice(b"[warn] "),
                Builtin::ConsoleError => output.extend_from_slice(b"[error] "),
                Builtin::ConsoleTrace => output.extend_from_slice(b"[trace] "),
                Builtin::ConsoleLog => {}
                _ => unreachable!("console builtin match is exhaustive"),
            }
            for (index, argument) in args.iter().enumerate() {
                if index != 0 {
                    output.push(b' ');
                }
                output.extend_from_slice(render_value(state, *argument).as_bytes());
            }
            output.push(b'\n');
            value::encode_undefined()
        }
        Builtin::AbstractCompare => {
            let [left, right, reverse, invert] = args else {
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
            let comparison = if value::decode_bool(*reverse) {
                abstract_compare(state, right, left)
            } else {
                abstract_compare(state, left, right)
            };
            let result = if value::decode_bool(*invert) {
                comparison.is_some_and(|ordering| ordering != Ordering::Less)
            } else {
                comparison == Some(Ordering::Less)
            };
            value::encode_bool(result)
        }
        Builtin::AbstractEq => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            match abstract_equal(ctx, state, *left, *right) {
                Ok(equal) => value::encode_bool(equal),
                Err(exception) => exception,
            }
        }
        Builtin::StrictEq => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            value::encode_bool(strict_equal(state, *left, *right))
        }
        Builtin::TypeOf => {
            let Some(input) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let name = if value::is_undefined(input) {
                "undefined"
            } else if value::is_bool(input) {
                "boolean"
            } else if value::is_string(input) {
                "string"
            } else if value::is_callable(input) {
                "function"
            } else if value::is_bigint(input) {
                "bigint"
            } else if value::is_symbol(input) {
                "symbol"
            } else if value::is_null(input) || value::is_js_object(input) || value::is_regexp(input)
            {
                "object"
            } else {
                "number"
            };
            state
                .intern_text(name.into(), value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ObjectCreate => {
            let Some(prototype) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let prototype = if value::is_null(prototype) {
                u32::MAX
            } else if let Some(prototype) = object_handle(prototype) {
                prototype
            } else {
                return fail_dispatch(ctx);
            };
            match state.allocate_object(0, false) {
                Ok(object) => state
                    .heap
                    .set_prototype(value::decode_handle(object), prototype)
                    .map(|()| object)
                    .unwrap_or_else(|_| fail_dispatch(ctx)),
                Err(_) => fail_dispatch(ctx),
            }
        }
        Builtin::ObjectSetPrototypeOf => {
            let [object, prototype] = args else {
                return fail_dispatch(ctx);
            };
            let Some(target_handle) = object_handle(*object) else {
                return fail_dispatch(ctx);
            };
            let prototype = if value::is_null(*prototype) {
                u32::MAX
            } else if let Some(prototype) = object_handle(*prototype) {
                prototype
            } else {
                return fail_dispatch(ctx);
            };
            state
                .heap
                .set_prototype(target_handle, prototype)
                .map(|()| *object)
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::ObjectKeys
        | Builtin::ObjectValues
        | Builtin::ObjectEntries
        | Builtin::ObjectGetOwnPropertyNames => {
            let Some(object) = args.first().and_then(|object| object_handle(*object)) else {
                return fail_dispatch(ctx);
            };
            let Ok(properties) = state.heap.own_property_slots(object) else {
                return fail_dispatch(ctx);
            };
            let mut results = Vec::with_capacity(properties.len());
            for (key, flags) in properties {
                if builtin != Builtin::ObjectGetOwnPropertyNames
                    && flags & constants::FLAG_ENUMERABLE as u32 == 0
                {
                    continue;
                }
                let Ok(Some(stored)) = state.heap.get_property(object, key) else {
                    return fail_dispatch(ctx);
                };
                let key = value::encode_handle(value::TAG_STRING, key);
                match builtin {
                    Builtin::ObjectKeys | Builtin::ObjectGetOwnPropertyNames => results.push(key),
                    Builtin::ObjectValues => results.push(stored as i64),
                    Builtin::ObjectEntries => {
                        let pair = [key, stored as i64];
                        match state.allocate_array_values(&pair) {
                            Ok(pair) => results.push(pair),
                            Err(_) => return fail_dispatch(ctx),
                        }
                    }
                    _ => unreachable!("guard restricts Object enumeration builtins"),
                }
            }
            state
                .allocate_array_values(&results)
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::ObjectAssign => {
            let Some(target) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let Some(target_handle) = object_handle(target) else {
                return fail_dispatch(ctx);
            };
            for source in &args[1..] {
                if value::is_null(*source) || value::is_undefined(*source) {
                    continue;
                }
                let Some(source_handle) = object_handle(*source) else {
                    return fail_dispatch(ctx);
                };
                let Ok(properties) = state.heap.own_property_slots(source_handle) else {
                    return fail_dispatch(ctx);
                };
                for (key, flags) in properties {
                    if flags & constants::FLAG_ENUMERABLE as u32 == 0 {
                        continue;
                    }
                    let Ok(Some(stored)) = state.heap.get_property(source_handle, key) else {
                        return fail_dispatch(ctx);
                    };
                    if state.heap.set_property(target_handle, key, stored).is_err() {
                        return fail_dispatch(ctx);
                    }
                }
            }
            target
        }
        Builtin::ObjectGetPrototypeOf => {
            let Some(object) = args.first().and_then(|object| object_handle(*object)) else {
                return fail_dispatch(ctx);
            };
            state
                .heap
                .prototype(object)
                .map(|prototype| {
                    if prototype == u32::MAX {
                        value::encode_null()
                    } else {
                        value::encode_handle(value::TAG_OBJECT, prototype)
                    }
                })
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::InstanceOf => {
            let [object, constructor] = args else {
                return fail_dispatch(ctx);
            };
            let has_instance_key =
                value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::HAS_INSTANCE);
            let method = match get_property(ctx, state, *constructor, has_instance_key) {
                Ok(method) => method,
                Err(()) => {
                    return runtime::type_error(
                        ctx,
                        state,
                        "Right-hand side of instanceof is not an object",
                    );
                }
            };
            if value::is_exception(method) {
                return method;
            }
            if !value::is_undefined(method) {
                if !value::is_callable(method) {
                    return runtime::type_error(
                        ctx,
                        state,
                        "Symbol.hasInstance method is not callable",
                    );
                }
                let result = state
                    .invoke_callable(ctx, method, *constructor, &[*object])
                    .unwrap_or_else(|| fail_dispatch(ctx));
                if value::is_exception(result) {
                    return result;
                }
                return value::encode_bool(runtime::is_truthy(state, result));
            }
            if !state.is_callable_value(*constructor) {
                return runtime::type_error(
                    ctx,
                    state,
                    "Right-hand side of instanceof is not callable",
                );
            }
            let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING)
            else {
                return fail_dispatch(ctx);
            };
            let prototype = match get_property(ctx, state, *constructor, prototype_key) {
                Ok(prototype) => prototype,
                Err(()) => return fail_dispatch(ctx),
            };
            if value::is_exception(prototype) {
                return prototype;
            }
            if !(value::is_object(prototype)
                || value::is_array(prototype)
                || value::is_callable(prototype)
                || value::is_proxy(prototype))
            {
                return runtime::type_error(
                    ctx,
                    state,
                    "Function has non-object prototype in instanceof check",
                );
            }
            value::encode_bool(state.prototype_chain_contains_value(*object, prototype))
        }
        Builtin::GetPrototypeFromConstructor => {
            let Some(constructor) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let Some(prototype_key) = state
                .intern_text("prototype".into(), value::TAG_STRING)
                .map(value::decode_handle)
            else {
                return fail_dispatch(ctx);
            };
            state
                .callable_property(constructor, prototype_key)
                .filter(|prototype| value::is_js_object(*prototype))
                .unwrap_or_else(value::encode_null)
        }
        Builtin::IsCallable => args
            .first()
            .map(|input| value::encode_bool(value::is_callable(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::IsJsObject => args
            .first()
            .map(|input| value::encode_bool(value::is_js_object(*input)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::CreateClosure => state
            .create_closure(args)
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::ObjectIs => {
            let [left, right] = args else {
                return fail_dispatch(ctx);
            };
            let equal = if value::is_f64(*left) && value::is_f64(*right) {
                let left = value::decode_f64(*left);
                let right = value::decode_f64(*right);
                (left.is_nan() && right.is_nan()) || left.to_bits() == right.to_bits()
            } else {
                left == right
            };
            value::encode_bool(equal)
        }
        Builtin::ObjectProtoValueOf => args.first().copied().unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::ObjectProtoToString => {
            let Some(input) = args.first().copied() else {
                return fail_dispatch(ctx);
            };
            let tag_input = if value::is_proxy(input) {
                state
                    .proxies
                    .get(usize::try_from(value::decode_proxy_handle(input)).unwrap_or(usize::MAX))
                    .and_then(|proxy| proxy.as_ref())
                    .map_or(input, |proxy| proxy.target)
            } else {
                input
            };
            let default_tag = if value::is_undefined(tag_input) {
                "Undefined"
            } else if value::is_null(tag_input) {
                "Null"
            } else if value::is_bool(tag_input) {
                "Boolean"
            } else if value::is_f64(tag_input) {
                "Number"
            } else if value::is_string(tag_input) {
                "String"
            } else if value::is_bigint(tag_input) {
                "BigInt"
            } else if value::is_symbol(tag_input) {
                "Symbol"
            } else if value::is_array(tag_input) {
                "Array"
            } else if value::is_callable(tag_input) {
                "Function"
            } else if value::is_regexp(tag_input) {
                "RegExp"
            } else if value::is_js_object(tag_input)
                && state
                    .heap
                    .object_type(value::decode_handle(tag_input))
                    .is_ok_and(|kind| kind == u32::from(wjsm_ir::HEAP_TYPE_ARGUMENTS))
            {
                "Arguments"
            } else if value::is_js_object(tag_input)
                && state
                    .error_objects
                    .contains(&value::decode_handle(tag_input))
            {
                "Error"
            } else if let Some(primitive) = value::is_js_object(tag_input)
                .then(|| value::decode_handle(tag_input))
                .and_then(|handle| state.boxed_primitives.get(&handle))
            {
                if value::is_bool(*primitive) {
                    "Boolean"
                } else if value::is_f64(*primitive) {
                    "Number"
                } else if value::is_string(*primitive) {
                    "String"
                } else if value::is_bigint(*primitive) {
                    "BigInt"
                } else if value::is_symbol(*primitive) {
                    "Symbol"
                } else {
                    "Object"
                }
            } else {
                "Object"
            };
            let tag = if value::is_null(input) || value::is_undefined(input) {
                default_tag.to_owned()
            } else {
                let key =
                    value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::TO_STRING_TAG);
                let custom =
                    get_property(ctx, state, input, key).unwrap_or_else(|()| fail_dispatch(ctx));
                if value::is_exception(custom) {
                    return custom;
                }
                state
                    .string(custom)
                    .and_then(|text| text.to_utf8())
                    .unwrap_or_else(|| default_tag.to_owned())
            };
            state
                .intern_text(format!("[object {tag}]"), value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ObjectHasOwn | Builtin::HasOwnProperty => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            let Some(key) = property_key(state, *key) else {
                return fail_dispatch(ctx);
            };
            if value::is_callable(*object) {
                return value::encode_bool(
                    state.callable_properties.contains_key(&(*object, key))
                        || state.callable_accessors.contains_key(&(*object, key)),
                );
            }
            let Some(object) = object_handle(*object) else {
                return fail_dispatch(ctx);
            };
            state
                .heap
                .get_property(object, key)
                .map(|property| value::encode_bool(property.is_some()))
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::ArrayPush => {
            let [array, element] = args else {
                return fail_dispatch(ctx);
            };
            let Some(array) = object_handle(*array) else {
                return fail_dispatch(ctx);
            };
            state
                .heap
                .push_element(array, *element as u64)
                .map(|length| value::encode_f64(f64::from(length)))
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::ArrayPushSpread => {
            let [target, source] = args else {
                return fail_dispatch(ctx);
            };
            let (Some(target), Some(source)) = (object_handle(*target), object_handle(*source))
            else {
                return fail_dispatch(ctx);
            };
            let Ok(length) = state.heap.array_length(source) else {
                return fail_dispatch(ctx);
            };
            for index in 0..length {
                let Ok(Some(element)) = state.heap.get_element(source, index) else {
                    return fail_dispatch(ctx);
                };
                if state.heap.push_element(target, element).is_err() {
                    return fail_dispatch(ctx);
                }
            }
            state
                .heap
                .array_length(target)
                .map(|length| value::encode_f64(f64::from(length)))
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::CreateGlobalObject => {
            if let Some(global) = state.global_object {
                global
            } else if state.ensure_intrinsic_prototypes().is_err() {
                fail_dispatch(ctx)
            } else {
                match state.allocate_object(0, false) {
                    Ok(global) => {
                        state.global_object = Some(global);
                        global
                    }
                    Err(_) => fail_dispatch(ctx),
                }
            }
        }
        Builtin::ExceptionValue => args
            .first()
            .and_then(|exception| state.exception_value(*exception))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::Throw => args
            .first()
            .and_then(|argument| state.create_exception(*argument))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Builtin::F64Mod => binary_f64(args, |left, right| left % right),
        Builtin::F64Exp => binary_f64(args, f64::powf),
        Builtin::Debugger => {
            crate::inspector::pause(ctx, state, "debuggerStatement");
            value::encode_undefined()
        }
        Builtin::In => {
            let [object, key] = args else {
                return fail_dispatch(ctx);
            };
            if value::is_proxy(*object) {
                proxy::has(ctx, state, &[*object, *key])
            } else {
                value::encode_bool(has_property(state, *object, *key))
            }
        }
        Builtin::IteratorFrom => iterator_from(ctx, state, args),
        Builtin::IteratorDone => iterator_done(ctx, state, args),
        Builtin::IteratorValue => iterator_value(ctx, state, args, false),
        Builtin::IteratorStepValue => iterator_value(ctx, state, args, true),
        Builtin::IteratorNext => iterator_next(ctx, state, args),
        Builtin::IteratorClose => iterator_close(ctx, state, args),
        Builtin::SetTimeout | Builtin::SetInterval => {
            let Some(callback) = args
                .first()
                .copied()
                .filter(|callback| value::is_callable(*callback))
            else {
                return runtime::type_error(
                    ctx,
                    state,
                    "TypeError: timer callback must be callable",
                );
            };
            let delay = args
                .get(1)
                .and_then(|delay| runtime::to_number(state, *delay))
                .filter(|delay| delay.is_finite() && *delay > 0.0)
                .map_or(0, |delay| delay.trunc() as u64);
            promise::enqueue_timer(
                ctx,
                state,
                callback,
                args.get(2..).unwrap_or_default().to_vec(),
                "Timeout",
                delay,
                builtin == Builtin::SetInterval,
            )
        }
        Builtin::ClearTimeout | Builtin::ClearInterval => {
            if let Some(timer) = args.first() {
                state.cancelled_timers.insert(value::decode_handle(*timer));
                if let Some(exception) =
                    node_async_hooks::destroy_scheduled_resource(ctx, state, *timer)
                {
                    return exception;
                }
            }
            value::encode_undefined()
        }
        Builtin::NewTarget => state
            .activations
            .last()
            .map(|activation| activation.new_target)
            .unwrap_or_else(value::encode_undefined),
        _ => {
            if std::env::var_os("WJSM_TRACE_INVARIANT").is_some() {
                eprintln!("native unhandled builtin: {builtin:?}");
            }
            fail_dispatch(ctx)
        }
    }
}

pub(crate) fn error_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let name = match builtin {
        Builtin::ErrorConstructor => "Error",
        Builtin::EvalErrorConstructor => "EvalError",
        Builtin::RangeErrorConstructor => "RangeError",
        Builtin::ReferenceErrorConstructor => "ReferenceError",
        Builtin::SyntaxErrorConstructor => "SyntaxError",
        Builtin::TypeErrorConstructor => "TypeError",
        Builtin::URIErrorConstructor => "URIError",
        _ => return fail_dispatch(ctx),
    };
    let message = match args.first().copied() {
        None => String::new(),
        Some(message) if value::is_undefined(message) => String::new(),
        Some(message) => match to_string_coerced(ctx, state, message) {
            Ok(message) => message,
            Err(exception) => return exception,
        },
    };
    let Some(intrinsic_prototype) = state.ensure_error_prototype(name) else {
        return fail_dispatch(ctx);
    };
    let new_target = state
        .activations
        .last()
        .map(|activation| activation.new_target)
        .unwrap_or_else(value::encode_undefined);
    let error = if !value::is_undefined(new_target) && value::is_js_object(this_value) {
        modules::initialize_error_object(state, this_value, name, message)
    } else {
        modules::named_error_object(state, name, message)
    };
    let Some(error) = error else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(new_target) {
        let Some(prototype_key) = state
            .intern_text("prototype".into(), value::TAG_STRING)
            .map(value::decode_handle)
        else {
            return fail_dispatch(ctx);
        };
        let prototype = state
            .callable_property(new_target, prototype_key)
            .filter(|prototype| value::is_js_object(*prototype))
            .unwrap_or(intrinsic_prototype);
        if state
            .heap
            .set_prototype(value::decode_handle(error), value::decode_handle(prototype))
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    let Some(options) = args.get(1).copied() else {
        return error;
    };
    if !value::is_js_object(options) && !value::is_regexp(options) {
        return error;
    }
    let Some(cause_key) = state.intern_text("cause".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    if has_property(state, options, cause_key) {
        let cause =
            get_property(ctx, state, options, cause_key).unwrap_or_else(|()| fail_dispatch(ctx));
        if value::is_exception(cause) {
            return cause;
        }
        if modules::set_named_property(state, error, "cause", cause).is_err() {
            return fail_dispatch(ctx);
        }
    }
    error
}

fn abstract_compare(state: &NativeAgentState, left: i64, right: i64) -> Option<Ordering> {
    if value::is_string(left) && value::is_string(right) {
        return state
            .string(left)?
            .as_utf16_units()
            .partial_cmp(state.string(right)?.as_utf16_units());
    }
    match (value::is_bigint(left), value::is_bigint(right)) {
        (true, true) => bigint::read(state, left)?.partial_cmp(&bigint::read(state, right)?),
        (true, false) => {
            bigint_number_compare(&bigint::read(state, left)?, to_number(state, right)?)
        }
        (false, true) => {
            bigint_number_compare(&bigint::read(state, right)?, to_number(state, left)?)
                .map(Ordering::reverse)
        }
        (false, false) => to_number(state, left)?.partial_cmp(&to_number(state, right)?),
    }
}

fn bigint_number_compare(bigint: &BigInt, number: f64) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }
    if number == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if number == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let integral = BigInt::from_f64(number.trunc())?;
    let comparison = bigint.cmp(&integral);
    if comparison != Ordering::Equal || number.fract() == 0.0 {
        return Some(comparison);
    }
    Some(if number.is_sign_positive() {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

fn eval_get_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key] = args else {
        return fail_dispatch(ctx);
    };
    if state.text_matches(*key, "__wjsm_new_target")
        && let Some(new_target) = modules::scope_record_new_target(state, *environment)
    {
        return new_target;
    }
    match modules::scope_record_get(state, *environment, *key) {
        modules::ScopeBindingRead::Value(result) => return result,
        modules::ScopeBindingRead::Uninitialized => {
            let name = eval_binding_name(state, *key);
            return javascript_error(
                ctx,
                state,
                "ReferenceError",
                format!("Cannot access '{name}' before initialization"),
            );
        }
        modules::ScopeBindingRead::Missing => {}
    }
    let outer = modules::scope_record_outer(state, *environment).unwrap_or(*environment);
    let Ok(result) = runtime::get_property(ctx, state, outer, *key) else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(result) || eval_binding_exists(ctx, state, *environment, *key) {
        return result;
    }
    javascript_error(
        ctx,
        state,
        "ReferenceError",
        format!("{} is not defined", eval_binding_name(state, *key)),
    )
}

fn eval_set_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key, stored] = args else {
        return fail_dispatch(ctx);
    };
    match modules::scope_record_set(state, *environment, *key, *stored) {
        modules::ScopeBindingWrite::Updated => return *stored,
        modules::ScopeBindingWrite::Constant => {
            return javascript_error(
                ctx,
                state,
                "TypeError",
                format!(
                    "assignment to constant `{}`",
                    eval_binding_name(state, *key)
                ),
            );
        }
        modules::ScopeBindingWrite::Missing => {}
    }
    if modules::scope_record_is_strict(state, *environment) {
        return javascript_error(
            ctx,
            state,
            "ReferenceError",
            format!(
                "assignment to undeclared variable `{}`",
                eval_binding_name(state, *key)
            ),
        );
    }
    let outer = modules::scope_record_outer(state, *environment).unwrap_or(*environment);
    dispatch_runtime(
        ctx,
        state,
        NativeRuntimeOp::SetProp,
        &[outer, *key, *stored],
    )
}

fn eval_binding_exists(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    environment: i64,
    key: i64,
) -> bool {
    if state.text_matches(key, "__wjsm_new_target")
        && modules::scope_record_new_target(state, environment).is_some()
    {
        return true;
    }
    if modules::scope_record_contains(state, environment, key) {
        return true;
    }
    let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
    runtime::get_property(ctx, state, outer, key)
        .is_ok_and(|property| !value::is_undefined(property) || has_property(state, outer, key))
}

fn eval_binding_name(state: &NativeAgentState, key: i64) -> String {
    state
        .string(key)
        .and_then(wjsm_host::RuntimeString::to_utf8)
        .unwrap_or_else(|| render_value(state, key))
}

fn javascript_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: String,
) -> i64 {
    modules::named_error_object(state, name, message)
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn eval_execution_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: Result<i64, modules::VmExecutionError>,
) -> i64 {
    match result {
        Ok(result) => result,
        Err(modules::VmExecutionError::JavaScript(exception)) => exception,
        Err(modules::VmExecutionError::Compile(error)) => {
            if let Some(wjsm_semantic::LoweringError::Diagnostic(diagnostic)) =
                error.downcast_ref::<wjsm_semantic::LoweringError>()
            {
                if diagnostic.message.contains("cannot redeclare identifier") {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "SyntaxError",
                        format!("cannot redeclare identifier `{identifier}` in eval"),
                    );
                }
                if diagnostic
                    .message
                    .contains("cannot reassign a const-declared variable")
                {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "TypeError",
                        format!("assignment to constant `{identifier}`"),
                    );
                }
            }
            javascript_error(ctx, state, "SyntaxError", error.to_string())
        }
        Err(error) => javascript_error(ctx, state, "Error", error.to_string()),
    }
}
