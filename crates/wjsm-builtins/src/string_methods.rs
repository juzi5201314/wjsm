//! String.prototype 方法（同步路径）。

use wjsm_host::{ExecContext, RuntimeString, Value};
use wjsm_intl_data::{NormalizationForm, normalize};
use wjsm_ir::value;

pub const INVALID_NORMALIZATION_FORM_MESSAGE: &str =
    "The normalization form should be one of NFC, NFD, NFKC, NFKD";
fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}
fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}
fn decode_surrogate_pair(high: u16, low: u16) -> u32 {
    0x10000 + (((high as u32 - 0xD800) << 10) | (low as u32 - 0xDC00))
}

fn normalize_string_by_form(s: &str, form: &str) -> Result<String, &'static str> {
    let form = NormalizationForm::parse(form).map_err(|_| INVALID_NORMALIZATION_FORM_MESSAGE)?;
    Ok(normalize(s, form))
}

fn flush_transformed_run<F>(out: &mut Vec<u16>, run: &mut String, transform: &mut F)
where
    F: FnMut(&str) -> String,
{
    if run.is_empty() {
        return;
    }
    out.extend(transform(run).encode_utf16());
    run.clear();
}

fn transform_scalar_runs<F>(input: &RuntimeString, mut transform: F) -> RuntimeString
where
    F: FnMut(&str) -> String,
{
    let units = input.as_flat_slice();
    let mut out = Vec::with_capacity(units.len());
    let mut run = String::new();
    let mut i = 0usize;
    while i < units.len() {
        let unit = units[i];
        if is_high_surrogate(unit) && i + 1 < units.len() && is_low_surrogate(units[i + 1]) {
            let cp = decode_surrogate_pair(unit, units[i + 1]);
            run.push(char::from_u32(cp).expect("valid surrogate pair"));
            i += 2;
            continue;
        }
        if is_high_surrogate(unit) || is_low_surrogate(unit) {
            flush_transformed_run(&mut out, &mut run, &mut transform);
            out.push(unit);
            i += 1;
            continue;
        }
        run.push(char::from_u32(unit as u32).expect("valid BMP scalar"));
        i += 1;
    }
    flush_transformed_run(&mut out, &mut run, &mut transform);
    RuntimeString::from_utf16_units(out)
}

pub fn normalize_runtime_string_by_form(
    input: &RuntimeString,
    form: &str,
) -> Result<RuntimeString, &'static str> {
    let mut error = None;
    let normalized =
        transform_scalar_runs(input, |run| match normalize_string_by_form(run, form) {
            Ok(out) => out,
            Err(msg) => {
                error = Some(msg);
                run.to_string()
            }
        });
    match error {
        Some(msg) => Err(msg),
        None => Ok(normalized),
    }
}

fn code_point_width_at(units: &[u16], index: usize) -> Option<(u32, usize, bool)> {
    let unit = *units.get(index)?;
    if is_high_surrogate(unit) && index + 1 < units.len() && is_low_surrogate(units[index + 1]) {
        return Some((decode_surrogate_pair(unit, units[index + 1]), 2, true));
    }
    Some((
        unit as u32,
        1,
        !(is_high_surrogate(unit) || is_low_surrogate(unit)),
    ))
}

fn previous_code_point_width(units: &[u16], end: usize) -> Option<(usize, u32, usize, bool)> {
    if end == 0 || end > units.len() {
        return None;
    }
    let last = units[end - 1];
    if is_low_surrogate(last) && end >= 2 && is_high_surrogate(units[end - 2]) {
        let start = end - 2;
        return Some((start, decode_surrogate_pair(units[start], last), 2, true));
    }
    Some((
        end - 1,
        last as u32,
        1,
        !(is_high_surrogate(last) || is_low_surrogate(last)),
    ))
}

fn is_ecmascript_trim_whitespace(cp: u32) -> bool {
    cp == 0xFEFF || char::from_u32(cp).is_some_and(char::is_whitespace)
}

