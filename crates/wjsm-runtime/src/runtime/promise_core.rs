use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{Datelike, DateTime, Local, TimeZone, Timelike, Utc};
use swc_core::ecma::ast as swc_ast;
use wasmtime::{Caller, Extern, ExternType, Func, FuncType, Global, Instance, Memory, Module, Store, Table, Val, ValType};

use crate::types::*;
use crate::runtime::string_utils::{store_runtime_string, store_runtime_string_in_state, read_value_string_bytes, read_string_bytes, read_eval_var_map};
use crate::runtime::memory::*;
use crate::runtime::object_ops::{read_object_property_by_name, find_property_slot_by_name_id, collect_own_property_names, collect_own_property_values};
use crate::runtime::format::format_number_js;
use crate::runtime::conversions::{to_number, get_string_value, type_tag};
use crate::runtime::render::{render_value, write_console_value};
use crate::runtime::function_ops::{read_shadow_arg, call_wasm_callback, resolve_and_call, resolve_callable_and_call, func_apply_impl, func_bind_impl, object_rest_impl, obj_spread_impl, raw_promise_handle, insert_promise_entry};
use crate::runtime::microtask::{set_runtime_error, call_host_function_from_caller, drain_microtasks_from_caller, nanbox_to_bool};
use crate::runtime::eval::{eval_to_number, eval_to_string, is_promise_value, promise_entry_mut, promise_entry, settle_promise, alloc_promise_from_caller, new_promise_capability_from_caller, create_promise_resolving_functions, runtime_error_value, alloc_iterator_result_from_caller, enqueue_async_resume_from_caller, pump_async_generator_from_caller, create_combinator_context, set_combinator_remaining, mark_combinator_settled, create_combinator_reaction_handler, combinator_reaction_record, open_combinator_context, decrement_combinator_remaining, handle_combinator_reaction_from_caller, handle_combinator_reaction_from_store, queue_promise_reactions, adopt_promise, resolve_promise_from_caller, resolve_promise_from_store, passive_reaction_settlement, create_async_generator_method, create_promise_resolving_function};
use wjsm_ir::{constants, value};

pub(crate) fn advance_object_iterator_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
    next: i64,
) -> (i64, i64, bool, bool) {
    let result =
        call_host_function_from_caller(caller, func_table, next, value::encode_undefined())
            .unwrap_or_else(value::encode_undefined);
    if value::is_object(result) || value::is_function(result) || value::is_array(result) {
        if let Some(ptr) = resolve_handle(caller, result) {
            let done = read_object_property_by_name(caller, ptr, "done")
                .map(nanbox_to_bool)
                .unwrap_or(false);
            let current_value = read_object_property_by_name(caller, ptr, "value")
                .unwrap_or_else(value::encode_undefined);
            return (result, current_value, done, true);
        }
    }
    if !value::is_undefined(result) {
        set_runtime_error(
            caller.data(),
            "TypeError: iterator next must return an object".to_string(),
        );
    }
    (result, value::encode_undefined(), true, true)
}

