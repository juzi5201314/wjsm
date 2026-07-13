use anyhow::Result;
use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use wasmtime::{Caller, Func, Linker, Store};

use crate::runtime_host_helpers::make_range_error_exception;
use crate::runtime_string::RuntimeString;
use crate::*;

fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

fn decode_surrogate_pair(high: u16, low: u16) -> u32 {
    0x10000 + (((high as u32 - 0xD800) << 10) | (low as u32 - 0xDC00))
}

/// ECMAScript §22.1.3.15：按 form 对 valid Unicode scalar run 做规范化。
fn normalize_string_by_form(s: &str, form: &str) -> std::result::Result<String, &'static str> {
    match form {
        "NFC" => Ok(ComposingNormalizerBorrowed::new_nfc()
            .normalize(s)
            .into_owned()),
        "NFD" => Ok(DecomposingNormalizerBorrowed::new_nfd()
            .normalize(s)
            .into_owned()),
        "NFKC" => Ok(ComposingNormalizerBorrowed::new_nfkc()
            .normalize(s)
            .into_owned()),
        "NFKD" => Ok(DecomposingNormalizerBorrowed::new_nfkd()
            .normalize(s)
            .into_owned()),
        _ => Err("The normalization form should be one of NFC, NFD, NFKC, NFKD"),
    }
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
    let units = input.as_utf16_units();
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

