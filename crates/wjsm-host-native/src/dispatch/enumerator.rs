use std::collections::HashSet;

use wjsm_ir::{Builtin, value};

use super::{fail_dispatch, runtime};
use crate::{NativeAgentState, NativeVmContext};

pub(crate) struct NativeEnumerator {
    keys: Vec<i64>,
    index: usize,
}

pub(super) fn dispatch_enumerator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::EnumeratorFrom => from(ctx, state, args),
        Builtin::EnumeratorDone => done(ctx, state, args),
        Builtin::EnumeratorKey => key(ctx, state, args),
        Builtin::EnumeratorNext => next(ctx, state, args),
        _ => return None,
    })
}

fn from(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(keys) = enumerable_keys(state, source) else {
        return fail_dispatch(ctx);
    };
    let Ok(enumerator) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    state.enumerators.insert(
        value::decode_handle(enumerator),
        NativeEnumerator { keys, index: 0 },
    );
    enumerator
}

fn enumerable_keys(state: &mut NativeAgentState, source: i64) -> Option<Vec<i64>> {
    if value::is_null(source) || value::is_undefined(source) {
        return Some(Vec::new());
    }
    if value::is_string(source) {
        let length = state.string_len(source)?;
        let mut keys = Vec::with_capacity(length);
        for index in 0..length {
            keys.push(state.intern_text(index.to_string(), value::TAG_STRING)?);
        }
        return Some(keys);
    }
    let Some(mut object) = runtime::object_handle(source) else {
        return Some(Vec::new());
    };
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let encoded = if state.gc.heap().object_type(object).ok()
            == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
        {
            value::encode_handle(value::TAG_ARRAY, object)
        } else {
            value::encode_object_handle(object)
        };
        let all = super::object::own_keys(state, encoded, false)?;
        let enumerable: HashSet<i64> = super::object::own_keys(state, encoded, true)?
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        for (key, _) in all {
            if !value::is_string(key) || !seen.insert(key) {
                continue;
            }
            if enumerable.contains(&key) {
                keys.push(key);
            }
        }
        object = state.gc.heap().prototype(object).ok()?;
        if object == u32::MAX {
            break;
        }
    }
    Some(keys)
}

fn done(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(enumerator) = entry(state, args) else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(enumerator.index >= enumerator.keys.len())
}

fn key(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    entry(state, args)
        .and_then(|enumerator| enumerator.keys.get(enumerator.index).copied())
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn next(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(args) else {
        return fail_dispatch(ctx);
    };
    let Some(enumerator) = state.enumerators.get_mut(&handle) else {
        return fail_dispatch(ctx);
    };
    enumerator.index = enumerator.index.saturating_add(1);
    value::encode_undefined()
}

fn entry<'a>(state: &'a NativeAgentState, args: &[i64]) -> Option<&'a NativeEnumerator> {
    state.enumerators.get(&handle(args)?)
}

fn handle(args: &[i64]) -> Option<u32> {
    args.first()
        .copied()
        .filter(|enumerator| value::is_js_object(*enumerator))
        .map(value::decode_handle)
}