pub(crate) fn create_async_generator_identity(state: &RuntimeState, generator: i64) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::AsyncGeneratorIdentity { generator });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_map_set_method(state: &RuntimeState, kind: MapSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::MapSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_date_method(state: &RuntimeState, kind: DateMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::DateMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakmap_method(state: &RuntimeState, kind: WeakMapMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakMapMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakset_method(state: &RuntimeState, kind: WeakSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn read_date_ms(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> f64 {
    if !value::is_object(this_val) {
        return f64::NAN;
    }
    let obj_ptr = resolve_handle_idx(
        caller,
        value::decode_object_handle(this_val) as usize,
    );
    let Some(op) = obj_ptr else {
        return f64::NAN;
    };
    let ms_val = read_object_property_by_name(caller, op, "__date_ms__");
    ms_val.map(|v| value::decode_f64(v)).unwrap_or(f64::NAN)
}

pub(crate) fn write_date_ms(caller: &mut Caller<'_, RuntimeState>, this_val: i64, ms: f64) {
    if !value::is_object(this_val) {
        return;
    }
    let obj_ptr = resolve_handle_idx(
        caller,
        value::decode_object_handle(this_val) as usize,
    );
    let Some(op) = obj_ptr else {
        return;
    };
    let name_id = match find_memory_c_string_global(caller, "__date_ms__") {
        Some(id) => id,
        None => return,
    };
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return;
    };
    let data = memory.data(&*caller);
    if op + 16 > data.len() {
        return;
    }
    let num_props = u32::from_le_bytes([
        data[op + 12],
        data[op + 13],
        data[op + 14],
        data[op + 15],
    ]) as usize;
    for i in 0..num_props {
        let slot_offset = op + 16 + i * 32;
        if slot_offset + 32 > data.len() {
            break;
        }
        let slot_name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if slot_name_id == name_id {
            let val = value::encode_f64(ms);
            let _ = data;
            let data = memory.data_mut(&mut *caller);
            data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
            return;
        }
    }
}

pub(crate) fn ms_to_datetime_utc(ms: f64) -> Option<DateTime<Utc>> {
    if ms.is_nan() || ms.is_infinite() {
        return None;
    }
    Utc.timestamp_millis_opt(ms as i64).single()
}

pub(crate) fn ms_to_datetime_local(ms: f64) -> Option<DateTime<Local>> {
    if ms.is_nan() || ms.is_infinite() {
        return None;
    }
    let utc_dt = ms_to_datetime_utc(ms)?;
    Some(utc_dt.with_timezone(&Local))
}

pub(crate) fn date_args_to_ms(args: &[i64], is_utc: bool) -> f64 {
    if args.is_empty() {
        return f64::NAN;
    }
    let first = value::decode_f64(args[0]);
    if first.is_nan() {
        return f64::NAN;
    }
    if args.len() == 1 {
        if first.is_infinite() {
            return f64::NAN;
        }
        return first;
    }
    let year = first;
    let month_val = if args.len() > 1 { value::decode_f64(args[1]) } else { 0.0 };
    let day = if args.len() > 2 { value::decode_f64(args[2]) } else { 1.0 };
    let hour = if args.len() > 3 { value::decode_f64(args[3]) } else { 0.0 };
    let minute = if args.len() > 4 { value::decode_f64(args[4]) } else { 0.0 };
    let second = if args.len() > 5 { value::decode_f64(args[5]) } else { 0.0 };
    let millisecond = if args.len() > 6 { value::decode_f64(args[6]) } else { 0.0 };

    if year.is_nan() || month_val.is_nan() || day.is_nan()
        || hour.is_nan() || minute.is_nan() || second.is_nan() || millisecond.is_nan()
    {
        return f64::NAN;
    }

    let y = year as i32;
    let m = month_val as u32;
    let d = day as u32;
    let h = hour as u32;
    let min = minute as u32;
    let s = second as u32;
    let ms = millisecond as u32;

    let adjusted_year = if y >= 0 && y <= 99 { 1900 + y } else { y };

    if is_utc {
        Utc.with_ymd_and_hms(adjusted_year, m + 1, d.max(1), h, min, s)
            .single()
            .map(|dt: DateTime<Utc>| dt.timestamp_millis() as f64 + ms as f64)
            .unwrap_or(f64::NAN)
    } else {
        Local.with_ymd_and_hms(adjusted_year, m + 1, d.max(1), h, min, s)
            .single()
            .map(|dt: DateTime<Local>| dt.timestamp_millis() as f64 + ms as f64)
            .unwrap_or(f64::NAN)
    }
}

pub(crate) fn call_date_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: DateMethodKind,
    args: Vec<i64>,
) -> i64 {
    let ms = read_date_ms(caller, this_val);

    match kind {
        DateMethodKind::GetTime => value::encode_f64(ms),
        DateMethodKind::ValueOf => value::encode_f64(ms),
        DateMethodKind::GetTimezoneOffset => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let utc_dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let local_dt = utc_dt.with_timezone(&Local);
            let offset_secs = local_dt.offset().local_minus_utc();
            value::encode_f64(-(offset_secs as f64) / 60.0)
        }
        DateMethodKind::GetFullYear => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.year() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetMonth => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64((dt.month0()) as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetDate => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.day() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetDay => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.weekday().num_days_from_sunday() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetHours => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.hour() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetMinutes => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.minute() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetSeconds => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64(dt.second() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetMilliseconds => {
            match ms_to_datetime_local(ms) {
                Some(dt) => value::encode_f64((dt.nanosecond() / 1_000_000) as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCFullYear => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.year() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCMonth => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64((dt.month0()) as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCDate => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.day() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCDay => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.weekday().num_days_from_sunday() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCHours => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.hour() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCMinutes => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.minute() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCSeconds => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64(dt.second() as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::GetUTCMilliseconds => {
            match ms_to_datetime_utc(ms) {
                Some(dt) => value::encode_f64((dt.nanosecond() / 1_000_000) as f64),
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::SetTime => {
            let new_ms = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetDate => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let d = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if d.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_day(d as u32).unwrap_or(dt);
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetMonth => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let m = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if m.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_month0(m as u32).unwrap_or(dt);
            if let Some(d_arg) = args.get(1) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetFullYear => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let y = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if y.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_year(y as i32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_month0(m as u32).unwrap_or(new_dt);
            }
            if let Some(d_arg) = args.get(2) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetHours => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let h = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if h.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_hour(h as u32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_minute(m as u32).unwrap_or(new_dt);
            }
            if let Some(s_arg) = args.get(2) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(3) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetMinutes => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let m = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if m.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_minute(m as u32).unwrap_or(dt);
            if let Some(s_arg) = args.get(1) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(2) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetSeconds => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let s = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if s.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_second(s as u32).unwrap_or(dt);
            let new_ms = if let Some(ms_arg) = args.get(1) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetMilliseconds => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let ms_arg = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if ms_arg.is_nan() { return value::encode_f64(f64::NAN); }
            let base_ms = (ms / 1000.0).trunc() * 1000.0;
            let new_ms = base_ms + ms_arg.trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCDate => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let d = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if d.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_day(d as u32).unwrap_or(dt);
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCMonth => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let m = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if m.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_month0(m as u32).unwrap_or(dt);
            if let Some(d_arg) = args.get(1) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCFullYear => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let y = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if y.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_year(y as i32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_month0(m as u32).unwrap_or(new_dt);
            }
            if let Some(d_arg) = args.get(2) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCHours => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let h = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if h.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_hour(h as u32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_minute(m as u32).unwrap_or(new_dt);
            }
            if let Some(s_arg) = args.get(2) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(3) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCMinutes => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let m = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if m.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_minute(m as u32).unwrap_or(dt);
            if let Some(s_arg) = args.get(1) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(2) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCSeconds => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let s = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if s.is_nan() { return value::encode_f64(f64::NAN); }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_second(s as u32).unwrap_or(dt);
            let new_ms = if let Some(ms_arg) = args.get(1) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() { return value::encode_f64(f64::NAN); }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCMilliseconds => {
            if ms.is_nan() { return value::encode_f64(f64::NAN); }
            let ms_arg = args.first().map(|v| value::decode_f64(*v)).unwrap_or(f64::NAN);
            if ms_arg.is_nan() { return value::encode_f64(f64::NAN); }
            let base_ms = (ms / 1000.0).trunc() * 1000.0;
            let new_ms = base_ms + ms_arg.trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::ToString => {
            if ms.is_nan() {
                return store_runtime_string(&caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%a %b %e %Y %H:%M:%S GMT%:z").to_string();
                    store_runtime_string(&caller, s)
                }
                None => store_runtime_string(&caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToDateString => {
            if ms.is_nan() {
                return store_runtime_string(&caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%Y-%m-%d").to_string();
                    store_runtime_string(&caller, s)
                }
                None => store_runtime_string(&caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToTimeString => {
            if ms.is_nan() {
                return store_runtime_string(&caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%H:%M:%S GMT%:z").to_string();
                    store_runtime_string(&caller, s)
                }
                None => store_runtime_string(&caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToISOString => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            match ms_to_datetime_utc(ms) {
                Some(dt) => {
                    let frac_sec = (ms % 1000.0).trunc().abs() as u32;
                    let year = dt.year();
                    let s = if year >= 0 && year <= 9999 {
                        format!(
                            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                            year,
                            dt.month(),
                            dt.day(),
                            dt.hour(),
                            dt.minute(),
                            dt.second(),
                            frac_sec
                        )
                    } else {
                        format!(
                            "{:+06}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                            year,
                            dt.month(),
                            dt.day(),
                            dt.hour(),
                            dt.minute(),
                            dt.second(),
                            frac_sec
                        )
                    };
                    store_runtime_string(&caller, s)
                }
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::ToUTCString => {
            if ms.is_nan() {
                return store_runtime_string(&caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_utc(ms) {
                Some(dt) => {
                    let s = dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
                    store_runtime_string(&caller, s)
                }
                None => store_runtime_string(&caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToJSON => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            match ms_to_datetime_utc(ms) {
                Some(dt) => {
                    let frac_sec = (ms % 1000.0).trunc().abs() as u32;
                    let year = dt.year();
                    let s = if year >= 0 && year <= 9999 {
                        format!(
                            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                            year,
                            dt.month(),
                            dt.day(),
                            dt.hour(),
                            dt.minute(),
                            dt.second(),
                            frac_sec
                        )
                    } else {
                        format!(
                            "{:+06}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                            year,
                            dt.month(),
                            dt.day(),
                            dt.hour(),
                            dt.minute(),
                            dt.second(),
                            frac_sec
                        )
                    };
                    store_runtime_string(&caller, s)
                }
                None => value::encode_f64(f64::NAN),
            }
        }
    }
}

pub(crate) fn read_weakmap_handle(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakmap_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

pub(crate) fn read_weakset_handle(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakset_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

pub(crate) fn is_object_key(key: i64) -> bool {
    value::is_object(key) || value::is_array(key) || value::is_function(key)
}

pub(crate) fn call_weakmap_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: WeakMapMethodKind,
    args: Vec<i64>,
) -> i64 {
    match kind {
        WeakMapMethodKind::Set => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        }
        WeakMapMethodKind::Get => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                if let Some(&val) = table[handle].map.get(&key_handle) {
                    return val;
                }
            }
            value::encode_undefined()
        }
        WeakMapMethodKind::Has => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakMapMethodKind::Delete => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        }
    }
}

pub(crate) fn call_weakset_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: WeakSetMethodKind,
    args: Vec<i64>,
) -> i64 {
    match kind {
        WeakSetMethodKind::Add => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        }
        WeakSetMethodKind::Has => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakSetMethodKind::Delete => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.remove(&key_handle));
            }
            value::encode_bool(false)
        }
    }
}

pub(crate) fn call_map_set_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: MapSetMethodKind,
    args: Vec<i64>,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(
        caller,
        value::decode_object_handle(this_val) as usize,
    );
    let Some(op) = obj_ptr else {
        return value::encode_undefined();
    };
    let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
    let set_handle = read_object_property_by_name(caller, op, "__set_handle__");

    match kind {
        MapSetMethodKind::MapSet => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            entry.values[i] = val;
                            return this_val;
                        }
                    }
                    entry.keys.push(key);
                    entry.values.push(val);
                }
            }
            this_val
        }
        MapSetMethodKind::MapGet => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            return entry.values[i];
                        }
                    }
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::SetAdd => {
            let val = args.first().copied().unwrap_or_else(value::encode_undefined);
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(entry.values[i], val) {
                            return this_val;
                        }
                    }
                    entry.values.push(val);
                }
            }
            this_val
        }
        MapSetMethodKind::Has => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller.data().set_table.lock().expect("set table mutex");
                if handle < table.len() {
                    let entry = &table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(entry.values[i], key) {
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            value::encode_bool(false)
        }
        MapSetMethodKind::Delete => {
            let key = args.first().copied().unwrap_or_else(value::encode_undefined);
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.keys.len() {
                        if same_value_zero(entry.keys[i], key) {
                            entry.keys.remove(i);
                            entry.values.remove(i);
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                if handle < table.len() {
                    let entry = &mut table[handle];
                    for i in 0..entry.values.len() {
                        if same_value_zero(entry.values[i], key) {
                            entry.values.remove(i);
                            return value::encode_bool(true);
                        }
                    }
                }
                return value::encode_bool(false);
            }
            value::encode_bool(false)
        }
        MapSetMethodKind::Clear => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    table[handle].keys.clear();
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                if handle < table.len() {
                    table[handle].values.clear();
                }
                return value::encode_undefined();
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Size => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    return value::encode_f64(table[handle].keys.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            if let Some(sh) = set_handle {
                let handle = value::decode_f64(sh) as usize;
                let table = caller.data().set_table.lock().expect("set table mutex");
                if handle < table.len() {
                    return value::encode_f64(table[handle].values.len() as f64);
                }
                return value::encode_f64(0.0);
            }
            value::encode_f64(0.0)
        }
        // TODO: implement forEach/keys/values/entries — currently stubbed
        MapSetMethodKind::ForEach
        | MapSetMethodKind::Keys
        | MapSetMethodKind::Values
        | MapSetMethodKind::Entries => {
            value::encode_undefined()
        }
    }
}

pub(crate) fn call_native_callable_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    argument: Option<i64>,
) -> Option<i64> {
    call_native_callable_with_args_from_caller(caller, callable, value::encode_undefined(), argument.into_iter().collect())
}

pub(crate) fn call_native_callable_with_args_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args: Vec<i64>,
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }

    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callable table mutex");
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    match record {
        NativeCallable::EvalIndirect => Some(perform_eval_from_caller(caller, argument, None)),
        NativeCallable::EvalFunction(function) => {
            Some(call_eval_function_from_caller(caller, function, args))
        }
        NativeCallable::PromiseResolvingFunction {
            promise,
            already_resolved,
            kind,
        } => {
            let mut already = already_resolved.lock().expect("promise resolver mutex");
            if *already {
                return Some(value::encode_undefined());
            }
            *already = true;
            drop(already);
            match kind {
                PromiseResolvingKind::Fulfill => {
                    resolve_promise_from_caller(caller, promise, argument);
                }
                PromiseResolvingKind::Reject => {
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(argument));
                }
            }
            Some(value::encode_undefined())
        }
        NativeCallable::PromiseCombinatorReaction { .. } => Some(value::encode_undefined()),
        NativeCallable::AsyncGeneratorIdentity { generator } => Some(generator),
        NativeCallable::AsyncGeneratorMethod { generator, kind } => {
            let result_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let request = AsyncGeneratorRequest {
                completion_type: kind,
                value: argument,
                promise: result_promise,
            };
            let completed = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return Some(result_promise);
                };
                if matches!(entry.state, AsyncGeneratorState::Completed) {
                    true
                } else {
                    entry.queue.push(request);
                    false
                }
            };
            if completed {
                match kind {
                    AsyncGeneratorCompletionType::Throw => settle_promise(
                        caller.data(),
                        result_promise,
                        PromiseSettlement::Reject(argument),
                    ),
                    _ => {
                        let result = alloc_iterator_result_from_caller(caller, argument, true);
                        resolve_promise_from_caller(caller, result_promise, result);
                    }
                }
            } else {
                pump_async_generator_from_caller(caller, generator);
            }
            Some(result_promise)
        }
        NativeCallable::MapSetMethod { kind } => {
            Some(call_map_set_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::DateMethod { kind } => {
            Some(call_date_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::WeakMapMethod { kind } => {
            Some(call_weakmap_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::WeakSetMethod { kind } => {
            Some(call_weakset_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::ArrayConstructor => {
            if value::is_object(this_val) {
                Some(this_val)
            } else {
                Some(alloc_host_object_from_caller(caller, 4))
            }
        }
        NativeCallable::ObjectConstructor => {
            if value::is_object(this_val) || value::is_function(this_val) {
                Some(this_val)
            } else {
                Some(alloc_host_object_from_caller(caller, 4))
            }
        }
        NativeCallable::FunctionConstructor
        | NativeCallable::StringConstructor
        | NativeCallable::BooleanConstructor
        | NativeCallable::NumberConstructor
        | NativeCallable::SymbolConstructor
        | NativeCallable::BigIntConstructor
        | NativeCallable::RegExpConstructor => {
            Some(value::encode_undefined())
        }
        NativeCallable::ErrorConstructor
        | NativeCallable::TypeErrorConstructor
        | NativeCallable::RangeErrorConstructor
        | NativeCallable::SyntaxErrorConstructor
        | NativeCallable::ReferenceErrorConstructor
        | NativeCallable::URIErrorConstructor
        | NativeCallable::EvalErrorConstructor
        | NativeCallable::AggregateErrorConstructor => {
            let error_name = match &record {
                NativeCallable::ErrorConstructor => "Error",
                NativeCallable::TypeErrorConstructor => "TypeError",
                NativeCallable::RangeErrorConstructor => "RangeError",
                NativeCallable::SyntaxErrorConstructor => "SyntaxError",
                NativeCallable::ReferenceErrorConstructor => "ReferenceError",
                NativeCallable::URIErrorConstructor => "URIError",
                NativeCallable::EvalErrorConstructor => "EvalError",
                NativeCallable::AggregateErrorConstructor => "AggregateError",
                _ => "Error",
            };
            let msg = args.first().copied().unwrap_or_else(value::encode_undefined);
            Some(create_error_object(caller, error_name, msg))
        }
        NativeCallable::MapConstructor => {
            Some(alloc_host_object_from_caller(caller, 0))
        }
        NativeCallable::SetConstructor => {
            Some(alloc_host_object_from_caller(caller, 0))
        }
        NativeCallable::WeakMapConstructor => {
            Some(alloc_host_object_from_caller(caller, 0))
        }
        NativeCallable::WeakSetConstructor => {
            Some(alloc_host_object_from_caller(caller, 0))
        }
        NativeCallable::DateConstructorGlobal => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::PromiseConstructor => {
            Some(alloc_host_object_from_caller(caller, 0))
        }
        NativeCallable::ArrayBufferConstructorGlobal => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::DataViewConstructorGlobal => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::TypedArrayConstructor(_) => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::ProxyConstructor => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::StubGlobal(_) => {
            Some(value::encode_undefined())
        }
    }
}

pub(crate) fn try_compiled_eval_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    code: &str,
    module: &swc_ast::Module,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
) -> Result<i64> {
    let data_base = reserve_eval_data_segment(caller, code.len() as u32)?;
    let wasm_bytes = cached_eval_wasm(
        caller.data(),
        code,
        module,
        scope_env.is_some(),
        var_writes_to_scope,
        data_base,
    )?;
    let eval_module = Module::new(caller.engine(), &wasm_bytes)?;
    let mut imports = Vec::with_capacity(eval_module.imports().count());

    for import in eval_module.imports() {
        match import.ty() {
            ExternType::Func(func_ty) => {
                let func = compiled_eval_import(caller, import.name(), &func_ty);
                imports.push(func.into());
            }
            ExternType::Memory(_) => {
                let memory = caller
                    .get_export(import.name())
                    .and_then(Extern::into_memory)
                    .ok_or_else(|| anyhow::anyhow!("eval parent missing memory import"))?;
                imports.push(memory.into());
            }
            ExternType::Global(_) => {
                let global = caller
                    .get_export(import.name())
                    .and_then(Extern::into_global)
                    .ok_or_else(|| {
                        anyhow::anyhow!("eval parent missing global import `{}`", import.name())
                    })?;
                imports.push(global.into());
            }
            _ => {
                anyhow::bail!("unsupported eval import `{}`", import.name());
            }
        }
    }

    let instance = Instance::new(&mut *caller, &eval_module, &imports)?;
    let entry = instance.get_typed_func::<i64, i64>(&mut *caller, "__eval_entry")?;
    Ok(entry.call(
        &mut *caller,
        scope_env.unwrap_or_else(value::encode_undefined),
    )?)
}

pub(crate) fn reserve_eval_data_segment(caller: &mut Caller<'_, RuntimeState>, code_len: u32) -> Result<u32> {
    let heap_ptr = caller
        .get_export("__heap_ptr")
        .and_then(Extern::into_global)
        .ok_or_else(|| anyhow::anyhow!("eval parent missing heap pointer"))?;
    let current = match heap_ptr.get(&mut *caller) {
        Val::I32(value) => value as u32,
        other => anyhow::bail!("eval parent heap pointer has unexpected type {other:?}"),
    };
    let base = (current + 7) & !7;
    let reserve = (constants::USER_STRING_START + code_len + 4096 + 7) & !7;
    heap_ptr.set(&mut *caller, Val::I32((base + reserve) as i32))?;
    Ok(base)
}

pub(crate) fn cached_eval_wasm(
    state: &RuntimeState,
    code: &str,
    module: &swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
    data_base: u32,
) -> Result<Vec<u8>> {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    has_scope_bridge.hash(&mut hasher);
    var_writes_to_scope.hash(&mut hasher);
    data_base.hash(&mut hasher);
    let key = hasher.finish();

    if let Some(bytes) = state
        .eval_cache
        .lock()
        .expect("eval cache mutex")
        .get(&key)
        .cloned()
    {
        return Ok(bytes);
    }

    let program = wjsm_semantic::lower_eval_module_with_scope(
        module.clone(),
        has_scope_bridge,
        var_writes_to_scope,
    )?;
    let bytes = wjsm_backend_wasm::compile_eval_at_data_base(&program, data_base)?;
    state
        .eval_cache
        .lock()
        .expect("eval cache mutex")
        .insert(key, bytes.clone());
    Ok(bytes)
}

pub(crate) fn compiled_eval_import(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
    func_ty: &FuncType,
) -> Func {
    if let Some(func) = caller.get_export(name).and_then(Extern::into_func) {
        return func;
    }

    match name {
        "string_concat" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    if value::is_string(a) || value::is_string(b) {
                        let a_s = if value::is_string(a) {
                            read_value_string_bytes(&mut caller, a).unwrap_or_default()
                        } else {
                            render_value(&mut caller, a)
                                .unwrap_or_default()
                                .into_bytes()
                        };
                        let b_s = if value::is_string(b) {
                            read_value_string_bytes(&mut caller, b).unwrap_or_default()
                        } else {
                            render_value(&mut caller, b)
                                .unwrap_or_default()
                                .into_bytes()
                        };
                        let mut result = a_s;
                        result.extend(b_s);
                        let s = String::from_utf8(result).unwrap_or_default();
                        store_runtime_string(&caller, s)
                    } else {
                        value::encode_undefined()
                    }
                },
            );
        }
        "f64_mod" => {
            return Func::wrap(
                &mut *caller,
                |_: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    value::encode_f64(f64::from_bits(a as u64) % f64::from_bits(b as u64))
                },
            );
        }
        "f64_pow" => {
            return Func::wrap(
                &mut *caller,
                |_: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    value::encode_f64(f64::from_bits(a as u64).powf(f64::from_bits(b as u64)))
                },
            );
        }
        _ => {}
    }

    let params: Vec<_> = func_ty.params().collect();
    let results: Vec<_> = func_ty.results().collect();
    let ty = FuncType::new(caller.engine(), params, results.clone());
    let name = name.to_string();
    Func::new(
        &mut *caller,
        ty,
        move |caller: Caller<'_, RuntimeState>, _params, values| {
            set_runtime_error(
                caller.data(),
                format!("RuntimeError: unsupported host import `{name}` called from compiled eval"),
            );
            for (slot, ty) in values.iter_mut().zip(results.iter()) {
                *slot = match ty {
                    ValType::I32 => Val::I32(0),
                    ValType::I64 => Val::I64(value::encode_handle(value::TAG_EXCEPTION, 0)),
                    ValType::F32 => Val::F32(0),
                    ValType::F64 => Val::F64(0),
                    ValType::V128 | ValType::Ref(_) => {
                        return Err(wasmtime::Error::msg(format!(
                            "unsupported compiled eval host result type {ty}"
                        )));
                    }
                };
            }
            Ok(())
        },
    )
}

pub(crate) fn perform_eval_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    code_value: i64,
    scope_env: Option<i64>,
) -> i64 {
    if !value::is_string(code_value) {
        return code_value;
    }

    let code = match read_value_string_bytes(caller, code_value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        Some(code) => code,
        None => return value::encode_undefined(),
    };
    if code.trim().is_empty() {
        return value::encode_undefined();
    }

    let eval_var_map = if scope_env.is_some() {
        read_eval_var_map(caller)
    } else {
        Vec::new()
    };
    let _eval_var_slots = eval_var_map
        .iter()
        .filter(|entry| {
            entry.offset % 8 == 0 && !entry.function_name.is_empty() && !entry.var_name.is_empty()
        })
        .count();

    let module = match wjsm_parser::parse_script_as_module(&code) {
        Ok(module) => module,
        Err(error) => {
            set_runtime_error(caller.data(), format!("SyntaxError: {error}"));
            return value::encode_handle(value::TAG_EXCEPTION, 0);
        }
    };
    let strict_eval_source = runtime_module_has_use_strict_directive(&module);

    let var_writes_to_scope = scope_env
        .map(|env| !strict_eval_source && !eval_scope_has_strict_marker(caller, env))
        .unwrap_or(false);

    match try_compiled_eval_from_caller(caller, &code, &module, scope_env, var_writes_to_scope) {
        Ok(value) => value,
        Err(error) => {
            set_runtime_error(caller.data(), format_eval_error(error));
            value::encode_handle(value::TAG_EXCEPTION, 0)
        }
    }
}

pub(crate) fn format_eval_error(error: anyhow::Error) -> String {
    let raw = error.to_string();
    let message = raw
        .split_once(": ")
        .and_then(|(prefix, message)| {
            prefix
                .starts_with("semantic lowering error [")
                .then_some(message)
        })
        .unwrap_or(raw.as_str());

    if message.starts_with("cannot reassign a const-declared variable") {
        let name = message
            .split_once('`')
            .and_then(|(_, rest)| rest.split_once('`'))
            .map(|(name, _)| name)
            .unwrap_or("unknown");
        format!("TypeError: assignment to constant `{name}`")
    } else if message.starts_with("assignment to constant") {
        format!("TypeError: {message}")
    } else if message.starts_with("cannot redeclare identifier") {
        let normalized = message.replace(" in the same scope", " in eval");
        format!("SyntaxError: {normalized}")
    } else if message.starts_with("const declarations must be initialised") {
        format!("SyntaxError: {message}")
    } else {
        format!("RuntimeError: {raw}")
    }
}

pub(crate) fn runtime_module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
    for item in &module.body {
        let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Expr(expr_stmt)) = item else {
            return false;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            return false;
        };
        if string.value.as_str() == Some("use strict") {
            return true;
        }
    }
    false
}

#[allow(dead_code)]
pub(crate) fn eval_module_items(
    caller: &mut Caller<'_, RuntimeState>,
    items: &[swc_ast::ModuleItem],
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    let mut completion = None;
    for item in items {
        match item {
            swc_ast::ModuleItem::Stmt(stmt) => {
                if let Some(value) =
                    eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)?
                {
                    completion = Some(value);
                }
            }
            swc_ast::ModuleItem::ModuleDecl(_) => {
                return Err(
                    "SyntaxError: import/export declarations are not valid in eval".to_string(),
                );
            }
        }
    }
    Ok(completion)
}

pub(crate) fn eval_stmt(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Empty(_) => Ok(None),
        swc_ast::Stmt::Expr(expr) => {
            Ok(Some(eval_expr(caller, &expr.expr, scope_env, eval_locals)?))
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Var(var_decl)) => {
            for declarator in &var_decl.decls {
                let Some(name) = pat_ident_name(&declarator.name) else {
                    return Err("SyntaxError: unsupported eval declaration pattern".to_string());
                };
                let value = if let Some(init) = &declarator.init {
                    eval_expr(caller, init, scope_env, eval_locals)?
                } else {
                    value::encode_undefined()
                };
                match var_decl.kind {
                    swc_ast::VarDeclKind::Var if var_writes_to_scope => {
                        if eval_locals
                            .get(name)
                            .is_some_and(|binding| !matches!(binding.kind, EvalLocalKind::Var))
                        {
                            return Err(format!(
                                "SyntaxError: cannot redeclare identifier `{name}` in eval"
                            ));
                        }
                        eval_write_binding(caller, scope_env, eval_locals, name, value)?;
                    }
                    swc_ast::VarDeclKind::Var => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
                    }
                    swc_ast::VarDeclKind::Let => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Let, value)?;
                    }
                    swc_ast::VarDeclKind::Const => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Const, value)?;
                    }
                }
            }
            Ok(None)
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl)) => {
            let function = eval_function_from_decl(fn_decl, scope_env)?;
            let value = create_eval_function(caller.data(), function);
            let name = fn_decl.ident.sym.as_ref();
            if var_writes_to_scope {
                eval_write_binding(caller, scope_env, eval_locals, name, value)?;
            } else {
                eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
            }
            Ok(None)
        }
        swc_ast::Stmt::Block(block) => eval_block(
            caller,
            &block.stmts,
            scope_env,
            var_writes_to_scope,
            eval_locals,
        ),
        swc_ast::Stmt::If(if_stmt) => {
            let test = eval_expr(caller, &if_stmt.test, scope_env, eval_locals)?;
            if !value::is_falsy(test) {
                eval_stmt(
                    caller,
                    &if_stmt.cons,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                )
            } else if let Some(alt) = &if_stmt.alt {
                eval_stmt(caller, alt, scope_env, var_writes_to_scope, eval_locals)
            } else {
                Ok(None)
            }
        }
        swc_ast::Stmt::Throw(throw_stmt) => {
            let value = eval_expr(caller, &throw_stmt.arg, scope_env, eval_locals)?;
            let rendered = render_value(caller, value).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            Err(format!("Uncaught exception: {rendered}"))
        }
        _ => Err("SyntaxError: unsupported eval statement".to_string()),
    }
}

