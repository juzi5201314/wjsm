use std::ops::Range;

use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    fail_dispatch, get_property as get_runtime_property, render_value, syntax_error, to_number,
    type_error,
};
use crate::{NativeAgentState, NativeCallableKind};

struct MatchInfo {
    start: usize,
    end: usize,
    captures: Vec<Option<Range<usize>>>,
    named_captures: Vec<(String, Option<Range<usize>>)>,
}

fn match_info(found: regress::Match) -> MatchInfo {
    let named_captures = found
        .named_groups()
        .map(|(name, range)| (name.to_owned(), range))
        .collect();
    let captures = (0..=found.captures.len())
        .map(|index| found.group(index))
        .collect();
    MatchInfo {
        start: found.start(),
        end: found.end(),
        captures,
        named_captures,
    }
}

pub(crate) struct RegExpIterator {
    regexp: i64,
    input: String,
    last_index: usize,
    done: bool,
}

pub(super) fn dispatch_regexp(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::RegExpCreate => create(ctx, state, args),
        Builtin::RegExpExec => exec(ctx, state, args),
        Builtin::RegExpTest => test(ctx, state, args),
        Builtin::RegExpProtoMatch => regexp_symbol_match(ctx, state, args),
        Builtin::RegExpProtoReplace => regexp_symbol_replace(ctx, state, args),
        Builtin::RegExpProtoSearch => regexp_symbol_search(ctx, state, args),
        Builtin::RegExpProtoSplit => regexp_symbol_split(ctx, state, args),
        _ => return None,
    })
}

fn create(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let pattern_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let flags_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if value::is_regexp(pattern_arg) && value::is_undefined(flags_arg) {
        return pattern_arg;
    }
    let pattern = if let Some(regexp) = state.regexp(pattern_arg) {
        regexp.pattern.clone()
    } else if value::is_undefined(pattern_arg) {
        String::new()
    } else {
        render_value(state, pattern_arg)
    };
    let flags = if value::is_undefined(flags_arg) {
        state
            .regexp(pattern_arg)
            .map(|regexp| regexp.flags.clone())
            .unwrap_or_default()
    } else {
        render_value(state, flags_arg)
    };
    compile_regexp(ctx, state, pattern, flags)
}
fn compile_regexp(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    pattern: String,
    flags: String,
) -> i64 {
    match state.create_regexp(pattern, flags) {
        Ok(regexp) => regexp,
        Err(error) => syntax_error(ctx, state, &error.to_string()),
    }
}

fn subject(state: &NativeAgentState, encoded: i64) -> String {
    if let Some(primitive) = super::runtime::primitive_string(state, encoded) {
        // boxed String 包装对象与原语同路：ToString(this) 经 ToPrimitive
        // 归约为 [[StringData]]（§7.1.17）。
        state
            .string_owned(primitive)
            .map(|text| text.to_utf8_lossy())
            .unwrap_or_default()
    } else {
        render_value(state, encoded)
    }
}