fn normalize_runtime_string_by_form(
    input: &RuntimeString,
    form: &str,
) -> std::result::Result<RuntimeString, &'static str> {
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
    let units = input.as_utf16_units();
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
            let Some((cp_start, cp, _width, scalar)) = previous_code_point_width(units, end) else {
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
    let source_units = source.as_utf16_units();
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

fn replace_all_units(
    haystack: &RuntimeString,
    search: &RuntimeString,
    replacement: &RuntimeString,
) -> RuntimeString {
    let mut out = Vec::new();
    if search.is_empty() {
        out.extend_from_slice(replacement.as_utf16_units());
        for unit in haystack.as_utf16_units() {
            out.push(*unit);
            out.extend_from_slice(replacement.as_utf16_units());
        }
        return RuntimeString::from_utf16_units(out);
    }

    let search_len = search.utf16_len();
    let mut pos = 0usize;
    while let Some(found) = haystack.find_units(search, pos) {
        out.extend_from_slice(&haystack.as_utf16_units()[pos..found]);
        out.extend_from_slice(replacement.as_utf16_units());
        pos = found + search_len;
    }
    out.extend_from_slice(&haystack.as_utf16_units()[pos..]);
    RuntimeString::from_utf16_units(out)
}

fn runtime_string_from_code_point(cp: u32) -> RuntimeString {
    let Some(ch) = char::from_u32(cp) else {
        return RuntimeString::empty();
    };
    let mut buf = [0u16; 2];
    RuntimeString::from_utf16_units(ch.encode_utf16(&mut buf).to_vec())
}

pub(crate) fn define_string_methods(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    fn to_f64_or(val: i64, default: f64) -> f64 {
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
        let v = n.trunc().rem_euclid(65536.0);
        v as u16
    }

    fn to_uint32_number(n: f64) -> u32 {
        if !n.is_finite() || n == 0.0 {
            return 0;
        }
        let v = n.trunc().rem_euclid(4294967296.0);
        v as u32
    }

    fn to_uint16_caller(caller: &mut Caller<'_, RuntimeState>, val: i64) -> u16 {
        to_uint16_number(value::decode_f64(to_number(caller, val)))
    }

    fn to_uint32_caller(caller: &mut Caller<'_, RuntimeState>, val: i64) -> u32 {
        to_uint32_number(value::decode_f64(to_number(caller, val)))
    }

    fn concat_arg_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> RuntimeString {
        if value::is_string(val) {
            get_string_value(caller, val)
        } else {
            RuntimeString::from_utf8_str(&render_value(caller, val).unwrap_or_default())
        }
    }

    fn is_valid_code_point(cp: u32) -> bool {
        cp <= 0x10FFFF && !(0xD800..=0xDFFF).contains(&cp)
    }

    let primitive_string_get_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, name_id: i32| -> i64 {
            let name_id = name_id as u32;
            let is_length = crate::runtime_render::read_string_bytes(&mut caller, name_id)
                == b"length"
                || WasmEnv::from_caller(&mut caller).is_some_and(|env| {
                    let expected = RuntimeString::from_utf8_str("length");
                    crate::property_key::name_id_matches_runtime_string(
                        &caller, &env, name_id, &expected,
                    )
                });
            if is_length {
                let len = get_string_value(&mut caller, receiver).utf16_len();
                return value::encode_f64(len as f64);
            }
            let method =
                match crate::runtime_render::read_string_bytes(&mut caller, name_id).as_slice() {
                    b"includes" => 0,
                    b"startsWith" => 1,
                    b"indexOf" => 2,
                    _ => return value::encode_undefined(),
                };
            create_native_callable(
                caller.data(),
                NativeCallable::StringPrimitiveMethod { method },
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_string_get_property",
        primitive_string_get_property_fn,
    )?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, index: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = s.utf16_len() as i64;
            let idx = to_f64_or(index, 0.0);
            let mut i = idx as i64;
            if idx < 0.0 {
                i += len;
            }
            if i < 0 || i >= len {
                return value::encode_undefined();
            }
            store_runtime_string(
                &caller,
                RuntimeString::from_utf16_code_unit(s.code_unit_at(i as usize).unwrap_or(0)),
            )
        },
    );
    linker.define(&mut store, "env", "string_at", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            let Some(unit) = s.code_unit_at(p) else {
                return store_runtime_string(&caller, RuntimeString::empty());
            };
            store_runtime_string(&caller, RuntimeString::from_utf16_code_unit(unit))
        },
    );
    linker.define(&mut store, "env", "string_char_at", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            s.code_unit_at(p)
                .map(|unit| value::encode_f64(unit as f64))
                .unwrap_or_else(|| value::encode_f64(f64::NAN))
        },
    );
    linker.define(&mut store, "env", "string_char_code_at", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let p = to_uint32_caller(&mut caller, pos) as usize;
            s.code_point_at(p)
                .map(|cp| value::encode_f64(cp as f64))
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(&mut store, "env", "string_code_point_at", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if value::is_array(this_val) {
                return super::array_object::array_concat_args(
                    &mut caller,
                    this_val,
                    args_base,
                    args_count,
                );
            }
            let mut result = get_string_value(&mut caller, this_val);
            let parts: Vec<RuntimeString> = (0..args_count as u32)
                .map(|i| {
                    let arg = read_shadow_arg(&mut caller, args_base, i);
                    concat_arg_to_string(&mut caller, arg)
                })
                .collect();
            for part in parts {
                result.push_units_from(&part);
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_proto_concat", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, end_pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let len = s.utf16_len();
            let end_utf16 = if end_pos == value::encode_undefined() {
                len
            } else {
                (to_f64_or(end_pos, 0.0) as usize).min(len)
            };
            value::encode_bool(s.ends_with_units(&search_str, end_utf16))
        },
    );
    linker.define(&mut store, "env", "string_ends_with", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                return super::array_object::array_includes_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    pos,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
            };
            value::encode_bool(
                s.find_units(&get_string_value(&mut caller, search), start_utf16)
                    .is_some(),
            )
        },
    );
    linker.define(&mut store, "env", "string_includes", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                return super::array_object::array_index_of_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    pos,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
            };
            s.find_units(&search_str, start_utf16)
                .map(|index| value::encode_f64(index as f64))
                .unwrap_or_else(|| value::encode_f64(-1.0))
        },
    );
    linker.define(&mut store, "env", "string_index_of", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            if value::is_array(receiver)
                && let Some(ptr) = resolve_array_ptr(&mut caller, receiver)
            {
                let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                let from_index = if pos == value::encode_undefined() {
                    value::encode_f64((len as i64 - 1) as f64)
                } else {
                    pos
                };
                return super::array_object::array_last_index_of_from(
                    &mut caller,
                    ptr,
                    len,
                    search,
                    from_index,
                );
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
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
        },
    );
    linker.define(&mut store, "env", "string_last_index_of", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         receiver: i64,
         target_len: i64,
         pad_str_val: i64|
         -> i64 {
            let mut s = get_string_value(&mut caller, receiver);
            let len = s.utf16_len();
            let target = if value::is_f64(target_len) {
                to_f64_or(target_len, 0.0) as usize
            } else {
                0
            };
            if target <= len {
                return store_runtime_string(&caller, s);
            }
            let pad = if pad_str_val == value::encode_undefined() {
                RuntimeString::from_utf8_str(" ")
            } else {
                let p = get_string_value(&mut caller, pad_str_val);
                if p.is_empty() {
                    RuntimeString::from_utf8_str(" ")
                } else {
                    p
                }
            };
            s.push_units_from(&repeat_units_to_len(&pad, target - len));
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_pad_end", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         receiver: i64,
         target_len: i64,
         pad_str_val: i64|
         -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let len = s.utf16_len();
            let target = if value::is_f64(target_len) {
                to_f64_or(target_len, 0.0) as usize
            } else {
                0
            };
            if target <= len {
                return store_runtime_string(&caller, s);
            }
            let pad = if pad_str_val == value::encode_undefined() {
                RuntimeString::from_utf8_str(" ")
            } else {
                let p = get_string_value(&mut caller, pad_str_val);
                if p.is_empty() {
                    RuntimeString::from_utf8_str(" ")
                } else {
                    p
                }
            };
            let mut result = repeat_units_to_len(&pad, target - len);
            result.push_units_from(&s);
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_pad_start", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, count: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let c = to_f64_or(count, 0.0);
            if c < 0.0 || c.is_infinite() {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("RangeError: Invalid count value".to_string());
                return value::encode_undefined();
            }
            store_runtime_string(&caller, s.repeat(c as usize))
        },
    );
    linker.define(&mut store, "env", "string_repeat", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, replace: i64| -> i64 {
            if value::is_regexp(search) {
                let handle = value::decode_regexp_handle(search);
                let is_global = {
                    let table = caller
                        .data()
                        .regex_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    match table.get(handle as usize) {
                        Some(e) => e.flags.contains('g'),
                        None => false,
                    }
                };
                if !is_global {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(
                        "TypeError: String.prototype.replaceAll called with a non-global RegExp argument"
                            .to_string(),
                    );
                    return value::encode_undefined();
                }
                let rt = tokio::runtime::Handle::current();
                return rt.block_on(super::reentrant_async::string_replace_async_body(
                    caller, receiver, search, replace,
                ));
            }
            let s = get_string_value(&mut caller, receiver);
            let search_str = get_string_value(&mut caller, search);
            let replace_str = get_string_value(&mut caller, replace);
            store_runtime_string(&caller, replace_all_units(&s, &search_str, &replace_str))
        },
    );
    linker.define(&mut store, "env", "string_replace_all", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
            if value::is_array(receiver) {
                return super::array_object::array_slice_range(&mut caller, receiver, start, end);
            }
            let s = get_string_value(&mut caller, receiver);
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
                return store_runtime_string(&caller, RuntimeString::empty());
            }
            store_runtime_string(&caller, s.slice_units(si as usize..ei as usize))
        },
    );
    linker.define(&mut store, "env", "string_slice", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64, pos: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let start_utf16 = if pos == value::encode_undefined() {
                0
            } else {
                (to_f64_or(pos, 0.0) as usize).min(s.utf16_len())
            };
            value::encode_bool(
                s.starts_with_units(&get_string_value(&mut caller, search), start_utf16),
            )
        },
    );
    linker.define(&mut store, "env", "string_starts_with", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, start: i64, end: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
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
                return store_runtime_string(&caller, RuntimeString::empty());
            }
            store_runtime_string(&caller, s.slice_units(lo as usize..hi as usize))
        },
    );
    linker.define(&mut store, "env", "string_substring", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, form_val: i64, _unused: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let form_lossy = if form_val == value::encode_undefined() {
                "NFC".to_string()
            } else {
                get_string_utf8_lossy(&mut caller, form_val)
            };
            match normalize_runtime_string_by_form(&s, &form_lossy) {
                Ok(out) => store_runtime_string(&caller, out),
                Err(msg) => make_range_error_exception(&mut caller, msg),
            }
        },
    );
    linker.define(&mut store, "env", "string_normalize", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s =
                transform_scalar_runs(&get_string_value(&mut caller, receiver), str::to_lowercase);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_to_lower_case", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s =
                transform_scalar_runs(&get_string_value(&mut caller, receiver), str::to_uppercase);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_to_upper_case", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = trim_runtime_string(&get_string_value(&mut caller, receiver), true, true);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = trim_runtime_string(&get_string_value(&mut caller, receiver), false, true);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim_end", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = trim_runtime_string(&get_string_value(&mut caller, receiver), true, false);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_trim_start", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            if value::is_string(receiver) {
                let s = get_string_value(&mut caller, receiver);
                store_runtime_string(&caller, s)
            } else {
                obj_proto_to_string_impl(&mut caller, receiver)
            }
        },
    );
    linker.define(&mut store, "env", "string_to_string", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_value_of", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let s = get_string_value(&mut caller, receiver);
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = iters.len() as u32;
            iters.push(IteratorState::StringIter {
                string: s,
                unit_pos: 0,
            });
            value::encode_handle(value::TAG_ITERATOR, handle)
        },
    );
    linker.define(&mut store, "env", "string_iterator", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut units = Vec::with_capacity(args_count.max(0) as usize);
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                units.push(to_uint16_caller(&mut caller, arg));
            }
            store_runtime_string(&caller, RuntimeString::from_utf16_units(units))
        },
    );
    linker.define(&mut store, "env", "string_from_char_code", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut result = RuntimeString::empty();
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                let code = to_uint32_caller(&mut caller, arg);
                if !is_valid_code_point(code) {
                    return make_range_error_exception(&mut caller, "Invalid code point");
                }
                result.push_units_from(&runtime_string_from_code_point(code));
            }
            store_runtime_string(&caller, result)
        },
    );
    linker.define(&mut store, "env", "string_from_code_point", f)?;

    Ok(())
}