pub(crate) fn eval_block(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    let mut completion = None;
    for stmt in stmts {
        if let Some(value) = eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)? {
            completion = Some(value);
        }
    }
    Ok(completion)
}

pub(crate) fn eval_expr(
    caller: &mut Caller<'_, RuntimeState>,
    expr: &swc_ast::Expr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    match expr {
        swc_ast::Expr::Lit(lit) => eval_lit(caller, lit),
        swc_ast::Expr::Ident(ident) => {
            Ok(
                eval_read_binding(caller, scope_env, eval_locals, ident.sym.as_ref())
                    .unwrap_or_else(value::encode_undefined),
            )
        }
        swc_ast::Expr::Paren(paren) => eval_expr(caller, &paren.expr, scope_env, eval_locals),
        swc_ast::Expr::Seq(seq) => {
            let mut result = value::encode_undefined();
            for expr in &seq.exprs {
                result = eval_expr(caller, expr, scope_env, eval_locals)?;
            }
            Ok(result)
        }
        swc_ast::Expr::Bin(bin) => {
            if matches!(
                bin.op,
                swc_ast::BinaryOp::LogicalAnd
                    | swc_ast::BinaryOp::LogicalOr
                    | swc_ast::BinaryOp::NullishCoalescing
            ) {
                return eval_logical(caller, bin, scope_env, eval_locals);
            }
            let lhs = eval_expr(caller, &bin.left, scope_env, eval_locals)?;
            let rhs = eval_expr(caller, &bin.right, scope_env, eval_locals)?;
            eval_binary(caller, bin.op, lhs, rhs)
        }
        swc_ast::Expr::Unary(unary) => {
            let val = eval_expr(caller, &unary.arg, scope_env, eval_locals)?;
            eval_unary(unary.op, val)
        }
        swc_ast::Expr::Cond(cond) => {
            let test = eval_expr(caller, &cond.test, scope_env, eval_locals)?;
            if value::is_falsy(test) {
                eval_expr(caller, &cond.alt, scope_env, eval_locals)
            } else {
                eval_expr(caller, &cond.cons, scope_env, eval_locals)
            }
        }
        swc_ast::Expr::Assign(assign) => eval_assign(caller, assign, scope_env, eval_locals),
        swc_ast::Expr::Call(call) => eval_call(caller, call, scope_env, eval_locals),
        _ => Err("SyntaxError: unsupported eval expression".to_string()),
    }
}