fn subject_runtime_string(state: &NativeAgentState, encoded: i64) -> wjsm_host::RuntimeString {
    if let Some(primitive) = super::runtime::primitive_string(state, encoded) {
        state.string_owned(primitive).unwrap_or_default()
    } else if value::is_symbol(encoded) {
        wjsm_host::RuntimeString::empty()
    } else {
        wjsm_host::RuntimeString::from(render_value(state, encoded))
    }
}
pub(crate) fn symbol_builtin(key: i64) -> Option<Builtin> {
    if !value::is_symbol(key) {
        return None;
    }
    match value::decode_handle(key) {
        wk_symbol::MATCH => Some(Builtin::RegExpProtoMatch),
        wk_symbol::REPLACE => Some(Builtin::RegExpProtoReplace),
        wk_symbol::SEARCH => Some(Builtin::RegExpProtoSearch),
        wk_symbol::SPLIT => Some(Builtin::RegExpProtoSplit),
        _ => None,
    }
}
pub(super) fn get_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    regexp: i64,
    key: i64,
) -> Option<i64> {
    if let Some(builtin) = symbol_builtin(key) {
        return state.native_callable(NativeCallableKind::Builtin(builtin, true));
    }
    if state.text_matches(key, "source") {
        Some(property_string(ctx, state, regexp, PropertyString::Source))
    } else if state.text_matches(key, "flags") {
        Some(property_string(ctx, state, regexp, PropertyString::Flags))
    } else if state.text_matches(key, "global") {
        Some(property_flag(ctx, state, regexp, 'g'))
    } else if state.text_matches(key, "ignoreCase") {
        Some(property_flag(ctx, state, regexp, 'i'))
    } else if state.text_matches(key, "multiline") {
        Some(property_flag(ctx, state, regexp, 'm'))
    } else if state.text_matches(key, "sticky") {
        Some(property_flag(ctx, state, regexp, 'y'))
    } else if state.text_matches(key, "unicode") {
        Some(property_flag(ctx, state, regexp, 'u'))
    } else if state.text_matches(key, "dotAll") {
        Some(property_flag(ctx, state, regexp, 's'))
    } else if state.text_matches(key, "hasIndices") {
        Some(property_flag(ctx, state, regexp, 'd'))
    } else if state.text_matches(key, "lastIndex") {
        Some(get_last_index(ctx, state, regexp))
    } else {
        state.primitive_property(regexp, key)
    }
}

fn execute_match(state: &mut NativeAgentState, regexp: i64, input: &str) -> Option<MatchInfo> {
    let entry = state.regexp_mut(regexp)?;
    let global = entry.flags.contains('g');
    let sticky = entry.flags.contains('y');
    let start = if global || sticky {
        entry.last_index
    } else {
        0
    };
    let found = if global || sticky {
        entry.compiled.find_from(input, start).next()
    } else {
        entry.compiled.find(input)
    };
    let info = match found {
        Some(found) if !sticky || found.start() == start => Some(match_info(found)),
        _ => None,
    };
    if global || sticky {
        entry.last_index = match &info {
            Some(info) if info.end == start && start < input.len() => advance(input, start),
            Some(info) => info.end,
            None => 0,
        };
    }
    info
}

fn advance(input: &str, index: usize) -> usize {
    input[index..].chars().next().map_or_else(
        || index.saturating_add(1),
        |character| index + character.len_utf8(),
    )
}

fn build_named_groups(
    state: &mut NativeAgentState,
    input: &str,
    info: &MatchInfo,
    indices: bool,
) -> Option<i64> {
    if info.named_captures.is_empty() {
        return Some(value::encode_undefined());
    }
    let capacity = u32::try_from(info.named_captures.len()).ok()?;
    let groups = state.allocate_object(capacity, false).ok()?;
    let handle = value::decode_handle(groups);
    for (name, capture) in &info.named_captures {
        let key = state.intern_property_string(name.clone().into())?;
        let stored = match capture {
            Some(range) if indices => build_index_pair(state, input, range.clone())?,
            Some(range) => state.intern_text(input[range.clone()].to_owned(), value::TAG_STRING)?,
            None => value::encode_undefined(),
        };
        state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .ok()?;
    }
    Some(groups)
}

fn build_index_pair(state: &mut NativeAgentState, input: &str, range: Range<usize>) -> Option<i64> {
    let start = u32::try_from(input[..range.start].encode_utf16().count()).ok()?;
    let end = u32::try_from(input[..range.end].encode_utf16().count()).ok()?;
    state
        .allocate_array_values(&[
            value::encode_f64(f64::from(start)),
            value::encode_f64(f64::from(end)),
        ])
        .ok()
}