fn trim_runtime_string(input: &RuntimeString, trim_start: bool, trim_end: bool) -> RuntimeString {
    let units = input.as_flat_slice();
    let mut start = 0usize;
    let mut end = units.len();
    if trim_start {
        while start < end {
            let Some((cp, width, scalar)) = code_point_width_at(units, start) else {
                break;
            };
            if !scalar || !is_ecmascript_trim_whitespace(cp) {
                break;
            }
            start += width;
        }
    }
    if trim_end {
        while start < end {
            let Some((cp_start, cp, _width, scalar)) = previous_code_point_width(units, end)
            else {
                break;
            };
            if !scalar || !is_ecmascript_trim_whitespace(cp) {
                break;
            }
            end = cp_start;
        }
    }
    input.slice_units(start..end)
}

fn repeat_units_to_len(source: &RuntimeString, len: usize) -> RuntimeString {
    let source_units = source.as_flat_slice();
    if len == 0 || source_units.is_empty() {
        return RuntimeString::empty();
    }
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let remaining = len - out.len();
        let take = remaining.min(source_units.len());
        out.extend_from_slice(&source_units[..take]);
    }
    RuntimeString::from_utf16_units(out)
}

/// 供 native host string_replace_all 同步路径复用。
pub fn replace_all_units(
    haystack: &RuntimeString,
    search: &RuntimeString,
    replacement: &RuntimeString,
) -> RuntimeString {
    let replacement_flat = replacement.as_flat_slice();
    let haystack_flat = haystack.as_flat_slice();
    let mut out = Vec::new();
    if search.is_empty() {
        out.extend_from_slice(replacement_flat);
        for unit in haystack_flat {
            out.push(*unit);
            out.extend_from_slice(replacement_flat);
        }
        return RuntimeString::from_utf16_units(out);
    }
    let search_len = search.utf16_len();
    let mut pos = 0usize;
    while let Some(found) = haystack.find_units(search, pos) {
        out.extend_from_slice(&haystack_flat[pos..found]);
        out.extend_from_slice(replacement_flat);
        pos = found + search_len;
    }
    out.extend_from_slice(&haystack_flat[pos..]);
    RuntimeString::from_utf16_units(out)
}

fn runtime_string_from_code_point(cp: u32) -> RuntimeString {
    let Some(ch) = char::from_u32(cp) else {
        return RuntimeString::empty();
    };
    let mut buf = [0u16; 2];
    RuntimeString::from_utf16_units(ch.encode_utf16(&mut buf).to_vec())
}

fn to_f64_or(val: Value, default: f64) -> f64 {
    if value::is_f64(val) {
        value::decode_f64(val)
    } else {
        default
    }
}

fn to_uint16_number(n: f64) -> u16 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(65536.0) as u16
}

fn to_uint32_number(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(4294967296.0) as u32
}

fn to_uint16_ctx<E: ExecContext>(ctx: &mut E, val: Value) -> u16 {
    to_uint16_number(value::decode_f64(ctx.to_number(val)))
}

fn to_uint32_ctx<E: ExecContext>(ctx: &mut E, val: Value) -> u32 {
    to_uint32_number(value::decode_f64(ctx.to_number(val)))
}

fn concat_arg_to_string<E: ExecContext>(ctx: &mut E, val: Value) -> RuntimeString {
    if value::is_string(val) {
        ctx.get_runtime_string(val)
    } else {
        RuntimeString::from_utf8_str(&ctx.render_value(val))
    }
}

fn is_valid_code_point(cp: u32) -> bool {
    cp <= 0x10FFFF && !(0xD800..=0xDFFF).contains(&cp)
}