pub(crate) fn eval_lit(caller: &Caller<'_, RuntimeState>, lit: &swc_ast::Lit) -> Result<i64, String> {
    match lit {
        swc_ast::Lit::Str(string) => Ok(store_runtime_string(
            caller,
            string.value.to_string_lossy().into_owned(),
        )),
        swc_ast::Lit::Num(number) => Ok(value::encode_f64(number.value)),
        swc_ast::Lit::Bool(boolean) => Ok(value::encode_bool(boolean.value)),
        swc_ast::Lit::Null(_) => Ok(value::encode_null()),
        _ => Err("SyntaxError: unsupported eval literal".to_string()),
    }
}

pub(crate) fn eval_binary(
    caller: &mut Caller<'_, RuntimeState>,
    op: swc_ast::BinaryOp,
    lhs: i64,
    rhs: i64,
) -> Result<i64, String> {
    if matches!(op, swc_ast::BinaryOp::Add) && (value::is_string(lhs) || value::is_string(rhs)) {
        let lhs_string = eval_to_string(caller, lhs);
        let rhs_string = eval_to_string(caller, rhs);
        return Ok(store_runtime_string(
            caller,
            format!("{lhs_string}{rhs_string}"),
        ));
    }

    let a = eval_to_number(lhs);
    let b = eval_to_number(rhs);
    let result = match op {
        swc_ast::BinaryOp::Add => a + b,
        swc_ast::BinaryOp::Sub => a - b,
        swc_ast::BinaryOp::Mul => a * b,
        swc_ast::BinaryOp::Div => a / b,
        swc_ast::BinaryOp::Mod => a - b * (a / b).trunc(),
        swc_ast::BinaryOp::EqEq => return Ok(value::encode_bool(a == b)),
        swc_ast::BinaryOp::NotEq => return Ok(value::encode_bool(a != b)),
        swc_ast::BinaryOp::Lt => return Ok(value::encode_bool(a < b)),
        swc_ast::BinaryOp::LtEq => return Ok(value::encode_bool(a <= b)),
        swc_ast::BinaryOp::Gt => return Ok(value::encode_bool(a > b)),
        swc_ast::BinaryOp::GtEq => return Ok(value::encode_bool(a >= b)),
        _ => return Err("SyntaxError: unsupported eval binary operator".to_string()),
    };
    Ok(value::encode_f64(result))
}

