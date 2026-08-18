//! IDNA / UTS #46 host 桥：供 `node:url` 的 domainToASCII/Unicode 与 host 解析共用。

use wjsm_intl_data::{domain_to_ascii_uts46, domain_to_unicode_uts46};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::runtime::{fail_dispatch, to_string_coerced};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IdnaMethod {
    DomainToAscii,
    DomainToUnicode,
}

#[derive(Default)]
pub(crate) struct IdnaState {
    bridge: Option<i64>,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.idna.bridge {
        return Some(bridge);
    }
    let methods = [
        ("domainToASCII", IdnaMethod::DomainToAscii),
        ("domainToUnicode", IdnaMethod::DomainToUnicode),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::Idna(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.idna.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: IdnaMethod,
    args: &[i64],
) -> i64 {
    match method {
        IdnaMethod::DomainToAscii => domain_to_ascii(ctx, state, args),
        IdnaMethod::DomainToUnicode => domain_to_unicode(ctx, state, args),
    }
}

fn domain_to_ascii(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let domain = match read_domain(ctx, state, args) {
        Ok(domain) => domain,
        Err(exception) => return exception,
    };
    // Node：失败时返回空串，不抛异常。
    let ascii = domain_to_ascii_uts46(&domain).unwrap_or_default();
    state
        .intern_text(ascii, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn domain_to_unicode(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let domain = match read_domain(ctx, state, args) {
        Ok(domain) => domain,
        Err(exception) => return exception,
    };
    let unicode = domain_to_unicode_uts46(&domain);
    state
        .intern_text(unicode, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn read_domain(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<String, i64> {
    let Some(encoded) = args.first().copied() else {
        return Err(modules::named_error_object(
            state,
            "TypeError",
            "The \"domain\" argument must be specified".to_owned(),
        )
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx)));
    };
    to_string_coerced(ctx, state, encoded)
}
