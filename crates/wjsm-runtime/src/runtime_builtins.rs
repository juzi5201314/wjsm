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
        // TODO: implement forEach/keys/values/entries — currently stubbed
        MapSetMethodKind::ForEach
        | MapSetMethodKind::Keys
        | MapSetMethodKind::Values
        | MapSetMethodKind::Entries => value::encode_undefined(),
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
        | NativeCallable::RegExpConstructor => Some(value::encode_undefined()),
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
        NativeCallable::MapConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::SetConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::WeakMapConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::WeakSetConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::DateConstructorGlobal => Some(alloc_host_object_from_caller(caller, 4)),
        NativeCallable::PromiseConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::ArrayBufferConstructorGlobal => {
            Some(alloc_host_object_from_caller(caller, 4))
        }
        NativeCallable::DataViewConstructorGlobal => Some(alloc_host_object_from_caller(caller, 4)),
        NativeCallable::TypedArrayConstructor(_) => Some(alloc_host_object_from_caller(caller, 4)),
        NativeCallable::ProxyConstructor => Some(alloc_host_object_from_caller(caller, 4)),
        NativeCallable::ProxyRevoker { proxy_handle } => {
            let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            if let Some(entry) = table.get_mut(proxy_handle as usize) {
                entry.revoked = true;
            }
            Some(value::encode_undefined())
        }
        NativeCallable::StubGlobal(_) => Some(value::encode_undefined()),
    }
}
