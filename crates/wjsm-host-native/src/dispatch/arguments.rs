use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::NativeVmContext;

use super::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind, PropertyKey};
use wjsm_host::RuntimeString;

const DATA_FLAGS: u32 =
    (constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE) as u32;
const HIDDEN_DATA_FLAGS: u32 = (constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE) as u32;

pub(super) fn dispatch_arguments(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let mapped = match builtin {
        Builtin::CreateMappedArgumentsObject => true,
        Builtin::CreateUnmappedArgumentsObject => false,
        _ => return None,
    };
    Some(create(ctx, state, mapped, args))
}

pub(super) fn create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    mapped: bool,
    args: &[i64],
) -> i64 {
    let Some(source) = args
        .first()
        .copied()
        .filter(|source| value::is_array(*source))
    else {
        return fail_dispatch(ctx);
    };
    let source_handle = value::decode_handle(source);
    let Ok(length) = state.gc.heap().array_length(source_handle) else {
        return fail_dispatch(ctx);
    };
    let Ok(arguments) = state.allocate_object_with_gc_retry(ctx, length.saturating_add(3), false)
    else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(arguments);
    if state
        .gc
        .heap()
        .set_object_type(handle, wjsm_ir::HEAP_TYPE_ARGUMENTS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    for index in 0..length {
        let stored = state
            .gc
            .heap()
            .get_element(source_handle, index)
            .ok()
            .flatten()
            .map(|stored| stored as i64)
            .unwrap_or_else(value::encode_undefined);
        let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_data_property(handle, key, stored as u64, DATA_FLAGS)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    if !define_named(
        state,
        handle,
        "length",
        value::encode_f64(f64::from(length)),
        HIDDEN_DATA_FLAGS,
    ) {
        return fail_dispatch(ctx);
    }
    let Some(iterator) =
        state.native_callable(NativeCallableKind::Builtin(Builtin::IteratorFrom, true))
    else {
        return fail_dispatch(ctx);
    };
    let iterator_key = PropertyKey::symbol(wjsm_ir::wk_symbol::ITERATOR);

    if state
        .gc
        .heap()
        .define_data_property(handle, iterator_key, iterator as u64, HIDDEN_DATA_FLAGS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    if mapped {
        if let Some(callee) = args
            .get(2)
            .copied()
            .filter(|callee| !value::is_undefined(*callee))
            && !define_named(state, handle, "callee", callee, HIDDEN_DATA_FLAGS)
        {
            return fail_dispatch(ctx);
        }
    } else {
        let Some(thrower) = state.native_callable(NativeCallableKind::ArgumentsStrictCallee) else {
            return fail_dispatch(ctx);
        };
        let Some(key) = state.intern_property_string("callee".into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_accessor_property_with_flags(handle, key, thrower as u64, thrower as u64, 0)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    arguments
}

pub(crate) fn strict_callee_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    super::modules::named_error_object(
        state,
        "TypeError",
        "'callee' and 'caller' properties are not defined".into(),
    )
    .and_then(|error| state.create_exception(error))
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn define_named(
    state: &mut NativeAgentState,
    object: u32,
    name: &str,
    stored: i64,
    flags: u32,
) -> bool {
    let Some(key) = state.intern_property_string(name.into()) else {
        return false;
    };
    state
        .gc
        .heap()
        .define_data_property(object, key, stored as u64, flags)
        .is_ok()
}
