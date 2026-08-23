use smallvec::SmallVec;
use wjsm_host::RuntimeString;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    PrimitiveHint, fail_dispatch, range_error, render_value, to_number, to_primitive,
    to_string_coerced, to_uint32, type_error,
};
use crate::NativeAgentState;

pub(super) fn dispatch_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::StringAt => string_at(ctx, state, args),
        Builtin::StringCharAt => string_char_at(ctx, state, args),
        Builtin::StringCharCodeAt => string_char_code_at(ctx, state, args),
        Builtin::StringCodePointAt => string_code_point_at(ctx, state, args),
        Builtin::StringConcatVa => string_concat(ctx, state, args),
        Builtin::StringBuilderAppend => string_builder_append(ctx, state, args),
        Builtin::StringBuilderFinish => string_builder_finish(ctx, state, args),
        Builtin::StringEndsWith => string_ends_with(ctx, state, args),
        Builtin::StringIncludes => string_includes(ctx, state, args),
        Builtin::StringIndexOf => string_index_of(ctx, state, args, false),
        Builtin::StringLastIndexOf => string_index_of(ctx, state, args, true),
        Builtin::StringPadEnd => string_pad(ctx, state, args, false),
        Builtin::StringPadStart => string_pad(ctx, state, args, true),
        Builtin::StringRepeat => string_repeat(ctx, state, args),
        Builtin::StringMatch => super::regexp::string_match(ctx, state, args),
        Builtin::StringMatchAll => super::regexp::string_match_all(ctx, state, args),
        Builtin::StringReplace => super::regexp::string_replace(ctx, state, args, false),
        Builtin::StringReplaceAll => super::regexp::string_replace(ctx, state, args, true),
        Builtin::StringSearch => super::regexp::string_search(ctx, state, args),
        Builtin::StringSlice => string_slice(ctx, state, args),
        Builtin::StringSplit => super::regexp::string_split(ctx, state, args),
        Builtin::StringStartsWith => string_starts_with(ctx, state, args),
        Builtin::StringNormalize => string_normalize(ctx, state, args),
        Builtin::StringSubstring => string_substring(ctx, state, args),
        Builtin::StringToLowerCase => string_case(ctx, state, args, false),
        Builtin::StringToUpperCase => string_case(ctx, state, args, true),
        Builtin::StringTrim => string_trim(ctx, state, args, true, true),
        Builtin::StringTrimEnd => string_trim(ctx, state, args, false, true),
        Builtin::StringTrimStart => string_trim(ctx, state, args, true, false),
        Builtin::StringToString | Builtin::StringValueOf => this_string_value(ctx, state, args),
        Builtin::StringFromCharCode => string_from_char_code(ctx, state, args),
        Builtin::StringFromCodePoint => string_from_code_point(ctx, state, args),
        _ => return None,
    })
}

fn this_string_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let receiver = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_string(receiver) {
        return receiver;
    }
    if value::is_js_object(receiver)
        && let Some(&primitive) = state.boxed_primitives.get(&value::decode_handle(receiver))
        && value::is_string(primitive)
    {
        return primitive;
    }
    type_error(
        ctx,
        state,
        "String.prototype.toString requires that 'this' be a String",
    )
}

fn string_normalize(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let receiver = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_null(receiver) || value::is_undefined(receiver) {
        return type_error(
            ctx,
            state,
            "String.prototype.normalize called on null or undefined",
        );
    }
    let primitive = match to_primitive(ctx, state, receiver, PrimitiveHint::String) {
        Ok(primitive) => primitive,
        Err(exception) => return exception,
    };
    if value::is_symbol(primitive) {
        return type_error(ctx, state, "Cannot convert a Symbol value to a string");
    }
    let text = if value::is_string(primitive) {
        let Some(text) = state.string_owned(primitive) else {
            return fail_dispatch(ctx);
        };
        text
    } else {
        RuntimeString::from(render_value(state, primitive))
    };
    let form = if args.get(1).is_none_or(|form| value::is_undefined(*form)) {
        "NFC".to_string()
    } else {
        match to_string_coerced(ctx, state, args[1]) {
            Ok(form) => form,
            Err(exception) => return exception,
        }
    };
    match wjsm_builtins::string_methods::normalize_runtime_string_by_form(&text, &form) {
        Ok(normalized) => intern(ctx, state, normalized),
        Err(message) => range_error(ctx, state, message),
    }
}