/// 原始字符串属性读取（length + 整数下标 StringGet + includes/startsWith/indexOf）。
pub fn primitive_string_get_property<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    name_id: u32,
) -> Value {
    if ctx.name_id_matches(name_id, "length") {
        let len = ctx.get_runtime_string(receiver).utf16_len();
        return value::encode_f64(len as f64);
    }
    // ECMAScript §10.4.3 StringGet：规范数字索引键返回该位置的 UTF-16 code unit。
    if let Some(index) = canonical_string_index_key(ctx, name_id) {
        let s = ctx.get_runtime_string(receiver);
        let Some(unit) = s.code_unit_at(index) else {
            return value::encode_undefined();
        };
        return ctx.store_runtime_string(RuntimeString::from_utf16_code_unit(unit));
    }
    let method = if ctx.name_id_matches(name_id, "includes") {
        0
    } else if ctx.name_id_matches(name_id, "startsWith") {
        1
    } else if ctx.name_id_matches(name_id, "indexOf") {
        2
    } else if ctx.name_id_matches(name_id, "slice") {
        3
    } else if ctx.name_id_matches(name_id, "concat") {
        4
    } else {
        return value::encode_undefined();
    };
    ctx.create_string_primitive_method(method)
}

/// name_id → 规范数字索引（CanonicalNumericIndexString 的整数形态）。
/// 仅接受纯 ASCII 数字且无前导零的键（"0"/"12"）；"007"/"1.5"/"-1"/symbol 等返回 None，
/// 交给后续方法分派（String.prototype 上无此类属性，结果仍为 undefined）。
fn canonical_string_index_key<E: ExecContext>(ctx: &mut E, name_id: u32) -> Option<usize> {
    let key = ctx.name_id_to_property_key_value(name_id)?;
    if !value::is_string(key) {
        return None;
    }
    let text = ctx.value_to_key_string(key).ok()?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse::<usize>().ok()
}

pub fn string_at<E: ExecContext>(ctx: &mut E, receiver: Value, index: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let len = s.utf16_len() as i64;
    let idx = to_f64_or(index, 0.0);
    let mut i = idx as i64;
    if idx < 0.0 {
        i += len;
    }
    if i < 0 || i >= len {
        return value::encode_undefined();
    }
    ctx.store_runtime_string(RuntimeString::from_utf16_code_unit(
        s.code_unit_at(i as usize).unwrap_or(0),
    ))
}

pub fn string_char_at<E: ExecContext>(ctx: &mut E, receiver: Value, pos: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let p = to_uint32_ctx(ctx, pos) as usize;
    let Some(unit) = s.code_unit_at(p) else {
        return ctx.store_runtime_string(RuntimeString::empty());
    };
    ctx.store_runtime_string(RuntimeString::from_utf16_code_unit(unit))
}

pub fn string_char_code_at<E: ExecContext>(ctx: &mut E, receiver: Value, pos: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let p = to_uint32_ctx(ctx, pos) as usize;
    s.code_unit_at(p)
        .map(|unit| value::encode_f64(unit as f64))
        .unwrap_or_else(|| value::encode_f64(f64::NAN))
}

pub fn string_code_point_at<E: ExecContext>(ctx: &mut E, receiver: Value, pos: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let p = to_uint32_ctx(ctx, pos) as usize;
    s.code_point_at(p)
        .map(|cp| value::encode_f64(cp as f64))
        .unwrap_or_else(value::encode_undefined)
}

pub fn string_proto_concat<E: ExecContext>(
    ctx: &mut E,
    this_val: Value,
    args_base: i32,
    args_count: i32,
) -> Value {
    if value::is_array(this_val) {
        return crate::array_object::array_concat_args(ctx, this_val, args_base, args_count);
    }
    let mut result = ctx.get_runtime_string(this_val);
    for i in 0..args_count as u32 {
        let arg = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            i,
        );
        let part = concat_arg_to_string(ctx, arg);
        result.push_units_from(&part);
    }
    ctx.store_runtime_string(result)
}

pub fn string_ends_with<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    end_pos: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let search_str = ctx.get_runtime_string(search);
    let len = s.utf16_len();
    let end_utf16 = if end_pos == value::encode_undefined() {
        len
    } else {
        (to_f64_or(end_pos, 0.0) as usize).min(len)
    };
    value::encode_bool(s.ends_with_units(&search_str, end_utf16))
}

