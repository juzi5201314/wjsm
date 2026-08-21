use wjsm_host::RuntimeString;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{PrimitiveHint, fail_dispatch, render_value, to_primitive, type_error};
use crate::NativeAgentState;

pub(super) fn dispatch_symbol(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::SymbolCreate => {
            let description = match args.first().copied() {
                None => None,
                Some(argument) if value::is_undefined(argument) => None,
                Some(argument) => match to_runtime_string(ctx, state, argument) {
                    Ok(description) => Some(description),
                    Err(exception) => return Some(exception),
                },
            };
            state
                .create_symbol(description)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::SymbolFor => {
            let argument = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let key = match to_runtime_string(ctx, state, argument) {
                Ok(key) => key,
                Err(exception) => return Some(exception),
            };
            if let Some(handle) = state.symbol_registry.get(&key).copied() {
                value::encode_handle(value::TAG_SYMBOL, handle)
            } else {
                let Some(symbol) = state.create_symbol(Some(key.clone())) else {
                    return Some(fail_dispatch(ctx));
                };
                state
                    .symbol_registry
                    .insert(key, value::decode_handle(symbol));
                symbol
            }
        }
        Builtin::SymbolKeyFor => {
            let Some(symbol) = args.first().copied() else {
                return Some(type_error(ctx, state, "Symbol.keyFor requires a symbol"));
            };
            if !value::is_symbol(symbol) {
                return Some(type_error(ctx, state, "Symbol.keyFor requires a symbol"));
            }
            let handle = value::decode_handle(symbol);
            let key = state
                .symbol_registry
                .iter()
                .find_map(|(key, candidate)| (*candidate == handle).then(|| key.clone()));
            key.and_then(|key| state.intern_runtime_string(key, value::TAG_STRING))
                .unwrap_or_else(value::encode_undefined)
        }
        Builtin::SymbolWellKnown => {
            let Some(index) = args
                .first()
                .copied()
                .filter(|value| value::is_f64(*value))
                .map(value::decode_f64)
                .filter(|index| {
                    index.is_finite()
                        && index.fract() == 0.0
                        && *index >= 0.0
                        && *index <= f64::from(wjsm_ir::wk_symbol::UNSCOPABLES)
                })
            else {
                return Some(fail_dispatch(ctx));
            };
            value::encode_handle(value::TAG_SYMBOL, index as u32)
        }
        Builtin::SymbolProtoToString => {
            let symbol = match this_symbol_value(ctx, state, args) {
                Ok(symbol) => symbol,
                Err(exception) => return Some(exception),
            };
            let description = state
                .symbol_description(symbol)
                .map(|description| description.to_utf8_lossy());
            let text =
                description.map_or_else(|| "Symbol()".into(), |text| format!("Symbol({text})"));
            state
                .intern_text(text, value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::SymbolProtoValueOf => match this_symbol_value(ctx, state, args) {
            Ok(symbol) => symbol,
            Err(exception) => exception,
        },
        _ => return None,
    })
}

fn to_runtime_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<RuntimeString, i64> {
    let primitive = to_primitive(ctx, state, encoded, PrimitiveHint::String)?;
    if value::is_symbol(primitive) {
        return Err(type_error(
            ctx,
            state,
            "Cannot convert a Symbol value to a string",
        ));
    }
    if value::is_string(primitive) {
        return state
            .string_owned(primitive)
            .ok_or_else(|| fail_dispatch(ctx));
    }
    Ok(RuntimeString::from(render_value(state, primitive)))
}

fn this_symbol_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<i64, i64> {
    args.first()
        .copied()
        .and_then(|receiver| state.symbol_value(receiver))
        .ok_or_else(|| type_error(ctx, state, "Symbol method called on incompatible receiver"))
}

pub(crate) fn well_known_description(handle: u32) -> Option<&'static str> {
    Some(match handle {
        wjsm_ir::wk_symbol::ITERATOR => "Symbol.iterator",
        wjsm_ir::wk_symbol::SPECIES => "Symbol.species",
        wjsm_ir::wk_symbol::TO_STRING_TAG => "Symbol.toStringTag",
        wjsm_ir::wk_symbol::ASYNC_ITERATOR => "Symbol.asyncIterator",
        wjsm_ir::wk_symbol::HAS_INSTANCE => "Symbol.hasInstance",
        wjsm_ir::wk_symbol::TO_PRIMITIVE => "Symbol.toPrimitive",
        wjsm_ir::wk_symbol::DISPOSE => "Symbol.dispose",
        wjsm_ir::wk_symbol::MATCH => "Symbol.match",
        wjsm_ir::wk_symbol::ASYNC_DISPOSE => "Symbol.asyncDispose",
        wjsm_ir::wk_symbol::IS_CONCAT_SPREADABLE => "Symbol.isConcatSpreadable",
        wjsm_ir::wk_symbol::MATCH_ALL => "Symbol.matchAll",
        wjsm_ir::wk_symbol::REPLACE => "Symbol.replace",
        wjsm_ir::wk_symbol::SEARCH => "Symbol.search",
        wjsm_ir::wk_symbol::SPLIT => "Symbol.split",
        wjsm_ir::wk_symbol::UNSCOPABLES => "Symbol.unscopables",
        _ => return None,
    })
}