fn runtime_string(state: &NativeAgentState, value: i64) -> Option<RuntimeString> {
    if value::is_string(value) || value::is_bigint(value) {
        state.string_owned(value)
    } else if value::is_symbol(value) {
        None
    } else {
        Some(RuntimeString::from(render_value(state, value)))
    }
}

/// 发布字符串，zgc 分配需推进 GC 时收集后重试（实现见
/// [`super::runtime::intern_string_with_gc_retry`]）。
pub(super) fn intern(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    string: RuntimeString,
) -> i64 {
    super::runtime::intern_string_with_gc_retry(ctx, state, string)
}

fn integer(state: &NativeAgentState, value: Option<i64>, default: i64) -> Option<i64> {
    let Some(value) = value else {
        return Some(default);
    };
    let number = to_number(state, value)?;
    if number.is_nan() || number == 0.0 {
        Some(0)
    } else if number >= i64::MAX as f64 {
        Some(i64::MAX)
    } else if number <= i64::MIN as f64 {
        Some(i64::MIN)
    } else {
        Some(number.trunc() as i64)
    }
}

fn receiver(state: &NativeAgentState, args: &[i64]) -> Option<RuntimeString> {
    runtime_string(state, *args.first()?)
}
fn string_at(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(encoded) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(mut index) = integer(state, args.get(1).copied(), 0) else {
        return fail_dispatch(ctx);
    };
    let length = state.string_len(encoded).unwrap_or(0) as i64;
    if index < 0 {
        index += length;
    }
    let Ok(index) = usize::try_from(index) else {
        return value::encode_undefined();
    };
    state
        .string_code_unit(encoded, index)
        .map(RuntimeString::from_utf16_code_unit)
        .map(|result| intern(ctx, state, result))
        .unwrap_or_else(value::encode_undefined)
}

fn string_char_at(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(index) =
        integer(state, args.get(1).copied(), 0).and_then(|index| usize::try_from(index).ok())
    else {
        return intern(ctx, state, RuntimeString::empty());
    };
    let result = text
        .code_unit_at(index)
        .map(RuntimeString::from_utf16_code_unit)
        .unwrap_or_default();
    intern(ctx, state, result)
}

fn string_char_code_at(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let code = integer(state, args.get(1).copied(), 0)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| text.code_unit_at(index))
        .map_or(f64::NAN, f64::from);
    value::encode_f64(code)
}

fn string_code_point_at(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    integer(state, args.get(1).copied(), 0)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| text.code_point_at(index))
        .map(|code_point| value::encode_f64(f64::from(code_point)))
        .unwrap_or_else(value::encode_undefined)
}

enum BuilderPart {
    Number(f64),
    Static(&'static str),
    String(RuntimeString),
}

impl BuilderPart {
    fn from_value(state: &NativeAgentState, encoded: i64) -> Option<Self> {
        if value::is_string(encoded) || value::is_bigint(encoded) {
            state.string_owned(encoded).map(Self::String).or_else(|| {
                Some(Self::String(RuntimeString::from(render_value(
                    state, encoded,
                ))))
            })
        } else if value::is_f64(encoded) {
            Some(Self::Number(value::decode_f64(encoded)))
        } else if value::is_bool(encoded) {
            Some(Self::Static(if value::decode_bool(encoded) {
                "true"
            } else {
                "false"
            }))
        } else if value::is_null(encoded) {
            Some(Self::Static("null"))
        } else if value::is_undefined(encoded) {
            Some(Self::Static("undefined"))
        } else if value::is_symbol(encoded) {
            None
        } else {
            Some(Self::String(RuntimeString::from(render_value(
                state, encoded,
            ))))
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Self::Number(_) => 24,
            Self::Static(text) => text.len(),
            Self::String(text) => text.utf16_len(),
        }
    }