fn build_match_indices(state: &mut NativeAgentState, input: &str, info: &MatchInfo) -> Option<i64> {
    let values = info
        .captures
        .iter()
        .map(|capture| {
            capture.as_ref().map_or_else(
                || Some(value::encode_undefined()),
                |range| build_index_pair(state, input, range.clone()),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let indices = state.allocate_array_values(&values).ok()?;
    let groups = build_named_groups(state, input, info, true)?;
    let key = state.intern_property_string("groups".into())?;
    let handle = value::decode_handle(indices);
    state.note_array_property(handle, key);
    state.array_properties.insert((handle, key), groups);
    Some(indices)
}

fn build_match_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: &str,
    info: &MatchInfo,
    input_value: Option<i64>,
    has_indices: bool,
) -> i64 {
    let mut captures = Vec::with_capacity(info.captures.len());
    for capture in &info.captures {
        let encoded = match capture {
            Some(range) => state
                .intern_text(input[range.clone()].to_owned(), value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx)),
            None => value::encode_undefined(),
        };
        captures.push(encoded);
    }
    let Ok(result) = state.allocate_array_values_with_gc_retry(ctx, &captures) else {
        return fail_dispatch(ctx);
    };
    let result_handle = value::decode_handle(result);
    let Ok(index) = u32::try_from(input[..info.start].encode_utf16().count()) else {
        return fail_dispatch(ctx);
    };
    let input_value = input_value
        .filter(|value| value::is_string(*value))
        .unwrap_or_else(|| {
            state
                .intern_text(input.to_owned(), value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        });
    let Some(groups) = build_named_groups(state, input, info, false) else {
        return fail_dispatch(ctx);
    };
    let mut properties = vec![
        ("index", value::encode_f64(f64::from(index))),
        ("input", input_value),
        ("groups", groups),
    ];
    if has_indices {
        let Some(indices) = build_match_indices(state, input, info) else {
            return fail_dispatch(ctx);
        };
        properties.push(("indices", indices));
    }
    for (name, stored) in properties {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        state.note_array_property(result_handle, key);
        state.array_properties.insert((result_handle, key), stored);
    }
    result
}

fn exec(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [regexp, input] = args else {
        return fail_dispatch(ctx);
    };
    let input_text = subject(state, *input);
    let has_indices = state
        .regexp(*regexp)
        .is_some_and(|entry| entry.flags.contains('d'));
    let Some(info) = execute_match(state, *regexp, &input_text) else {
        return value::encode_null();
    };
    build_match_result(ctx, state, &input_text, &info, Some(*input), has_indices)
}
fn iterator_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: i64,
    done: bool,
) -> i64 {
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(result);
    for (name, stored) in [("value", value), ("done", value::encode_bool(done))] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}

pub(super) fn string_match_all(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [receiver, pattern] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_undefined(*pattern) {
        return fail_dispatch(ctx);
    }
    let regexp = if value::is_regexp(*pattern) {
        *pattern
    } else {
        let regexp = compile_regexp(ctx, state, subject(state, *pattern), "g".into());
        if value::is_exception(regexp) {
            return regexp;
        }
        regexp
    };
    let Some(last_index) = state
        .regexp(regexp)
        .and_then(|entry| entry.flags.contains('g').then_some(entry.last_index))
    else {
        return fail_dispatch(ctx);
    };
    let input = subject(state, *receiver);
    let Ok(iterator_id) = u32::try_from(state.regexp_iterators.len()) else {
        return fail_dispatch(ctx);
    };
    state.regexp_iterators.push(RegExpIterator {
        regexp,
        input,
        last_index,
        done: false,
    });
    let Ok(iterator_object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    let Some(next) = state.native_callable(NativeCallableKind::RegExpIteratorNext(iterator_id))
    else {
        return fail_dispatch(ctx);
    };
    state.iterator_next.insert(
        value::decode_handle(iterator_object),
        value::decode_native_callable_idx(next),
    );
    iterator_object
}

pub(crate) fn next_match_all(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator_id: u32,
) -> i64 {
    let Ok(index) = usize::try_from(iterator_id) else {
        return fail_dispatch(ctx);
    };
    let Some((regexp, input, last_index, done)) =
        state.regexp_iterators.get(index).map(|iterator| {
            (
                iterator.regexp,
                iterator.input.clone(),
                iterator.last_index,
                iterator.done,
            )
        })
    else {
        return fail_dispatch(ctx);
    };
    if done {
        return iterator_result(ctx, state, value::encode_undefined(), true);
    }
    let Some(info) = (|| {
        let entry = state.regexp(regexp)?;
        let sticky = entry.flags.contains('y');
        let found = entry.compiled.find_from(&input, last_index).next()?;
        if sticky && found.start() != last_index {
            return None;
        }
        Some(match_info(found))
    })() else {
        if let Some(iterator) = state.regexp_iterators.get_mut(index) {
            iterator.done = true;
        }
        return iterator_result(ctx, state, value::encode_undefined(), true);
    };
    let next_index = if info.start == info.end && last_index < input.len() {
        advance(&input, info.end)
    } else {
        info.end
    };
    if let Some(iterator) = state.regexp_iterators.get_mut(index) {
        iterator.last_index = next_index;
    }
    let has_indices = state
        .regexp(regexp)
        .is_some_and(|entry| entry.flags.contains('d'));
    let input_value = state
        .intern_text(input.clone(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx));
    let result = build_match_result(ctx, state, &input, &info, Some(input_value), has_indices);
    if value::is_exception(result) {
        return result;
    }
    iterator_result(ctx, state, result, false)
}

fn test(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [regexp, input] = args else {
        return fail_dispatch(ctx);
    };
    let input = subject(state, *input);
    value::encode_bool(execute_match(state, *regexp, &input).is_some())
}

#[derive(Clone, Copy)]
enum PropertyString {
    Flags,
    Source,
}

fn property_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
    kind: PropertyString,
) -> i64 {
    let Some(regexp) = state.regexp(encoded) else {
        return fail_dispatch(ctx);
    };
    let text = match kind {
        PropertyString::Flags => regexp.flags.clone(),
        PropertyString::Source => regexp.pattern.clone(),
    };
    state
        .intern_text(text, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn property_flag(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    encoded: i64,
    flag: char,
) -> i64 {
    state
        .regexp(encoded)
        .map(|regexp| value::encode_bool(regexp.flags.contains(flag)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn get_last_index(ctx: &mut NativeVmContext, state: &NativeAgentState, encoded: i64) -> i64 {
    state
        .regexp(encoded)
        .and_then(|regexp| u32::try_from(regexp.last_index).ok())
        .map(|last_index| value::encode_f64(f64::from(last_index)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) fn set_last_index(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [regexp, encoded] = args else {
        return fail_dispatch(ctx);
    };
    let Some(number) = to_number(state, *encoded) else {
        return fail_dispatch(ctx);
    };
    let index = if number.is_finite() && number > 0.0 {
        number.floor().to_usize().unwrap_or(usize::MAX)
    } else {
        0
    };
    let Some(regexp) = state.regexp_mut(*regexp) else {
        return fail_dispatch(ctx);
    };
    regexp.last_index = index;
    *encoded
}

fn invoke_symbol_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    symbol: u32,
    args: &[i64],
) -> Option<i64> {
    if value::is_null(target) || value::is_undefined(target) {
        return None;
    }
    let key = value::encode_handle(value::TAG_SYMBOL, symbol);
    let method = match get_runtime_property(ctx, state, target, key) {
        Ok(method) => method,
        Err(()) => return Some(fail_dispatch(ctx)),
    };
    if value::is_null(method) || value::is_undefined(method) {
        return None;
    }
    if !value::is_callable(method) {
        return Some(type_error(
            ctx,
            state,
            "well-known symbol method is not callable",
        ));
    }
    Some(
        state
            .invoke_callable(ctx, method, target, args)
            .unwrap_or_else(|| fail_dispatch(ctx)),
    )
}

fn regexp_symbol_match(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [regexp, input] = args else {
        return fail_dispatch(ctx);
    };
    string_match_impl(ctx, state, &[*input, *regexp], false)
}

fn regexp_symbol_replace(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [regexp, input, replacement] = args else {
        return fail_dispatch(ctx);
    };
    string_replace_impl(ctx, state, &[*input, *regexp, *replacement], false, false)
}

fn regexp_symbol_search(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [regexp, input] = args else {
        return fail_dispatch(ctx);
    };
    string_search_impl(ctx, state, &[*input, *regexp], false)
}

fn regexp_symbol_split(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [regexp, input, rest @ ..] = args else {
        return fail_dispatch(ctx);
    };
    let limit = rest
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    string_split_impl(ctx, state, &[*input, *regexp, limit], false)
}

pub(super) fn string_match(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    string_match_impl(ctx, state, args, true)
}

fn string_match_impl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    invoke_protocol: bool,
) -> i64 {
    let [receiver, pattern] = args else {
        return fail_dispatch(ctx);
    };
    if invoke_protocol
        && let Some(result) =
            invoke_symbol_method(ctx, state, *pattern, wk_symbol::MATCH, &[*receiver])
    {
        return result;
    }
    let input = subject(state, *receiver);
    let regexp = if value::is_regexp(*pattern) {
        *pattern
    } else {
        let regexp = compile_regexp(ctx, state, subject(state, *pattern), String::new());
        if value::is_exception(regexp) {
            return regexp;
        }
        regexp
    };
    let global = state
        .regexp(regexp)
        .is_some_and(|entry| entry.flags.contains('g'));
    if !global {
        return exec(ctx, state, &[regexp, *receiver]);
    }
    if let Some(entry) = state.regexp_mut(regexp) {
        entry.last_index = 0;
    }
    let mut matches = Vec::new();
    while let Some(info) = execute_match(state, regexp, &input) {
        let Some(encoded) =
            state.intern_text(input[info.start..info.end].to_owned(), value::TAG_STRING)
        else {
            return fail_dispatch(ctx);
        };
        matches.push(encoded);
    }
    if matches.is_empty() {
        value::encode_null()
    } else {
        state
            .allocate_array_values_with_gc_retry(ctx, &matches)
            .unwrap_or_else(|_| fail_dispatch(ctx))
    }
}

pub(super) fn string_search(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    string_search_impl(ctx, state, args, true)
}

fn string_search_impl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    invoke_protocol: bool,
) -> i64 {
    let [receiver, pattern] = args else {
        return fail_dispatch(ctx);
    };
    if invoke_protocol
        && let Some(result) =
            invoke_symbol_method(ctx, state, *pattern, wk_symbol::SEARCH, &[*receiver])
    {
        return result;
    }
    let input = subject(state, *receiver);
    let regexp = if value::is_regexp(*pattern) {
        *pattern
    } else {
        let regexp = compile_regexp(ctx, state, subject(state, *pattern), String::new());
        if value::is_exception(regexp) {
            return regexp;
        }
        regexp
    };
    let previous = state.regexp(regexp).map_or(0, |entry| entry.last_index);
    if let Some(entry) = state.regexp_mut(regexp) {
        entry.last_index = 0;
    }
    let found = execute_match(state, regexp, &input);
    if let Some(entry) = state.regexp_mut(regexp) {
        entry.last_index = previous;
    }
    let Some(found) = found else {
        return value::encode_f64(-1.0);
    };
    let Ok(index) = u32::try_from(input[..found.start].encode_utf16().count()) else {
        return fail_dispatch(ctx);
    };
    value::encode_f64(f64::from(index))
}

pub(super) fn string_split(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    string_split_impl(ctx, state, args, true)
}

fn string_split_impl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    invoke_protocol: bool,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let separator = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let limit_arg = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    if invoke_protocol
        && let Some(result) = invoke_symbol_method(
            ctx,
            state,
            separator,
            wk_symbol::SPLIT,
            &[receiver, limit_arg],
        )
    {
        return result;
    }
    let limit = to_number(state, limit_arg)
        .and_then(|limit| limit.to_u32())
        .unwrap_or(u32::MAX);
    if limit == 0 {
        return state
            .allocate_array_values_with_gc_retry(ctx, &[])
            .unwrap_or_else(|_| fail_dispatch(ctx));
    }
    if value::is_regexp(separator) {
        let input = subject(state, receiver);
        let Some(entry) = state.regexp(separator) else {
            return fail_dispatch(ctx);
        };
        let ranges = entry
            .compiled
            .find_iter(&input)
            .map(|matched| matched.range())
            .collect::<Vec<_>>();
        let mut values = Vec::new();
        let mut start = 0;
        for range in ranges {
            if values.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                break;
            }
            let Some(part) =
                state.intern_text(input[start..range.start].to_owned(), value::TAG_STRING)
            else {
                return fail_dispatch(ctx);
            };
            values.push(part);
            start = range.end;
        }
        if values.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
            let Some(part) = state.intern_text(input[start..].to_owned(), value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            values.push(part);
        }
        return state
            .allocate_array_values_with_gc_retry(ctx, &values)
            .unwrap_or_else(|_| fail_dispatch(ctx));
    }
    if value::is_undefined(separator) {
        let input = subject_runtime_string(state, receiver);
        let Some(part) = state.intern_runtime_string(input, value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        return state
            .allocate_array_values_with_gc_retry(ctx, &[part])
            .unwrap_or_else(|_| fail_dispatch(ctx));
    }
    let input = subject_runtime_string(state, receiver);
    let separator = subject_runtime_string(state, separator);
    string_split_string_fast(ctx, state, input, separator, limit)
}

fn string_split_string_fast(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: wjsm_host::RuntimeString,
    separator: wjsm_host::RuntimeString,
    limit: u32,
) -> i64 {
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    if separator.is_empty() {
        return string_split_empty_separator(ctx, state, input, limit_usize);
    }
    if separator.utf16_len() == 1 {
        return string_split_single_unit(ctx, state, input, separator, limit_usize);
    }
    let mut values = Vec::new();
    let mut start = 0usize;
    let sep_len = separator.utf16_len();
    let mut search_from = 0usize;
    while values.len() + 1 < limit_usize {
        let Some(pos) = input.find_units(&separator, search_from) else {
            break;
        };
        let Some(part) =
            state.intern_runtime_string(input.slice_units(start..pos), value::TAG_STRING)
        else {
            return fail_dispatch(ctx);
        };
        values.push(part);
        start = pos + sep_len;
        search_from = start;
    }
    let Some(part) = state.intern_runtime_string(
        input.slice_units(start..input.utf16_len()),
        value::TAG_STRING,
    ) else {
        return fail_dispatch(ctx);
    };
    values.push(part);
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn string_split_empty_separator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: wjsm_host::RuntimeString,
    limit: usize,
) -> i64 {
    let total = input.utf16_len().min(limit);
    let mut values = Vec::with_capacity(total);
    for index in 0..total {
        let Some(part) =
            state.intern_runtime_string(input.slice_units(index..index + 1), value::TAG_STRING)
        else {
            return fail_dispatch(ctx);
        };
        values.push(part);
    }
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn string_split_single_unit(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: wjsm_host::RuntimeString,
    separator: wjsm_host::RuntimeString,
    limit: usize,
) -> i64 {
    let needle = separator.as_flat_slice()[0];
    let units = input.as_flat_slice();
    let mut values = smallvec::SmallVec::<[i64; 16]>::new();
    let mut start = 0usize;
    for (index, unit) in units.iter().copied().enumerate() {
        if unit != needle {
            continue;
        }
        let Some(part) = state.intern_utf16_slice(&units[start..index], value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        values.push(part);
        if values.len() == limit {
            return state
                .allocate_array_values_with_gc_retry(ctx, &values)
                .unwrap_or_else(|_| fail_dispatch(ctx));
        }
        start = index + 1;
    }
    let Some(part) = state.intern_utf16_slice(&units[start..], value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    values.push(part);
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

pub(super) fn string_replace(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    replace_all: bool,
) -> i64 {
    string_replace_impl(ctx, state, args, replace_all, true)
}

fn string_replace_impl(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    replace_all: bool,
    invoke_protocol: bool,
) -> i64 {
    let [receiver, pattern, replacement] = args else {
        return fail_dispatch(ctx);
    };
    if replace_all
        && value::is_regexp(*pattern)
        && !state
            .regexp(*pattern)
            .is_some_and(|regexp| regexp.flags.contains('g'))
    {
        return type_error(
            ctx,
            state,
            "String.prototype.replaceAll requires a global RegExp",
        );
    }
    if invoke_protocol
        && let Some(result) = invoke_symbol_method(
            ctx,
            state,
            *pattern,
            wk_symbol::REPLACE,
            &[*receiver, *replacement],
        )
    {
        return result;
    }
    let input = subject(state, *receiver);
    let (matches, global) = if value::is_regexp(*pattern) {
        let Some(entry) = state.regexp(*pattern) else {
            return fail_dispatch(ctx);
        };
        let global = entry.flags.contains('g');
        let matches = entry
            .compiled
            .find_iter(&input)
            .map(match_info)
            .collect::<Vec<_>>();
        (matches, global)
    } else {
        let pattern = subject(state, *pattern);
        let matches = input
            .match_indices(&pattern)
            .map(|(start, matched)| MatchInfo {
                start,
                end: start + matched.len(),
                captures: vec![Some(start..start + matched.len())],
                named_captures: Vec::new(),
            })
            .collect();
        (matches, false)
    };
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    for info in matches
        .iter()
        .take(if replace_all || global { usize::MAX } else { 1 })
    {
        output.push_str(&input[copied..info.start]);
        if value::is_callable(*replacement) {
            let mut callback_args = Vec::with_capacity(info.captures.len() + 3);
            for capture in &info.captures {
                let encoded = match capture {
                    Some(range) => state
                        .intern_text(input[range.clone()].to_owned(), value::TAG_STRING)
                        .unwrap_or_else(|| fail_dispatch(ctx)),
                    None => value::encode_undefined(),
                };
                callback_args.push(encoded);
            }
            let Ok(index) = u32::try_from(input[..info.start].encode_utf16().count()) else {
                return fail_dispatch(ctx);
            };
            callback_args.push(value::encode_f64(f64::from(index)));
            callback_args.push(*receiver);
            if !info.named_captures.is_empty() {
                let Some(groups) = build_named_groups(state, &input, info, false) else {
                    return fail_dispatch(ctx);
                };
                callback_args.push(groups);
            }
            let result = state
                .invoke_callable(ctx, *replacement, value::encode_undefined(), &callback_args)
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                return result;
            }
            output.push_str(&subject(state, result));
        } else {
            let replacement = subject(state, *replacement);
            output.push_str(&expand_replacement(&replacement, &input, info));
        }
        copied = info.end;
    }
    output.push_str(&input[copied..]);
    state
        .intern_text(output, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn expand_replacement(replacement: &str, input: &str, info: &MatchInfo) -> String {
    let mut output = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '$' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('$') => output.push('$'),
            Some('&') => output.push_str(&input[info.start..info.end]),
            Some('`') => output.push_str(&input[..info.start]),
            Some('\'') => output.push_str(&input[info.end..]),
            Some(digit @ '1'..='9') => {
                let index = digit
                    .to_digit(10)
                    .and_then(|index| usize::try_from(index).ok());
                if let Some(range) = index
                    .and_then(|index| info.captures.get(index))
                    .and_then(Clone::clone)
                {
                    output.push_str(&input[range]);
                }
            }
            Some('<') => {
                let mut name = String::new();
                let mut closed = false;
                for character in chars.by_ref() {
                    if character == '>' {
                        closed = true;
                        break;
                    }
                    name.push(character);
                }
                if closed && !info.named_captures.is_empty() {
                    if let Some(range) = info
                        .named_captures
                        .iter()
                        .find_map(|(candidate, range)| (candidate == &name).then_some(range))
                        .and_then(Clone::clone)
                    {
                        output.push_str(&input[range]);
                    }
                } else {
                    output.push_str("$<");
                    output.push_str(&name);
                    if closed {
                        output.push('>');
                    }
                }
            }
            Some(other) => {
                output.push('$');
                output.push(other);
            }
            None => output.push('$'),
        }
    }
    output
}

pub(crate) fn clone_parts(state: &NativeAgentState, encoded: i64) -> Option<(String, String)> {
    state
        .regexp(encoded)
        .map(|regexp| (regexp.pattern.clone(), regexp.flags.clone()))
}

pub(crate) fn from_parts(
    state: &mut NativeAgentState,
    pattern: String,
    flags: String,
) -> Option<i64> {
    state.create_regexp(pattern, flags).ok()
}
