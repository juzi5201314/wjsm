use crate::runtime_string::RuntimeString;
use crate::*;
use wasmtime::Caller;

/// 返回当前 `unit_pos` 处完整 UTF-16 码点对应的运行时字符串值（不推进位置）。
pub(crate) fn string_iter_current_value(
    caller: &Caller<'_, RuntimeState>,
    string: &RuntimeString,
    unit_pos: usize,
) -> i64 {
    let Some(unit) = string.code_unit_at(unit_pos) else {
        return value::encode_undefined();
    };
    let width = if (0xD800..=0xDBFF).contains(&unit)
        && string
            .code_unit_at(unit_pos + 1)
            .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
    {
        2
    } else {
        1
    };
    store_runtime_string(caller, string.slice_units(unit_pos..unit_pos + width))
}

/// 将字符串迭代器 `unit_pos` 推进到下一个码点（Phase 0 纯函数，re-export builtins）。
pub(crate) use wjsm_builtins::string_iter_advance_unit_pos;

/// `IteratorValue` 宿主实现：按迭代器状态返回当前元素值
/// 被同步 `iterator_value` 与 `core_async::iterator_step_value_async` 共用
pub(crate) fn iterator_value_impl(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
    let handle_idx = value::decode_handle(handle) as usize;
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(iter) = iters.get_mut(handle_idx) {
        match iter {
            IteratorState::StringIter { string, unit_pos } => {
                if *unit_pos < string.utf16_len() {
                    string_iter_current_value(caller, string, *unit_pos)
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::ArrayIter { ptr, index, length } => {
                if *index < *length {
                    let idx = *index;
                    let arr_ptr = *ptr;
                    drop(iters);
                    read_array_elem(caller, arr_ptr, idx).unwrap_or(value::encode_undefined())
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::MapKeyIter {
                map_handle, index, ..
            } => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let val = if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        Some(entry.keys[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::MapValueIter {
                map_handle, index, ..
            } => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let val = if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        Some(entry.values[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::MapEntryIter {
                map_handle, index, ..
            } => {
                let table = caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        let key = entry.keys[idx];
                        let value = entry.values[idx];
                        drop(table);
                        drop(iters);
                        let arr = alloc_array(caller, 2);
                        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
                            write_array_elem(caller, arr_ptr, 0, key);
                            write_array_elem(caller, arr_ptr, 1, value);
                            write_array_length(caller, arr_ptr, 2);
                        }
                        arr
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                }
            }
            IteratorState::SetValueIter {
                set_handle, index, ..
            } => {
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let val = if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        Some(entry.values[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::SetEntryIter {
                set_handle, index, ..
            } => {
                let table = caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        let item = entry.values[idx];
                        drop(table);
                        drop(iters);
                        let arr = alloc_array(caller, 2);
                        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
                            write_array_elem(caller, arr_ptr, 0, item);
                            write_array_elem(caller, arr_ptr, 1, item);
                            write_array_length(caller, arr_ptr, 2);
                        }
                        arr
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                }
            }
            IteratorState::IndexValueIter { values, index } => {
                if (*index as usize) < values.len() {
                    values[*index as usize]
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::TypedArrayValueIter {
                entry,
                index,
                length,
            } => {
                if *index < *length {
                    let entry = entry.clone();
                    let idx = *index;
                    drop(iters);
                    typedarray_element_read_entry(caller, &entry, idx)
                        .unwrap_or_else(value::encode_undefined)
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::TypedArrayEntryIter {
                entry,
                index,
                length,
            } => {
                if *index < *length {
                    let typedarray_entry = entry.clone();
                    let idx = *index;
                    drop(iters);
                    let entry = alloc_array(caller, 2);
                    if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                        let elem = typedarray_element_read_entry(caller, &typedarray_entry, idx)
                            .unwrap_or_else(value::encode_undefined);
                        write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                        write_array_elem(caller, entry_ptr, 1, elem);
                        write_array_length(caller, entry_ptr, 2);
                    }
                    entry
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::RegExpStringIter { .. } => {
                let idx = handle_idx;
                drop(iters);
                regexp_string_iter_value(caller, idx)
            }
            IteratorState::ObjectIter { current_value, .. } => *current_value,
            IteratorState::Error => {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: value is not iterable".to_string());
                value::encode_undefined()
            }
        }
    } else {
        value::encode_undefined()
    }
}