pub(crate) fn eval_logical(
    caller: &mut Caller<'_, RuntimeState>,
    bin: &swc_ast::BinExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let left = eval_expr(caller, &bin.left, scope_env, eval_locals)?;
    match bin.op {
        swc_ast::BinaryOp::LogicalAnd if value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalAnd => eval_expr(caller, &bin.right, scope_env, eval_locals),
        swc_ast::BinaryOp::LogicalOr if !value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalOr => eval_expr(caller, &bin.right, scope_env, eval_locals),
        swc_ast::BinaryOp::NullishCoalescing
            if value::is_null(left) || value::is_undefined(left) =>
        {
            eval_expr(caller, &bin.right, scope_env, eval_locals)
        }
        swc_ast::BinaryOp::NullishCoalescing => Ok(left),
        _ => Err("SyntaxError: unsupported eval logical operator".to_string()),
    }
}

pub(crate) fn eval_unary(op: swc_ast::UnaryOp, val: i64) -> Result<i64, String> {
    match op {
        swc_ast::UnaryOp::Minus => Ok(value::encode_f64(-eval_to_number(val))),
        swc_ast::UnaryOp::Plus => Ok(value::encode_f64(eval_to_number(val))),
        swc_ast::UnaryOp::Bang => Ok(value::encode_bool(value::is_falsy(val))),
        swc_ast::UnaryOp::Void => Ok(value::encode_undefined()),
        _ => Err("SyntaxError: unsupported eval unary operator".to_string()),
    }
}

