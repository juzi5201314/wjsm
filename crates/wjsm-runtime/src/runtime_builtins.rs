use super::*;

#[derive(Clone, Copy)]
pub(crate) enum PromiseSettlement {
    Fulfill(i64),
    Reject(i64),
}

pub(crate) fn raw_promise_handle(promise: i64) -> usize {
    if value::is_object(promise) {
        value::decode_object_handle(promise) as usize
    } else {
        promise as usize
    }
}

pub(crate) fn insert_promise_entry(
    table: &mut Vec<PromiseEntry>,
    handle: usize,
    entry: PromiseEntry,
) {
    if table.len() <= handle {
        table.resize_with(handle + 1, PromiseEntry::empty);
    }
    table[handle] = entry;
}

pub(crate) fn advance_object_iterator_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
    next: i64,
) -> (i64, i64, bool, bool) {
    let result =
        call_host_function_from_caller(caller, func_table, next, value::encode_undefined())
            .unwrap_or_else(value::encode_undefined);
    if is_promise_value(caller.data(), result) {
        return (result, value::encode_undefined(), false, false);
    }
    if (value::is_object(result) || value::is_function(result) || value::is_array(result))
        && let Some(ptr) = resolve_handle(caller, result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        let current_value = read_object_property_by_name(caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        return (result, current_value, done, true);
    }
    if !value::is_undefined(result) {
        set_runtime_error(
            caller.data(),
            "TypeError: iterator next must return an object".to_string(),
        );
    }
    (result, value::encode_undefined(), true, true)
}

pub(crate) async fn advance_object_iterator_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    func_table: &Table,
    next: i64,
) -> (i64, i64, bool, bool) {
    let result =
        call_host_function_from_caller_async(caller, func_table, next, value::encode_undefined())
            .await
            .unwrap_or_else(value::encode_undefined);
    if is_promise_value(caller.data(), result) {
        return (result, value::encode_undefined(), false, false);
    }
    if (value::is_object(result) || value::is_function(result) || value::is_array(result))
        && let Some(ptr) = resolve_handle(caller, result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        let current_value = read_object_property_by_name(caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        return (result, current_value, done, true);
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
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let Some(op) = obj_ptr else {
        return f64::NAN;
    };
    let ms_val = read_object_property_by_name(caller, op, "__date_ms__");
    ms_val.map(value::decode_f64).unwrap_or(f64::NAN)
}

pub(crate) fn write_date_ms(caller: &mut Caller<'_, RuntimeState>, this_val: i64, ms: f64) {
    if !value::is_object(this_val) {
        return;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
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
    let num_props =
        u32::from_le_bytes([data[op + 12], data[op + 13], data[op + 14], data[op + 15]]) as usize;
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
    let month_val = if args.len() > 1 {
        value::decode_f64(args[1])
    } else {
        0.0
    };
    let day = if args.len() > 2 {
        value::decode_f64(args[2])
    } else {
        1.0
    };
    let hour = if args.len() > 3 {
        value::decode_f64(args[3])
    } else {
        0.0
    };
    let minute = if args.len() > 4 {
        value::decode_f64(args[4])
    } else {
        0.0
    };
    let second = if args.len() > 5 {
        value::decode_f64(args[5])
    } else {
        0.0
    };
    let millisecond = if args.len() > 6 {
        value::decode_f64(args[6])
    } else {
        0.0
    };

    if year.is_nan()
        || month_val.is_nan()
        || day.is_nan()
        || hour.is_nan()
        || minute.is_nan()
        || second.is_nan()
        || millisecond.is_nan()
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

    let adjusted_year = if (0..=99).contains(&y) { 1900 + y } else { y };

    if is_utc {
        Utc.with_ymd_and_hms(adjusted_year, m + 1, d.max(1), h, min, s)
            .single()
            .map(|dt: DateTime<Utc>| dt.timestamp_millis() as f64 + ms as f64)
            .unwrap_or(f64::NAN)
    } else {
        Local
            .with_ymd_and_hms(adjusted_year, m + 1, d.max(1), h, min, s)
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
        DateMethodKind::GetFullYear => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.year() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetMonth => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64((dt.month0()) as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetDate => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.day() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetDay => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.weekday().num_days_from_sunday() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetHours => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.hour() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetMinutes => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.minute() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetSeconds => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64(dt.second() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetMilliseconds => match ms_to_datetime_local(ms) {
            Some(dt) => value::encode_f64((dt.nanosecond() / 1_000_000) as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCFullYear => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.year() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCMonth => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64((dt.month0()) as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCDate => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.day() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCDay => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.weekday().num_days_from_sunday() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCHours => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.hour() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCMinutes => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.minute() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCSeconds => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64(dt.second() as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::GetUTCMilliseconds => match ms_to_datetime_utc(ms) {
            Some(dt) => value::encode_f64((dt.nanosecond() / 1_000_000) as f64),
            None => value::encode_f64(f64::NAN),
        },
        DateMethodKind::SetTime => {
            let new_ms = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetDate => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let d = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if d.is_nan() {
                return value::encode_f64(f64::NAN);
            }
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
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let m = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if m.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_month0(m as u32).unwrap_or(dt);
            if let Some(d_arg) = args.get(1) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetFullYear => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let y = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if y.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_year(y as i32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_month0(m as u32).unwrap_or(new_dt);
            }
            if let Some(d_arg) = args.get(2) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetHours => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let h = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if h.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_hour(h as u32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_minute(m as u32).unwrap_or(new_dt);
            }
            if let Some(s_arg) = args.get(2) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(3) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetMinutes => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let m = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if m.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_minute(m as u32).unwrap_or(dt);
            if let Some(s_arg) = args.get(1) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(2) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetSeconds => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let s = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if s.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_local(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_second(s as u32).unwrap_or(dt);
            let new_ms = if let Some(ms_arg) = args.get(1) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetMilliseconds => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let ms_arg = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if ms_arg.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let base_ms = (ms / 1000.0).trunc() * 1000.0;
            let new_ms = base_ms + ms_arg.trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCDate => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let d = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if d.is_nan() {
                return value::encode_f64(f64::NAN);
            }
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
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let m = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if m.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_month0(m as u32).unwrap_or(dt);
            if let Some(d_arg) = args.get(1) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCFullYear => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let y = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if y.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_year(y as i32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_month0(m as u32).unwrap_or(new_dt);
            }
            if let Some(d_arg) = args.get(2) {
                let d = value::decode_f64(*d_arg);
                if d.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_day(d as u32).unwrap_or(new_dt);
            }
            let new_ms = new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCHours => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let h = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if h.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_hour(h as u32).unwrap_or(dt);
            if let Some(m_arg) = args.get(1) {
                let m = value::decode_f64(*m_arg);
                if m.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_minute(m as u32).unwrap_or(new_dt);
            }
            if let Some(s_arg) = args.get(2) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(3) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCMinutes => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let m = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if m.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let mut new_dt = dt.with_minute(m as u32).unwrap_or(dt);
            if let Some(s_arg) = args.get(1) {
                let s = value::decode_f64(*s_arg);
                if s.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt = new_dt.with_second(s as u32).unwrap_or(new_dt);
            }
            let new_ms = if let Some(ms_arg) = args.get(2) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCSeconds => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let s = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if s.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let dt = match ms_to_datetime_utc(ms) {
                Some(dt) => dt,
                None => return value::encode_f64(f64::NAN),
            };
            let new_dt = dt.with_second(s as u32).unwrap_or(dt);
            let new_ms = if let Some(ms_arg) = args.get(1) {
                let ms_val = value::decode_f64(*ms_arg);
                if ms_val.is_nan() {
                    return value::encode_f64(f64::NAN);
                }
                new_dt.timestamp_millis() as f64 + ms_val.trunc()
            } else {
                new_dt.timestamp_millis() as f64 + (ms % 1000.0).trunc()
            };
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::SetUTCMilliseconds => {
            if ms.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let ms_arg = args
                .first()
                .map(|v| value::decode_f64(*v))
                .unwrap_or(f64::NAN);
            if ms_arg.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            let base_ms = (ms / 1000.0).trunc() * 1000.0;
            let new_ms = base_ms + ms_arg.trunc();
            write_date_ms(caller, this_val, new_ms);
            value::encode_f64(new_ms)
        }
        DateMethodKind::ToString => {
            if ms.is_nan() {
                return store_runtime_string(caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%a %b %e %Y %H:%M:%S GMT%:z").to_string();
                    store_runtime_string(caller, s)
                }
                None => store_runtime_string(caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToDateString => {
            if ms.is_nan() {
                return store_runtime_string(caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%Y-%m-%d").to_string();
                    store_runtime_string(caller, s)
                }
                None => store_runtime_string(caller, "Invalid Date".to_string()),
            }
        }
        DateMethodKind::ToTimeString => {
            if ms.is_nan() {
                return store_runtime_string(caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_local(ms) {
                Some(dt) => {
                    let s = dt.format("%H:%M:%S GMT%:z").to_string();
                    store_runtime_string(caller, s)
                }
                None => store_runtime_string(caller, "Invalid Date".to_string()),
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
                    let s = if (0..=9999).contains(&year) {
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
                    store_runtime_string(caller, s)
                }
                None => value::encode_f64(f64::NAN),
            }
        }
        DateMethodKind::ToUTCString => {
            if ms.is_nan() {
                return store_runtime_string(caller, "Invalid Date".to_string());
            }
            match ms_to_datetime_utc(ms) {
                Some(dt) => {
                    let s = dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
                    store_runtime_string(caller, s)
                }
                None => store_runtime_string(caller, "Invalid Date".to_string()),
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
                    let s = if (0..=9999).contains(&year) {
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
                    store_runtime_string(caller, s)
                }
                None => value::encode_f64(f64::NAN),
            }
        }
    }
}

pub(crate) fn read_weakmap_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakmap_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

pub(crate) fn read_weakset_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
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
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakmap_table
                    .lock()
                    .expect("weakmap_table mutex");
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        }
        WeakMapMethodKind::Get => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            if handle < table.len()
                && let Some(&val) = table[handle].map.get(&key_handle)
            {
                return val;
            }
            value::encode_undefined()
        }
        WeakMapMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakMapMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakmap_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakmap_table
                .lock()
                .expect("weakmap_table mutex");
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
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller
                    .data()
                    .weakset_table
                    .lock()
                    .expect("weakset_table mutex");
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        }
        WeakSetMethodKind::Has => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller
                .data()
                .weakset_table
                .lock()
                .expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        }
        WeakSetMethodKind::Delete => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let handle = read_weakset_handle(caller, this_val).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller
                .data()
                .weakset_table
                .lock()
                .expect("weakset_table mutex");
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
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let Some(op) = obj_ptr else {
        return value::encode_undefined();
    };
    let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
    let set_handle = read_object_property_by_name(caller, op, "__set_handle__");

    match kind {
        MapSetMethodKind::MapSet => {
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
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
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
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
            let val = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
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
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
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
            let key = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
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
        MapSetMethodKind::ForEach => value::encode_undefined(),
        MapSetMethodKind::Keys => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let keys = table[handle].keys.clone();
                    drop(table);
                    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapKeyIter { keys, index: 0 });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Values => {
            if let Some(mh) = map_handle {
                let handle = value::decode_f64(mh) as usize;
                let table = caller.data().map_table.lock().expect("map table mutex");
                if handle < table.len() {
                    let values = table[handle].values.clone();
                    drop(table);
                    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                    let iter_handle = iters.len() as u32;
                    iters.push(IteratorState::MapValueIter { values, index: 0 });
                    return value::encode_handle(value::TAG_ITERATOR, iter_handle);
                }
            }
            value::encode_undefined()
        }
        MapSetMethodKind::Entries => value::encode_undefined(),
    }
}

pub(crate) fn call_native_callable_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    argument: Option<i64>,
) -> Option<i64> {
    call_native_callable_with_args_from_caller(
        caller,
        callable,
        value::encode_undefined(),
        argument.into_iter().collect(),
    )
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
            recycle_native_callable(caller.data(), callable);
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
                    entry.queue.push_back(request);
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
        NativeCallable::MapSetMethod { kind } => Some(call_map_set_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::DateMethod { kind } => {
            Some(call_date_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::WeakMapMethod { kind } => Some(call_weakmap_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::WeakSetMethod { kind } => Some(call_weakset_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::ArrayConstructor => {
            if value::is_object(this_val) {
                Some(this_val)
            } else {
                Some({
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_object(caller, &_wjsm_env, 4)
                })
            }
        }
        NativeCallable::ObjectConstructor => {
            if value::is_object(this_val) || value::is_function(this_val) {
                Some(this_val)
            } else {
                Some({
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_object(caller, &_wjsm_env, 4)
                })
            }
        }
        NativeCallable::FunctionConstructor
        | NativeCallable::StringConstructor
        | NativeCallable::BooleanConstructor
        | NativeCallable::NumberConstructor
        | NativeCallable::BigIntConstructor
        | NativeCallable::RegExpConstructor => Some(value::encode_undefined()),
        NativeCallable::SymbolConstructor => Some({
            let desc = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let description = if value::is_undefined(desc) {
                None
            } else if value::is_string(desc) {
                Some(get_string_value(caller, desc))
            } else {
                Some(
                    render_value(caller, desc)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string(),
                )
            };
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description,
                global_key: None,
            });
            value::encode_symbol_handle(handle)
        }),
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
            let msg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(create_error_object(caller, error_name, msg))
        }
        NativeCallable::MapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::SetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakMapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakSetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakRefConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::FinalizationRegistryConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::DateConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::PromiseConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::ArrayBufferConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::DataViewConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::TypedArrayConstructor(_) => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::BigInt64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            4,
            Some(this_val),
        )),
        NativeCallable::BigUint64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            5,
            Some(this_val),
        )),
        NativeCallable::ProxyConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::ProxyRevoker { proxy_handle } => {
            let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            if let Some(entry) = table.get_mut(proxy_handle as usize) {
                entry.revoked = true;
            }
            Some(value::encode_undefined())
        }
        NativeCallable::WeakRefDerefMethod => Some(weakref_deref_impl(caller, this_val)),
        NativeCallable::FinalizationRegistryRegisterMethod => {
            Some(fr_register_impl_with_args(caller, this_val, args))
        }
        NativeCallable::FinalizationRegistryUnregisterMethod => {
            let token = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(fr_unregister_impl(caller, this_val, token))
        }
        NativeCallable::StubGlobal(_) => Some(value::encode_undefined()),
        NativeCallable::GcCollect => {
            trigger_gc(caller);
            Some(value::encode_undefined())
        }
        NativeCallable::SharedArrayBufferConstructor => {
            let length = argument;
            let options = args
                .get(1)
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(crate::shared_buffer::construct_shared_array_buffer(
                caller, length, options, this_val,
            ))
        }
        // ── Agent harness ──
        NativeCallable::AgentStart => {
            // Simplified: no-op for now (would parse script and spawn thread)
            Some(value::encode_undefined())
        }
        NativeCallable::AgentBroadcast => Some(value::encode_undefined()),
        NativeCallable::AgentReceiveBroadcast => Some(value::encode_undefined()),
        NativeCallable::AgentGetReport => {
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return Some(value::encode_undefined()),
            };
            let report = shared.agent_state.reports.lock().unwrap().pop();
            match report {
                Some(r) => Some(store_runtime_string(caller, r)),
                None => Some(value::encode_null()),
            }
        }
        NativeCallable::AgentSleep => {
            let ms = args.first().copied().map(value::decode_f64).unwrap_or(0.0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Some(value::encode_undefined())
        }
        NativeCallable::AgentMonotonicNow => {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let start = START.get_or_init(std::time::Instant::now);
            Some(value::encode_f64(start.elapsed().as_millis() as f64))
        }
        NativeCallable::AtomicsGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        // ── Async iterator methods ──
        NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => Some(this_val),
        NativeCallable::AsyncFromSyncNext { handle } => {
            Some(advance_async_from_sync(caller, handle))
        }
        NativeCallable::AsyncFromSyncReturn { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                let result = alloc_iterator_result_from_caller(caller, arg, true);
                resolve_promise_from_caller(caller, promise, result);
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            Some(call_sync_iter_and_wrap(
                caller,
                sync_iter_handle,
                Some(arg),
                false,
            ))
        }
        NativeCallable::AsyncFromSyncThrow { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (_sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
            Some(promise)
        }
        NativeCallable::HeadersMethod { kind, .. } => {
            call_headers_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::ResponseMethod { kind, .. } => {
            call_response_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::RequestMethod { kind, .. } => {
            call_request_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::HeadersConstructor => construct_headers(caller, this_val, &args),
        NativeCallable::ResponseConstructor => construct_response(caller, this_val, &args),
        NativeCallable::RequestConstructor => construct_request(caller, this_val, &args),
        NativeCallable::StreamMethod { handle, kind } => {
            call_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReaderMethod { handle, kind } => {
            call_reader_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::AbortControllerConstructor => {
            construct_abort_controller(caller, this_val, &args)
        }
        NativeCallable::AbortControllerAbort { signal_handle } => {
            abort_controller_abort(caller, signal_handle, &args)
        }
        // ── ReadableStream (WHATWG Streams Phase 1) ──
        // ReadableStreamConstructor is async-only: routed through the host-import
        // `readable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::ReadableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::ReadableStreamMethod { handle, kind } => {
            call_readable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultReaderMethod { handle, kind } => {
            call_default_reader_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind } => {
            call_default_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── ReadableStream async iterator (WHATWG Streams Phase 2) ──
        NativeCallable::ReadableStreamAsyncIteratorNext { reader_handle } => {
            call_default_reader_method_from_caller(
                caller,
                this_val,
                reader_handle,
                ReadableStreamDefaultReaderMethodKind::Read,
                &args,
            )
        }
        NativeCallable::ReadableStreamAsyncIteratorReturn { reader_handle } => {
            // releaseLock：释放流的锁定
            let stream_handle = {
                let reader_table = caller.data().reader_table.lock().expect("reader mutex");
                reader_table
                    .get(reader_handle as usize)
                    .map(|e| e.stream_handle)
            };
            if let Some(sh) = stream_handle {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                if let Some(entry) = stream_table.get_mut(sh as usize) {
                    entry.locked = false;
                }
            }
            // 返回 {done: true, value: undefined} 作为 resolved Promise
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let result = build_reader_result(caller, true, None);
            settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
            Some(p)
        }
        // ── WritableStream (WHATWG Streams Phase 4) ──
        // WritableStreamConstructor is async-only: routed through the host-import
        // `writable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::WritableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::WritableStreamMethod { handle, kind } => {
            call_writable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultWriterMethod { handle, kind } => {
            call_default_writer_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultControllerMethod { handle, kind } => {
            call_writable_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── TransformStream (WHATWG Streams Phase 5) ──
        // TransformStreamConstructor is async-only: routed through the host-import
        // `transform_stream_constructor`. It is never dispatched via the sync NativeCallable path.
        NativeCallable::TransformStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::TransformStreamMethod { handle, kind } => {
            call_transform_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── QueuingStrategy (WHATWG Streams Phase 2) ──
        NativeCallable::CountQueuingStrategyConstructor => {
            construct_count_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::ByteLengthQueuingStrategyConstructor => {
            construct_byte_length_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::QueuingStrategySize { kind } => {
            call_queuing_strategy_size_from_caller(caller, kind, &args)
        }
    }
}

pub(crate) async fn call_native_callable_with_args_from_caller_async(
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
        NativeCallable::EvalIndirect => {
            Some(perform_eval_from_caller_async(caller, argument, None).await)
        }
        NativeCallable::EvalFunction(function) => {
            Some(call_eval_function_from_caller_async(caller, function, args).await)
        }
        _ => call_native_callable_with_args_from_caller(caller, callable, this_val, args),
    }
}
/// 创建 AsyncFromSyncIterator：将同步迭代器包装为异步迭代器协议。
/// 在 iterators 表中注册 ObjectIter（next/return 为 NativeCallable），
/// 返回 TAG_ITERATOR 句柄供 for-await 使用。
pub(crate) fn create_async_from_sync_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
) -> i64 {
    // 注册 async-from-sync iterator 状态条目
    let table_idx = {
        let mut table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .expect("async-from-sync iterators mutex");
        let idx = table.len() as u32;
        table.push(AsyncFromSyncIteratorEntry {
            sync_iterator: sync_iter_handle,
            sync_done: false,
        });
        idx
    };

    // 创建 NativeCallable 包装 next/return/throw
    let next_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables mutex");
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncNext { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };
    let return_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables mutex");
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncReturn { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };

    // 注册为 ObjectIter（next/return 为 NativeCallable）
    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
    let iter_handle = iters.len() as u32;
    iters.push(IteratorState::ObjectIter {
        next: next_callable,
        return_method: Some(return_callable),
        current_value: value::encode_undefined(),
        has_current: false,
        done: false,
    });
    value::encode_handle(value::TAG_ITERATOR, iter_handle)
}

/// 调用同步迭代器的方法并将结果包装为 resolved Promise。
fn call_sync_iter_and_wrap(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
    arg_if_return: Option<i64>,
    is_throw: bool,
) -> i64 {
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    let table = caller.get_export("__table").and_then(|e| e.into_table());
    let Some(func_table) = table else {
        return value::encode_undefined();
    };

    let method_to_call = {
        let iters = caller.data().iterators.lock().expect("iterators mutex");
        match iters.get(sync_handle_idx) {
            Some(IteratorState::ObjectIter {
                next,
                return_method,
                ..
            }) => {
                if arg_if_return.is_some() {
                    return_method.unwrap_or(*next)
                } else {
                    *next
                }
            }
            _ => return value::encode_undefined(),
        }
    };

    if is_throw {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        settle_promise(
            caller.data(),
            promise,
            PromiseSettlement::Reject(arg_if_return.unwrap_or(value::encode_undefined())),
        );
        return promise;
    }

    let call_arg = arg_if_return.unwrap_or(value::encode_undefined());
    let raw_result = call_host_function_from_caller(caller, &func_table, method_to_call, call_arg)
        .unwrap_or(value::encode_undefined());

    let (done, current_value) = if (value::is_object(raw_result)
        || value::is_function(raw_result)
        || value::is_array(raw_result))
        && let Some(ptr) = resolve_handle(caller, raw_result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        let value =
            read_object_property_by_name(caller, ptr, "value").unwrap_or(value::encode_undefined());
        (done, value)
    } else {
        (true, call_arg)
    };

    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let result = alloc_iterator_result_from_caller(caller, current_value, done);
    resolve_promise_from_caller(caller, promise, result);
    promise
}

/// AsyncFromSyncIterator.next()：推进同步迭代器并返回 Promise<IteratorResult>。
fn advance_async_from_sync(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> i64 {
    let (sync_iter_handle, sync_done) = {
        let table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .expect("async-from-sync iterators mutex");
        let entry = match table.get(handle as usize) {
            Some(e) => e,
            None => return value::encode_undefined(),
        };
        (entry.sync_iterator, entry.sync_done)
    };

    if sync_done {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    // Direct advancement for non-ObjectIter types
    let direct_result = {
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
        match iters.get_mut(sync_handle_idx) {
            Some(IteratorState::ArrayIter { ptr, index, length }) => {
                if *index < *length {
                    let idx = *index;
                    let array_ptr = *ptr;
                    *index += 1;
                    drop(iters);
                    let val = read_array_elem(caller, array_ptr, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapKeyIter { keys, index }) => {
                if (*index as usize) < keys.len() {
                    let val = keys[*index as usize];
                    *index += 1;
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapValueIter { values, index }) => {
                if (*index as usize) < values.len() {
                    let val = values[*index as usize];
                    *index += 1;
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayValueIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let val = typedarray_element_read_entry(caller, &entry, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayEntryIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let typedarray_entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let entry = alloc_array(caller, 2);
                    if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                        let elem = typedarray_element_read_entry(caller, &typedarray_entry, idx)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                        write_array_elem(caller, entry_ptr, 1, elem);
                        write_array_length(caller, entry_ptr, 2);
                    }
                    Some((false, entry))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::StringIter { byte_pos, data }) => {
                if *byte_pos < data.len() {
                    let ch = data[*byte_pos] as char;
                    *byte_pos += 1;
                    drop(iters);
                    let val = store_runtime_string(&caller, ch.to_string());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            _ => None,
        }
    };

    if let Some((done, current_value)) = direct_result {
        if done {
            let mut table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .expect("async-from-sync iterators mutex");
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.sync_done = true;
            }
        }
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, current_value, done);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }

    let promise = call_sync_iter_and_wrap(caller, sync_iter_handle, None, false);

    {
        let iters = caller.data().iterators.lock().expect("iterators mutex");
        if let Some(IteratorState::ObjectIter { done, .. }) = iters.get(sync_handle_idx) {
            if *done {
                drop(iters);
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
        }
    }

    promise
}
pub(crate) fn weakref_deref_impl(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val =
        obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "__weakref_handle__"));
    let handle = handle_val
        .map(|v| value::decode_f64(v) as usize)
        .unwrap_or(0);
    let table = caller
        .data()
        .weakref_table
        .lock()
        .expect("weakref table mutex");
    if handle >= table.len() {
        return value::encode_undefined();
    }
    let entry = &table[handle];
    if entry.target_handle == 0 {
        return value::encode_undefined();
    }
    value::encode_object_handle(entry.target_handle)
}
pub(crate) fn fr_unregister_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    token: i64,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_bool(false);
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_bool(false);
    };
    let mut table = caller
        .data()
        .finalization_registry_table
        .lock()
        .expect("finalization registry table mutex");
    if handle >= table.len() {
        return value::encode_bool(false);
    }
    let entry = &mut table[handle];
    let initial_len = entry.registrations.len();
    entry.registrations.retain(|r| match &r.unregister_token {
        Some(t) => !same_value_zero(*t, token),
        None => true,
    });
    value::encode_bool(entry.registrations.len() < initial_len)
}
pub(crate) fn fr_register_impl_with_args(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: Vec<i64>,
) -> i64 {
    if args.len() < 2 {
        return value::encode_undefined();
    }
    let target = args[0];
    let held_value = args[1];
    let unregister_token = if args.len() >= 3 {
        let token = args[2];
        if value::is_js_object(token) || value::is_symbol(token) {
            Some(token)
        } else {
            None
        }
    } else {
        None
    };
    if !value::is_js_object(target) {
        return value::encode_undefined();
    }
    let target_handle = match resolve_handle(caller, target) {
        Some(ptr) => ptr as u32,
        None => return value::encode_undefined(),
    };
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_undefined();
    };
    {
        let mut table = caller
            .data()
            .finalization_registry_table
            .lock()
            .expect("finalization registry table mutex");
        if handle < table.len() {
            table[handle].registrations.push(FinalizationRegistration {
                target_handle,
                held_value,
                unregister_token,
            });
        }
    }
    value::encode_undefined()
}
/// Trigger a mark-sweep GC cycle.
pub(crate) fn trigger_gc(caller: &mut Caller<'_, RuntimeState>) {
    // Helper: read an i32 WASM global by name
    fn get_global_i32(caller: &mut Caller<'_, RuntimeState>, name: &str) -> i32 {
        match caller.get_export(name) {
            Some(Extern::Global(g)) => g.get(caller).i32().unwrap_or(0),
            _ => 0,
        }
    }

    // Get WASM globals
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return;
    };
    let heap_ptr = get_global_i32(caller, "__heap_ptr");
    let obj_table_ptr = get_global_i32(caller, "__obj_table_ptr");
    let obj_table_count = get_global_i32(caller, "__obj_table_count");
    let object_heap_start = get_global_i32(caller, "__object_heap_start");
    let num_ir_functions = get_global_i32(caller, "__num_ir_functions");
    let shadow_sp = get_global_i32(caller, "__shadow_sp");

    if heap_ptr == 0 || obj_table_count == 0 {
        return;
    }

    // Initialize/clear mark bits
    {
        let mut mark_bits = caller.data().gc_mark_bits.lock().expect("gc_mark_bits");
        let needed_words = (obj_table_count as usize).div_ceil(64).max(mark_bits.len());
        if mark_bits.len() < needed_words {
            mark_bits.resize(needed_words, 0);
        } else {
            mark_bits.fill(0);
        }
    }

    // Build root set
    let oht_usize = object_heap_start as usize;
    let opt_usize = obj_table_ptr as usize;
    let mut roots: Vec<(usize, usize)> = Vec::new();
    let data = memory.data(&caller);
    let shadow_stack_base = oht_usize.saturating_sub(65536);

    let add_root = |handle_idx: usize, data: &[u8], roots: &mut Vec<(usize, usize)>| {
        let slot_addr = opt_usize + handle_idx * 4;
        if slot_addr + 4 <= data.len() {
            let obj_ptr = u32::from_le_bytes([
                data[slot_addr],
                data[slot_addr + 1],
                data[slot_addr + 2],
                data[slot_addr + 3],
            ]) as usize;
            if obj_ptr != 0 {
                roots.push((handle_idx, obj_ptr));
            }
        }
    };

    // 3a. Shadow stack
    let shadow_sp_usize = shadow_sp as usize;
    if shadow_sp_usize > shadow_stack_base {
        let frame_count = (shadow_sp_usize - shadow_stack_base) / 8;
        for frame in 0..frame_count {
            let frame_addr = shadow_stack_base + frame * 8;
            if frame_addr + 8 <= data.len() {
                let val = i64::from_le_bytes([
                    data[frame_addr],
                    data[frame_addr + 1],
                    data[frame_addr + 2],
                    data[frame_addr + 3],
                    data[frame_addr + 4],
                    data[frame_addr + 5],
                    data[frame_addr + 6],
                    data[frame_addr + 7],
                ]);
                if value::is_object(val) {
                    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                    add_root(handle_idx, data, &mut roots);
                } else if value::is_function(val) {
                    let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                    if func_idx < num_ir_functions as usize {
                        add_root(func_idx, data, &mut roots);
                    }
                } else if value::is_closure(val) {
                    let closure_idx = value::decode_closure_idx(val) as usize;
                    let closures = caller.data().closures.lock().expect("closures");
                    if let Some(entry) = closures.get(closure_idx)
                        && value::is_object(entry.env_obj)
                    {
                        let handle_idx = value::decode_object_handle(entry.env_obj) as usize;
                        add_root(handle_idx, data, &mut roots);
                    }
                }
            }
        }
    }

    // 3b. IR function property objects
    for handle_idx in 0..num_ir_functions as usize {
        add_root(handle_idx, data, &mut roots);
    }

    // 3c. Timer callbacks
    {
        let timers = caller.data().timers.lock().expect("timers");
        for timer in timers.iter() {
            let val = timer.callback;
            if value::is_function(val) {
                let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                if func_idx < num_ir_functions as usize {
                    add_root(func_idx, data, &mut roots);
                }
            } else if value::is_closure(val) {
                let closure_idx = value::decode_closure_idx(val) as usize;
                let closures = caller.data().closures.lock().expect("closures");
                if let Some(entry) = closures.get(closure_idx)
                    && value::is_object(entry.env_obj)
                {
                    let handle_idx = value::decode_object_handle(entry.env_obj) as usize;
                    add_root(handle_idx, data, &mut roots);
                }
            }
        }
    }

    // 3d. Closure env_obj
    {
        let closures = caller.data().closures.lock().expect("closures");
        for entry in closures.iter() {
            if value::is_object(entry.env_obj) {
                let handle_idx = value::decode_object_handle(entry.env_obj) as usize;
                add_root(handle_idx, data, &mut roots);
            }
        }
    }

    // 3e. Module namespace cache
    {
        let cache = caller
            .data()
            .module_namespace_cache
            .lock()
            .expect("module cache");
        for &val in cache.values() {
            if value::is_object(val) {
                let handle_idx = value::decode_object_handle(val) as usize;
                add_root(handle_idx, data, &mut roots);
            }
        }
    }

    // Deduplicate roots
    roots.sort();
    roots.dedup_by_key(|&mut (handle_idx, _)| handle_idx);
    let _ = data;

    // Phase 1: Mark
    for (handle_idx, obj_ptr) in roots {
        mark_object_recursive(
            caller,
            handle_idx,
            obj_ptr,
            opt_usize,
            obj_table_count as usize,
        );
    }

    // Phase 2: Sweep + Compact
    let mark_snapshot: Vec<u64> = {
        let mark_bits = caller.data().gc_mark_bits.lock().expect("gc_mark_bits");
        mark_bits.clone()
    };
    let heap_base = oht_usize;

    // Collect live objects (reading from memory without holding caller borrow)
    let mut live_objects: Vec<(usize, usize, usize)> = Vec::new();
    {
        let data = memory.data(&caller);
        for handle_idx in 0..obj_table_count as usize {
            let word_idx = handle_idx / 64;
            let bit_idx = handle_idx % 64;
            if word_idx < mark_snapshot.len() && (mark_snapshot[word_idx] & (1u64 << bit_idx)) != 0
            {
                let slot_addr = opt_usize + handle_idx * 4;
                if slot_addr + 4 > data.len() {
                    continue;
                }
                let old_ptr = u32::from_le_bytes([
                    data[slot_addr],
                    data[slot_addr + 1],
                    data[slot_addr + 2],
                    data[slot_addr + 3],
                ]) as usize;
                if old_ptr == 0 {
                    continue;
                }
                if old_ptr + 16 > data.len() {
                    continue;
                }
                let heap_type = data[old_ptr + 4];
                let (capacity, elem_size) = if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
                    (
                        u32::from_le_bytes([
                            data[old_ptr + 12],
                            data[old_ptr + 13],
                            data[old_ptr + 14],
                            data[old_ptr + 15],
                        ]) as usize,
                        8usize,
                    )
                } else {
                    (
                        u32::from_le_bytes([
                            data[old_ptr + 8],
                            data[old_ptr + 9],
                            data[old_ptr + 10],
                            data[old_ptr + 11],
                        ]) as usize,
                        32usize,
                    )
                };
                let Some(payload_size) = capacity.checked_mul(elem_size) else {
                    continue;
                };
                let Some(size) = 16usize.checked_add(payload_size) else {
                    continue;
                };
                live_objects.push((handle_idx, old_ptr, size));
            }
        }
    }

    // Sort by old pointer
    live_objects.sort_by_key(|&(_, old_ptr, _)| old_ptr);

    // Compact objects to heap_base
    let mut current_ptr = heap_base;
    for (_, _, size) in &live_objects {
        current_ptr += size;
    }
    let new_heap_end = current_ptr;

    // Move objects (independent data_mut calls, no held borrows)
    let mut current_ptr = heap_base;
    for &(handle_idx, old_ptr, size) in &live_objects {
        if old_ptr != current_ptr
            && old_ptr < heap_ptr as usize
            && current_ptr + size <= heap_ptr as usize
        {
            unsafe {
                let data_ptr = memory.data_mut(&mut *caller);
                std::ptr::copy(
                    data_ptr.as_ptr().add(old_ptr),
                    data_ptr.as_mut_ptr().add(current_ptr),
                    size,
                );
            }
        }
        // Update handle table
        {
            let data = memory.data_mut(&mut *caller);
            if opt_usize + handle_idx * 4 + 4 <= data.len() {
                data[opt_usize + handle_idx * 4..opt_usize + handle_idx * 4 + 4]
                    .copy_from_slice(&(current_ptr as u32).to_le_bytes());
            }
        }
        current_ptr += size;
    }

    // Update heap_ptr global
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        g.set(&mut *caller, Val::I32(new_heap_end as i32)).ok();
    }

    // Reset alloc counter
    *caller.data().alloc_counter.lock().expect("alloc_counter") = 0;
}