pub fn string_includes<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    pos: Value,
) -> Value {
    if value::is_array(receiver) && ctx.resolve_array(receiver) {
        return crate::timers_arrays::arr_includes_from(ctx, receiver, search, pos);
    }
    let s = ctx.get_runtime_string(receiver);
    let start_utf16 = if pos == value::encode_undefined() {
        0
    } else {
        (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
    };
    let search_str = ctx.get_runtime_string(search);
    value::encode_bool(s.find_units(&search_str, start_utf16).is_some())
}

pub fn string_index_of<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    pos: Value,
) -> Value {
    if value::is_array(receiver) && ctx.resolve_array(receiver) {
        return crate::timers_arrays::arr_index_of(ctx, receiver, search, pos);
    }
    let s = ctx.get_runtime_string(receiver);
    let search_str = ctx.get_runtime_string(search);
    let start_utf16 = if pos == value::encode_undefined() {
        0
    } else {
        (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
    };
    s.find_units(&search_str, start_utf16)
        .map(|index| value::encode_f64(index as f64))
        .unwrap_or_else(|| value::encode_f64(-1.0))
}

pub fn string_last_index_of<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    pos: Value,
) -> Value {
    if value::is_array(receiver) && ctx.resolve_array(receiver) {
        let len = ctx.array_read_length(receiver).unwrap_or(0);
        let from_index = if pos == value::encode_undefined() {
            value::encode_f64((len as i64 - 1) as f64)
        } else {
            pos
        };
        return crate::timers_arrays::arr_last_index_of(ctx, receiver, search, from_index);
    }
    let s = ctx.get_runtime_string(receiver);
    let search_str = ctx.get_runtime_string(search);
    let len = s.utf16_len();
    let from = if pos == value::encode_undefined() {
        len
    } else {
        (to_f64_or(pos, 0.0) as usize).min(len)
    };
    if search_str.is_empty() {
        return value::encode_f64(from as f64);
    }
    let end = from.saturating_add(search_str.utf16_len()).min(len);
    s.rfind_units_before(&search_str, end)
        .map(|index| value::encode_f64(index as f64))
        .unwrap_or_else(|| value::encode_f64(-1.0))
}

pub fn string_pad_end<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    target_len: Value,
    pad_str_val: Value,
) -> Value {
    let mut s = ctx.get_runtime_string(receiver);
    let len = s.utf16_len();
    let target = if value::is_f64(target_len) {
        to_f64_or(target_len, 0.0) as usize
    } else {
        0
    };
    if target <= len {
        return ctx.store_runtime_string(s);
    }
    let pad = if pad_str_val == value::encode_undefined() {
        RuntimeString::from_utf8_str(" ")
    } else {
        let p = ctx.get_runtime_string(pad_str_val);
        if p.is_empty() {
            RuntimeString::from_utf8_str(" ")
        } else {
            p
        }
    };
    s.push_units_from(&repeat_units_to_len(&pad, target - len));
    ctx.store_runtime_string(s)
}

pub fn string_pad_start<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    target_len: Value,
    pad_str_val: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let len = s.utf16_len();
    let target = if value::is_f64(target_len) {
        to_f64_or(target_len, 0.0) as usize
    } else {
        0
    };
    if target <= len {
        return ctx.store_runtime_string(s);
    }
    let pad = if pad_str_val == value::encode_undefined() {
        RuntimeString::from_utf8_str(" ")
    } else {
        let p = ctx.get_runtime_string(pad_str_val);
        if p.is_empty() {
            RuntimeString::from_utf8_str(" ")
        } else {
            p
        }
    };
    let mut result = repeat_units_to_len(&pad, target - len);
    result.push_units_from(&s);
    ctx.store_runtime_string(result)
}

pub fn string_repeat<E: ExecContext>(ctx: &mut E, receiver: Value, count: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let c = to_f64_or(count, 0.0);
    if c < 0.0 || c.is_infinite() {
        ctx.set_last_error("RangeError: Invalid count value".to_string());
        return value::encode_undefined();
    }
    ctx.store_runtime_string(s.repeat(c as usize))
}