pub(crate) fn eval_assign(
    caller: &mut Caller<'_, RuntimeState>,
    assign: &swc_ast::AssignExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let val = eval_expr(caller, &assign.right, scope_env, eval_locals)?;
    let swc_ast::AssignTarget::Simple(simple) = &assign.left else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    let swc_ast::SimpleAssignTarget::Ident(ident) = simple else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    eval_write_binding(caller, scope_env, eval_locals, ident.id.sym.as_ref(), val)?;
    Ok(val)
}

pub(crate) fn eval_call(
    caller: &mut Caller<'_, RuntimeState>,
    call: &swc_ast::CallExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    if let swc_ast::Callee::Expr(callee) = &call.callee {
        if let swc_ast::Expr::Ident(ident) = callee.as_ref() {
            if ident.sym.as_ref() == "eval" {
                let arg = if let Some(first) = call.args.first() {
                    eval_expr(caller, &first.expr, scope_env, eval_locals)?
                } else {
                    value::encode_undefined()
                };
                return Ok(perform_eval_from_caller(caller, arg, scope_env));
            }
        }
        if let swc_ast::Expr::Member(member) = callee.as_ref() {
            if let swc_ast::Expr::Ident(obj) = member.obj.as_ref() {
                if obj.sym.as_ref() == "console" {
                    if let swc_ast::MemberProp::Ident(prop) = &member.prop {
                        if prop.sym.as_ref() == "log" {
                            let arg = if let Some(first) = call.args.first() {
                                eval_expr(caller, &first.expr, scope_env, eval_locals)?
                            } else {
                                value::encode_undefined()
                            };
                            write_console_value(caller, arg, None);
                            return Ok(value::encode_undefined());
                        }
                    }
                }
            }
        }
    }
    let swc_ast::Callee::Expr(callee_expr) = &call.callee else {
        return Err("SyntaxError: unsupported eval call".to_string());
    };
    let callee = eval_expr(caller, callee_expr.as_ref(), scope_env, eval_locals)?;
    if value::is_native_callable(callee) {
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            args.push(eval_expr(caller, &arg.expr, scope_env, eval_locals)?);
        }
        return call_native_callable_with_args_from_caller(caller, callee, value::encode_undefined(), args)
            .ok_or_else(|| "TypeError: eval callee is not callable".to_string());
    }
    Err("SyntaxError: unsupported eval call".to_string())
}

