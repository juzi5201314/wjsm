mod parse;
mod stringify;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime;
use crate::NativeAgentState;

pub(super) fn dispatch_json(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::JsonParse => parse::parse(ctx, state, args),
        Builtin::JsonStringify => stringify(ctx, state, args),
        _ => return None,
    })
}

fn stringify(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    stringify::stringify(ctx, state, args)
}
pub(super) fn get_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<i64, i64> {
    let key = state
        .intern_text(name.into(), value::TAG_STRING)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    let encoded =
        runtime::get_property(ctx, state, object, key).map_err(|_| runtime::fail_dispatch(ctx))?;
    if value::is_exception(encoded) {
        Err(encoded)
    } else {
        Ok(encoded)
    }
}

pub(super) fn invoke(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: i64,
    this_value: i64,
    arguments: &[i64],
) -> Result<i64, i64> {
    let encoded = state
        .invoke_callable(ctx, callable, this_value, arguments)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    if value::is_exception(encoded) {
        Err(encoded)
    } else {
        Ok(encoded)
    }
}