pub fn string_replace_all<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    replace: Value,
) -> Value {
    // RegExp 路径：要求 g flag，同步走默认 replace 算法（无回调时）或 call_js。
    if value::is_regexp(search) {
        if !ctx.regexp_is_global(search) {
            ctx.set_last_error(
                "TypeError: String.prototype.replaceAll called with a non-global RegExp argument"
                    .to_string(),
            );
            return value::encode_undefined();
        }
        // 同步桥接：经 call_js 语义等价的阻塞路径由后端 call_js 提供；
        // 此处用 string_replace_default 的同步子集——无函数替换时纯同步，
        // 有函数替换时经 call_js。
        return string_replace_all_regexp(ctx, receiver, search, replace);
    }
    let s = ctx.get_runtime_string(receiver);
    let search_str = ctx.get_runtime_string(search);
    let replace_str = ctx.get_runtime_string(replace);
    ctx.store_runtime_string(replace_all_units(&s, &search_str, &replace_str))
}

/// replaceAll + RegExp(g)：同步路径，替换回调经 `call_js`。
fn string_replace_all_regexp<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    replace: Value,
) -> Value {
    use crate::reentrant::string::process_replacement_from_captures;
    use wjsm_host::RuntimeString;

    let s = ctx.get_runtime_string(receiver);
    let subject_lossy = s.to_utf8_lossy();
    let is_func_replace = value::is_callable(replace);
    let matches = ctx.regexp_collect_matches(search, &subject_lossy, true);
    if matches.is_empty() {
        return ctx.store_runtime_string(s);
    }
    let mut result = String::new();
    let mut last_end = 0;
    for mi in &matches {
        result.push_str(&subject_lossy[last_end..mi.start]);
        let replaced = if is_func_replace {
            let capture_count = mi.captures.len().saturating_sub(1);
            let mut args = Vec::with_capacity(1 + capture_count + 3);
            args.push(ctx.store_string(&subject_lossy[mi.start..mi.end]));
            for i in 1..=capture_count {
                let capture_val = if let Some(Some(range)) = mi.captures.get(i) {
                    ctx.store_string(&subject_lossy[range.clone()])
                } else {
                    value::encode_undefined()
                };
                args.push(capture_val);
            }
            args.push(value::encode_f64(mi.start as f64));
            args.push(ctx.store_string(&subject_lossy));
            let groups_obj = if mi.named.is_empty() {
                value::encode_undefined()
            } else {
                let obj = ctx.alloc_null_proto_object(mi.named.len() as u32);
                for (name, range) in &mi.named {
                    let val = match range {
                        Some(r) => ctx.store_string(&subject_lossy[r.clone()]),
                        None => value::encode_undefined(),
                    };
                    ctx.define_data_property(obj, name, val);
                }
                obj
            };
            args.push(groups_obj);
            match ctx.call_js(replace, value::encode_undefined(), &args) {
                Ok(v) if value::is_string(v) || value::is_runtime_string_handle(v) => {
                    ctx.read_string_utf8_lossy(v)
                }
                Ok(v) if !value::is_undefined(v) => ctx.value_to_display_string(v),
                _ => String::new(),
            }
        } else {
            let replace_str_lossy = ctx.read_string_utf8_lossy(replace);
            process_replacement_from_captures(
                &replace_str_lossy,
                &subject_lossy,
                mi.start,
                mi.end,
                &mi.captures,
                &mi.named,
            )
        };
        result.push_str(&replaced);
        last_end = mi.end;
    }
    result.push_str(&subject_lossy[last_end..]);
    ctx.store_runtime_string(RuntimeString::from_utf8_str(&result))
}

pub fn string_slice<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    start: Value,
    end: Value,
) -> Value {
    if value::is_array(receiver) {
        return crate::timers_arrays::arr_slice(ctx, receiver, start, end);
    }
    let s = ctx.get_runtime_string(receiver);
    let len = s.utf16_len() as i64;
    let si = if value::is_f64(start) {
        let v = to_f64_or(start, 0.0) as i64;
        if v < 0 { (v + len).max(0) } else { v.min(len) }
    } else {
        0
    };
    let ei = if end == value::encode_undefined() {
        len
    } else if value::is_f64(end) {
        let v = to_f64_or(end, 0.0) as i64;
        if v < 0 { (v + len).max(0) } else { v.min(len) }
    } else {
        0
    };
    if si >= ei {
        return ctx.store_runtime_string(RuntimeString::empty());
    }
    ctx.store_runtime_string(s.slice_units(si as usize..ei as usize))
}