pub(crate) fn pat_ident_name(pat: &swc_ast::Pat) -> Option<&str> {
    match pat {
        swc_ast::Pat::Ident(ident) => Some(ident.id.sym.as_ref()),
        _ => None,
    }
}

pub(crate) fn eval_scope_has_strict_marker(caller: &mut Caller<'_, RuntimeState>, scope_env: i64) -> bool {
    let Some(ptr) = resolve_handle(caller, scope_env) else {
        return false;
    };
    read_object_property_by_name(caller, ptr, "__wjsm_eval_strict")
        .map(nanbox_to_bool)
        .unwrap_or(false)
}

pub(crate) fn eval_read_binding(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
    eval_locals: &HashMap<String, EvalLocalBinding>,
    name: &str,
) -> Option<i64> {
    if let Some(binding) = eval_locals.get(name) {
        return Some(binding.value);
    }
    match name {
        "undefined" => return Some(value::encode_undefined()),
        "NaN" => return Some(value::encode_f64(f64::NAN)),
        "Infinity" => return Some(value::encode_f64(f64::INFINITY)),
        _ => {}
    }
    let env = scope_env?;
    let ptr = resolve_handle(caller, env)?;
    read_object_property_by_name(caller, ptr, name)
}

pub(crate) fn eval_write_binding(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
    name: &str,
    val: i64,
) -> Result<(), String> {
    if let Some(binding) = eval_locals.get_mut(name) {
        if matches!(binding.kind, EvalLocalKind::Const) {
            return Err(format!("TypeError: assignment to constant `{name}`"));
        }
        binding.value = val;
        return Ok(());
    }
    let Some(env) = scope_env else {
        return Ok(());
    };
    let _ = set_host_data_property_from_caller(caller, env, name, val);
    Ok(())
}

