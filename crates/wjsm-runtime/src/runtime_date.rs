//! Date method dispatch and timezone conversion.
//!
//! Extracted from runtime_builtins.rs to concentrate all Date-related logic
//! (42 method kinds, timezone conversions via chrono, millisecond ↔ DateTime).

use super::*;

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

/// Parse a date string per ECMAScript `Date.parse` / `new Date(string)` expectations.
/// Supports ISO 8601 (RFC3339), common chrono formats, `Date.prototype.toString()` output,
/// and `Date.prototype.toUTCString()` output.
pub(crate) fn parse_date_string(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let s = s
        .trim_end()
        .trim_end_matches("(Coordinated Universal Time)")
        .trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis() as f64);
    }

    const NAIVE_FMTS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d",
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y %H:%M:%S",
    ];
    for fmt in NAIVE_FMTS {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc().timestamp_millis() as f64);
        }
        if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, fmt) {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
                return Some(ndt.and_utc().timestamp_millis() as f64);
            }
        }
    }

    // `Date.prototype.toUTCString()`: "Wed, 22 Jun 2026 12:00:00 GMT"
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%a, %d %b %Y %H:%M:%S GMT") {
        return Some(ndt.and_utc().timestamp_millis() as f64);
    }

    // `Date.prototype.toString()`: "Tue Jun 22 2026 12:00:00 GMT+0000"
    let gmt_stripped = s.replace("GMT", "");
    let gmt_stripped = gmt_stripped.trim();
    if let Ok(dt) = DateTime::parse_from_str(gmt_stripped, "%a %b %e %Y %H:%M:%S %z") {
        return Some(dt.timestamp_millis() as f64);
    }
    if let Ok(dt) = DateTime::parse_from_str(gmt_stripped, "%a %b %e %Y %H:%M:%S %:z") {
        return Some(dt.timestamp_millis() as f64);
    }

    None
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