    fn append_to(&self, builder: &mut RuntimeString) -> bool {
        match self {
            Self::Number(number) => builder.append_builder_number(*number),
            Self::Static(text) => builder.append_builder_utf8(text),
            Self::String(text) => builder.append_builder(text),
        }
    }
}
fn append_builder_parts(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    current: i64,
    parts: &[BuilderPart],
) -> Option<()> {
    if !state.string_is_builder(current) {
        return None;
    }
    let handle = value::decode_handle(current);
    let cur_len = state.gc.heap().string_length(handle).ok()? as usize;
    let added = parts.iter().map(BuilderPart::capacity).sum::<usize>();
    let needed = cur_len.checked_add(added)?;
    let byte_capacity = u32::try_from(needed.checked_mul(2)?.checked_add(7)? & !7).ok()?;
    let capacity = state.gc.heap().string_capacity(handle).ok()?;
    if capacity < byte_capacity
        && state
            .gc
            .heap()
            .grow_string_capacity(handle, byte_capacity)
            .is_err()
    {
        state.temporary_roots.push(current);
        let _ = state.collect_garbage(ctx);
        let _ = state.gc.heap().finish_relocation_epoch();
        for _ in 0..3 {
            let _ = state.gc.heap().advance_epoch_and_reclaim();
        }
        let res = state.gc.heap().grow_string_capacity(handle, byte_capacity);
        state.temporary_roots.pop();
        res.ok()?;
    }
    let mut append_bytes = Vec::with_capacity(added * 2);
    for part in parts {
        match part {
            BuilderPart::Number(number) => {
                let text = RuntimeString::from_number(*number);
                for unit in text.as_flat_slice() {
                    append_bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
            BuilderPart::Static(text) => {
                for unit in text.encode_utf16() {
                    append_bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
            BuilderPart::String(text) => {
                for unit in text.as_flat_slice() {
                    append_bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
        }
    }
    let write_offset = u32::try_from(cur_len.checked_mul(2)?).ok()?;
    state
        .gc
        .heap()
        .write_string_payload(handle, write_offset, &append_bytes)
        .ok()?;
    let final_len = cur_len.checked_add(append_bytes.len() / 2)?;
    state
        .gc
        .heap()
        .set_string_length(handle, u32::try_from(final_len).ok()?)
        .ok()?;
    Some(())
}

pub(super) fn string_builder_append_direct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    current: i64,
    first: i64,
    second: i64,
) -> i64 {
    string_builder_append(ctx, state, &[current, first, second])
}

pub(super) fn string_builder_append_number_direct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    current: i64,
    first: i64,
    second: f64,
) -> i64 {
    string_builder_append(ctx, state, &[current, first, value::encode_f64(second)])
}

pub(super) fn string_builder_append(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some((current, rest)) = args.split_first() else {
        return fail_dispatch(ctx);
    };
    let is_builder = state.string_is_builder(*current);
    if is_builder {
        let mut parts = SmallVec::<[BuilderPart; 4]>::new();
        for encoded in rest {
            let Some(part) = BuilderPart::from_value(state, *encoded) else {
                return fail_dispatch(ctx);
            };
            parts.push(part);
        }
        if append_builder_parts(ctx, state, *current, &parts).is_some() {
            return *current;
        }
        let Some(mut rebuilt) = state.string_owned(*current) else {
            return fail_dispatch(ctx);
        };
        for part in &parts {
            if !part.append_to(&mut rebuilt) {
                return fail_dispatch(ctx);
            }
        }
        return intern(ctx, state, rebuilt);
    }
    let Some(prefix) = BuilderPart::from_value(state, *current) else {
        return fail_dispatch(ctx);
    };
    let mut parts = SmallVec::<[BuilderPart; 4]>::new();
    parts.push(prefix);
    for encoded in rest {
        let Some(part) = BuilderPart::from_value(state, *encoded) else {
            return fail_dispatch(ctx);
        };
        parts.push(part);
    }
    const MIN_BUILDER_INITIAL_CAPACITY: usize = 16384;
    let needed: usize = parts.iter().map(BuilderPart::capacity).sum();
    let capacity = needed.max(MIN_BUILDER_INITIAL_CAPACITY);
    let mut builder = RuntimeString::builder(capacity);
    for part in &parts {
        if !part.append_to(&mut builder) {
            return fail_dispatch(ctx);
        }
    }
    intern(ctx, state, builder)
}

pub(super) fn string_builder_finish(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [encoded] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_string(*encoded) {
        return fail_dispatch(ctx);
    }
    let handle = value::decode_handle(*encoded);
    if state.gc.heap().freeze_builder_in_place(handle).is_ok() {
        *encoded
    } else {
        fail_dispatch(ctx)
    }
}

fn string_concat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some((first, rest)) = args.split_first() else {
        return fail_dispatch(ctx);
    };
    if rest.is_empty() {
        return *first;
    }
    // 快速探测：如果所有参数总长度 <= 128 且均为 Latin-1，走栈缓冲单次分配发布
    let mut stack_buf = [0u8; 128];
    let mut stack_len = 0usize;
    let mut all_latin1_small = true;

    for &arg in args {
        if value::is_string(arg) {
            let handle = value::decode_handle(arg);
            let len = state.gc.heap().string_length(handle).unwrap_or(0) as usize;
            if stack_len + len > 128 {
                all_latin1_small = false;
                break;
            }
            if len == 0 {
                continue;
            }
            let is_latin1 = state
                .with_string_bytes(arg, |view| {
                    if let Some(bytes) = view.as_latin1() {
                        stack_buf[stack_len..stack_len + len].copy_from_slice(bytes);
                        stack_len += len;
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !is_latin1 {
                all_latin1_small = false;
                break;
            }
        } else if value::is_f64(arg) {
            let num = value::decode_f64(arg);
            if (0.0..=1_000_000_000.0).contains(&num) && num.fract() == 0.0 {
                let mut temp = [0u8; 20];
                let mut val = num as u64;
                let mut i = 0;
                if val == 0 {
                    temp[0] = b'0';
                    i = 1;
                } else {
                    while val > 0 {
                        temp[i] = b'0' + (val % 10) as u8;
                        val /= 10;
                        i += 1;
                    }
                    temp[..i].reverse();
                }
                if stack_len + i > 128 {
                    all_latin1_small = false;
                    break;
                }
                stack_buf[stack_len..stack_len + i].copy_from_slice(&temp[..i]);
                stack_len += i;
            } else {
                all_latin1_small = false;
                break;
            }
        } else {
            all_latin1_small = false;
            break;
        }
    }

    if all_latin1_small
        && let Some(encoded) =
            state.publish_string_bytes(&stack_buf[..stack_len], value::TAG_STRING, false)
    {
        state.gc.record_host_write(encoded, None, Some(encoded));
        return encoded;
    }

    let mut current = *first;
    if !value::is_string(current) {
        let Some(s) = runtime_string(state, current) else {
            return fail_dispatch(ctx);
        };
        current = intern(ctx, state, s);
    }
    for &argument in rest {
        let arg = if value::is_string(argument) {
            argument
        } else {
            let Some(s) = runtime_string(state, argument) else {
                return fail_dispatch(ctx);
            };
            intern(ctx, state, s)
        };
        let len1 = state.string_len(current).unwrap_or(0);
        let len2 = state.string_len(arg).unwrap_or(0);
        if len1 == 0 {
            current = arg;
            continue;
        }
        if len2 == 0 {
            continue;
        }
        let total_len = (len1 + len2) as u32;
        let h1 = value::decode_handle(current);
        let h2 = value::decode_handle(arg);
        if let Some(encoded) =
            state.publish_cons_string_with_gc_retry(ctx, h1, h2, total_len, value::TAG_STRING)
        {
            state.gc.record_host_write(encoded, None, Some(encoded));
            current = encoded;
        } else {
            return fail_dispatch(ctx);
        }
    }
    current
}

fn string_includes(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (Some(text), Some(search)) = (
        receiver(state, args),
        args.get(1).and_then(|value| runtime_string(state, *value)),
    ) else {
        return fail_dispatch(ctx);
    };
    let Some(from) = integer(state, args.get(2).copied(), 0).map(|from| from.max(0) as usize)
    else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(text.find_units(&search, from).is_some())
}

fn string_index_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let (Some(text), Some(search)) = (
        receiver(state, args),
        args.get(1).and_then(|value| runtime_string(state, *value)),
    ) else {
        return fail_dispatch(ctx);
    };
    let index = if reverse {
        let end = integer(state, args.get(2).copied(), text.utf16_len() as i64)
            .unwrap_or(text.utf16_len() as i64)
            .clamp(0, text.utf16_len() as i64) as usize;
        text.rfind_units_before(&search, end.saturating_add(search.utf16_len()))
    } else {
        let from = integer(state, args.get(2).copied(), 0)
            .unwrap_or(0)
            .clamp(0, text.utf16_len() as i64) as usize;
        text.find_units(&search, from)
    };
    value::encode_f64(index.map_or(-1.0, |index| index as f64))
}

fn string_starts_with(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let (Some(text), Some(search)) = (
        receiver(state, args),
        args.get(1).and_then(|value| runtime_string(state, *value)),
    ) else {
        return fail_dispatch(ctx);
    };
    let from = integer(state, args.get(2).copied(), 0)
        .unwrap_or(0)
        .clamp(0, text.utf16_len() as i64) as usize;
    value::encode_bool(text.starts_with_units(&search, from))
}

fn string_ends_with(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (Some(text), Some(search)) = (
        receiver(state, args),
        args.get(1).and_then(|value| runtime_string(state, *value)),
    ) else {
        return fail_dispatch(ctx);
    };
    let end = integer(state, args.get(2).copied(), text.utf16_len() as i64)
        .unwrap_or(text.utf16_len() as i64)
        .clamp(0, text.utf16_len() as i64) as usize;
    value::encode_bool(text.ends_with_units(&search, end))
}

fn string_pad(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    start: bool,
) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(target) =
        integer(state, args.get(1).copied(), 0).and_then(|target| usize::try_from(target).ok())
    else {
        return fail_dispatch(ctx);
    };
    if target <= text.utf16_len() {
        return intern(ctx, state, text);
    }
    let fill = args
        .get(2)
        .and_then(|fill| runtime_string(state, *fill))
        .unwrap_or_else(|| RuntimeString::from(" "));
    if fill.is_empty() {
        return intern(ctx, state, text);
    }
    let needed = target - text.utf16_len();
    let fill_flat = fill.as_flat_slice();
    let mut padding = Vec::with_capacity(needed);
    while padding.len() < needed {
        let remaining = needed - padding.len();
        padding.extend_from_slice(&fill_flat[..remaining.min(fill_flat.len())]);
    }
    let padding_rs = RuntimeString::from_utf16_units(padding);
    let result = if start {
        RuntimeString::concat(padding_rs, text)
    } else {
        RuntimeString::concat(text, padding_rs)
    };
    intern(ctx, state, result)
}

fn string_repeat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(count) =
        integer(state, args.get(1).copied(), 0).and_then(|count| usize::try_from(count).ok())
    else {
        return fail_dispatch(ctx);
    };
    let Some(total) = text.utf16_len().checked_mul(count) else {
        return fail_dispatch(ctx);
    };
    if total > u32::MAX as usize {
        return fail_dispatch(ctx);
    }
    intern(ctx, state, text.repeat(count))
}

fn string_slice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let first = *args.first().unwrap_or(&value::encode_undefined());
    if !value::is_string(first) && !value::is_bigint(first) {
        let Some(text) = runtime_string(state, first) else {
            return fail_dispatch(ctx);
        };
        let length = text.utf16_len() as i64;
        let start = relative_index(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
        let end = relative_index(
            integer(state, args.get(2).copied(), length).unwrap_or(length),
            length,
        );
        let range = if end < start {
            start..start
        } else {
            start..end
        };
        if range.start >= range.end {
            return intern(ctx, state, RuntimeString::empty());
        }
        return intern(ctx, state, text.slice_units(range));
    }
    let Some(length) = state.string_len(first) else {
        return fail_dispatch(ctx);
    };
    let length = length as i64;
    let start = relative_index(integer(state, args.get(1).copied(), 0).unwrap_or(0), length);
    let end = relative_index(
        integer(state, args.get(2).copied(), length).unwrap_or(length),
        length,
    );
    let range = if end < start {
        start..start
    } else {
        start..end
    };
    if range.start >= range.end {
        return intern(ctx, state, RuntimeString::empty());
    }
    let base_handle = value::decode_handle(first);
    let slice_len = (range.end - range.start) as u32;
    let (actual_base, actual_start, actual_end) =
        if let Ok(Some((orig_base, orig_start, _orig_end))) =
            state.gc.heap().slice_parts(base_handle)
        {
            (
                orig_base,
                orig_start + range.start as u32,
                orig_start + range.end as u32,
            )
        } else {
            (base_handle, range.start as u32, range.end as u32)
        };
    if let Some(encoded) = state.publish_slice_string_with_gc_retry(
        ctx,
        actual_base,
        actual_start,
        actual_end,
        slice_len,
        value::TAG_STRING,
    ) {
        state.gc.record_host_write(encoded, None, Some(encoded));
        return encoded;
    }
    fail_dispatch(ctx)
}

fn string_substring(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let first = *args.first().unwrap_or(&value::encode_undefined());
    let (length, text_opt) = if value::is_string(first) || value::is_bigint(first) {
        (state.string_len(first), None)
    } else {
        let Some(text) = runtime_string(state, first) else {
            return fail_dispatch(ctx);
        };
        (Some(text.utf16_len()), Some(text))
    };
    let Some(length) = length else {
        return fail_dispatch(ctx);
    };
    let mut start = integer(state, args.get(1).copied(), 0)
        .unwrap_or(0)
        .clamp(0, length as i64) as usize;
    let mut end = integer(state, args.get(2).copied(), length as i64)
        .unwrap_or(length as i64)
        .clamp(0, length as i64) as usize;
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    if start >= end {
        return intern(ctx, state, RuntimeString::empty());
    }
    if let Some(text) = text_opt {
        return intern(ctx, state, text.slice_units(start..end));
    }
    let base_handle = value::decode_handle(first);
    let slice_len = (end - start) as u32;
    let (actual_base, actual_start, actual_end) =
        if let Ok(Some((orig_base, orig_start, _orig_end))) =
            state.gc.heap().slice_parts(base_handle)
        {
            (
                orig_base,
                orig_start + start as u32,
                orig_start + end as u32,
            )
        } else {
            (base_handle, start as u32, end as u32)
        };
    if let Some(encoded) = state.publish_slice_string_with_gc_retry(
        ctx,
        actual_base,
        actual_start,
        actual_end,
        slice_len,
        value::TAG_STRING,
    ) {
        state.gc.record_host_write(encoded, None, Some(encoded));
        return encoded;
    }
    fail_dispatch(ctx)
}

fn relative_index(index: i64, length: i64) -> usize {
    if index < 0 {
        (length + index).max(0) as usize
    } else {
        index.min(length) as usize
    }
}

fn string_case(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    uppercase: bool,
) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(text) = text.to_utf8() else {
        return intern(ctx, state, text);
    };
    let converted = if uppercase {
        text.to_uppercase()
    } else {
        text.to_lowercase()
    };
    intern(ctx, state, RuntimeString::from(converted))
}

fn string_trim(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    start: bool,
    end: bool,
) -> i64 {
    let Some(text) = receiver(state, args) else {
        return fail_dispatch(ctx);
    };
    let flat = text.as_flat_slice();
    let mut first = 0;
    let mut last = flat.len();
    if start {
        while first < last && is_trim_unit(flat[first]) {
            first += 1;
        }
    }
    if end {
        while last > first && is_trim_unit(flat[last - 1]) {
            last -= 1;
        }
    }
    intern(ctx, state, text.slice_units(first..last))
}

fn is_trim_unit(unit: u16) -> bool {
    unit == 0xFEFF
        || char::from_u32(u32::from(unit)).is_some_and(|character| character.is_whitespace())
}

fn string_from_char_code(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let mut units = Vec::with_capacity(args.len());
    for argument in args {
        let Some(number) = to_number(state, *argument) else {
            return fail_dispatch(ctx);
        };
        units.push(to_uint32(number) as u16);
    }
    intern(ctx, state, RuntimeString::from_utf16_units(units))
}

fn string_from_code_point(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let mut units = Vec::with_capacity(args.len());
    for argument in args {
        let Some(number) = to_number(state, *argument) else {
            return fail_dispatch(ctx);
        };
        if !number.is_finite()
            || number.fract() != 0.0
            || !(0.0..=0x10FFFF as f64).contains(&number)
        {
            return range_error(ctx, state, "Invalid code point");
        }
        let code_point = number as u32;
        if code_point <= 0xFFFF {
            units.push(code_point as u16);
        } else {
            let offset = code_point - 0x1_0000;
            units.push(0xD800 | (offset >> 10) as u16);
            units.push(0xDC00 | (offset & 0x3FF) as u16);
        }
    }
    intern(ctx, state, RuntimeString::from_utf16_units(units))
}