pub fn string_starts_with<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    search: Value,
    pos: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let start_utf16 = if pos == value::encode_undefined() {
        0
    } else {
        (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
    };
    let search_str = ctx.get_runtime_string(search);
    value::encode_bool(s.starts_with_units(&search_str, start_utf16))
}

pub fn string_substring<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    start: Value,
    end: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let len = s.utf16_len() as i64;
    let s1 = if value::is_f64(start) {
        (to_f64_or(start, 0.0) as i64).max(0).min(len)
    } else {
        0
    };
    let e1 = if end == value::encode_undefined() {
        len
    } else {
        (to_f64_or(end, 0.0) as i64).max(0).min(len)
    };
    let (lo, hi) = if s1 < e1 { (s1, e1) } else { (e1, s1) };
    if lo >= hi {
        return ctx.store_runtime_string(RuntimeString::empty());
    }
    ctx.store_runtime_string(s.slice_units(lo as usize..hi as usize))
}

pub fn string_normalize<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    form_val: Value,
    _unused: Value,
) -> Value {
    let s = ctx.get_runtime_string(receiver);
    let form_lossy = if form_val == value::encode_undefined() {
        "NFC".to_string()
    } else {
        ctx.get_runtime_string(form_val).to_utf8_lossy()
    };
    match normalize_runtime_string_by_form(&s, &form_lossy) {
        Ok(out) => ctx.store_runtime_string(out),
        Err(msg) => ctx.make_range_error(msg),
    }
}

pub fn string_to_lower_case<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = transform_scalar_runs(&ctx.get_runtime_string(receiver), str::to_lowercase);
    ctx.store_runtime_string(s)
}

pub fn string_to_upper_case<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = transform_scalar_runs(&ctx.get_runtime_string(receiver), str::to_uppercase);
    ctx.store_runtime_string(s)
}

pub fn string_trim<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = trim_runtime_string(&ctx.get_runtime_string(receiver), true, true);
    ctx.store_runtime_string(s)
}

pub fn string_trim_end<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = trim_runtime_string(&ctx.get_runtime_string(receiver), false, true);
    ctx.store_runtime_string(s)
}

pub fn string_trim_start<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = trim_runtime_string(&ctx.get_runtime_string(receiver), true, false);
    ctx.store_runtime_string(s)
}

pub fn string_to_string<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    if value::is_string(receiver) {
        let s = ctx.get_runtime_string(receiver);
        ctx.store_runtime_string(s)
    } else {
        ctx.obj_proto_to_string(receiver)
    }
}

pub fn string_value_of<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    ctx.store_runtime_string(s)
}

pub fn string_iterator<E: ExecContext>(ctx: &mut E, receiver: Value) -> Value {
    let s = ctx.get_runtime_string(receiver);
    ctx.create_string_iterator(s)
}

pub fn string_from_char_code<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
) -> Value {
    let mut units = Vec::with_capacity(args_count.max(0) as usize);
    for i in 0..args_count as u32 {
        let arg = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            i,
        );
        units.push(to_uint16_ctx(ctx, arg));
    }
    ctx.store_runtime_string(RuntimeString::from_utf16_units(units))
}

pub fn string_from_code_point<E: ExecContext>(
    ctx: &mut E,
    args_base: i32,
    args_count: i32,
) -> Value {
    let mut result = RuntimeString::empty();
    for i in 0..args_count as u32 {
        let arg = ctx.read_call_arg(
            wjsm_host::CallArgs::new(args_base as u32, args_count as u32),
            i,
        );
        let code = to_uint32_ctx(ctx, arg);
        if !is_valid_code_point(code) {
            return ctx.make_range_error("Invalid code point");
        }
        result.push_units_from(&runtime_string_from_code_point(code));
    }
    ctx.store_runtime_string(result)
}