pub(crate) fn eval_function_from_decl(
    fn_decl: &swc_ast::FnDecl,
    scope_env: Option<i64>,
) -> Result<EvalFunction, String> {
    let mut params = Vec::with_capacity(fn_decl.function.params.len());
    for param in &fn_decl.function.params {
        let Some(name) = pat_ident_name(&param.pat) else {
            return Err("SyntaxError: unsupported eval function parameter".to_string());
        };
        params.push(name.to_string());
    }
    let Some(body) = &fn_decl.function.body else {
        return Err("SyntaxError: eval function body is missing".to_string());
    };
    Ok(EvalFunction {
        params,
        body: body.stmts.clone(),
        scope_env,
    })
}

pub(crate) fn create_eval_function(state: &RuntimeState, function: EvalFunction) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::EvalFunction(function));
    value::encode_native_callable_idx(handle)
}

pub(crate) fn call_eval_function_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    function: EvalFunction,
    args: Vec<i64>,
) -> i64 {
    match eval_call_function(caller, &function, args) {
        Ok(value) => value,
        Err(message) => {
            set_runtime_error(caller.data(), message);
            value::encode_handle(value::TAG_EXCEPTION, 0)
        }
    }
}

pub(crate) fn eval_call_function(
    caller: &mut Caller<'_, RuntimeState>,
    function: &EvalFunction,
    args: Vec<i64>,
) -> Result<i64, String> {
    let mut locals = HashMap::new();
    for (index, param) in function.params.iter().enumerate() {
        let value = args
            .get(index)
            .copied()
            .unwrap_or_else(value::encode_undefined);
        eval_declare_local(&mut locals, param, EvalLocalKind::Var, value)?;
    }
    eval_function_block(caller, &function.body, function.scope_env, &mut locals)
        .map(|value| value.unwrap_or_else(value::encode_undefined))
}

pub(crate) fn eval_function_block(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    for stmt in stmts {
        if let Some(value) = eval_function_stmt(caller, stmt, scope_env, eval_locals)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) fn eval_function_stmt(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Return(return_stmt) => {
            let value = if let Some(arg) = &return_stmt.arg {
                eval_expr(caller, arg, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            Ok(Some(value))
        }
        swc_ast::Stmt::Block(block) => {
            eval_function_block(caller, &block.stmts, scope_env, eval_locals)
        }
        swc_ast::Stmt::If(if_stmt) => {
            let test = eval_expr(caller, &if_stmt.test, scope_env, eval_locals)?;
            if !value::is_falsy(test) {
                eval_function_stmt(caller, &if_stmt.cons, scope_env, eval_locals)
            } else if let Some(alt) = &if_stmt.alt {
                eval_function_stmt(caller, alt, scope_env, eval_locals)
            } else {
                Ok(None)
            }
        }
        _ => {
            let _ = eval_stmt(caller, stmt, scope_env, false, eval_locals)?;
            Ok(None)
        }
    }
}

pub(crate) fn eval_declare_local(
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
    name: &str,
    kind: EvalLocalKind,
    value: i64,
) -> Result<(), String> {
    if let Some(binding) = eval_locals.get_mut(name) {
        if !matches!(binding.kind, EvalLocalKind::Var) || !matches!(kind, EvalLocalKind::Var) {
            return Err(format!(
                "SyntaxError: cannot redeclare identifier `{name}` in eval"
            ));
        }
        binding.value = value;
        return Ok(());
    }
    eval_locals.insert(name.to_string(), EvalLocalBinding { kind, value });
    Ok(())
}

pub(crate) fn set_host_data_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_global(caller, name)
        .or_else(|| alloc_heap_c_string_global(caller, name))?;
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize)?;
    if let Some((slot_offset, flags, _old)) =
        find_property_slot_by_name_id(caller, obj_ptr, name_id)
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if flags & constants::FLAG_WRITABLE == 0 || slot_offset + 16 > data.len() {
            return None;
        }
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        Some(())
    } else {
        define_host_data_property_from_caller(caller, obj, name, val)
    }
}

