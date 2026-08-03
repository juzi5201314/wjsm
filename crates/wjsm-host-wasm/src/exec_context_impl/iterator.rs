// ExecContext 方法片段：iterator
macro_rules! exec_ctx_iterator {
    () => {
    fn create_enumerator(&mut self, val: Value) -> Value {
        if let Some(string_data) = crate::runtime_render::read_value_string_bytes(self.caller, val)
        {
            let len = string_data.len();
            let mut enums = self
                .caller
                .data()
                .enumerators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = enums.len() as u32;
            enums.push(EnumeratorState::StringEnum {
                length: len,
                index: 0,
            });
            return value::encode_handle(value::TAG_ENUMERATOR, handle);
        }
        if value::is_object(val) || value::is_function(val) || value::is_array(val) {
            let keys = crate::runtime_values::enumerate_object_keys(self.caller, val);
            let mut enums = self
                .caller
                .data()
                .enumerators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = enums.len() as u32;
            enums.push(EnumeratorState::ObjectEnum { keys, index: 0 });
            return value::encode_handle(value::TAG_ENUMERATOR, handle);
        }
        if value::is_f64(val) || value::is_bool(val) {
            let mut enums = self
                .caller
                .data()
                .enumerators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let handle = enums.len() as u32;
            enums.push(EnumeratorState::StringEnum {
                length: 0,
                index: 0,
            });
            return value::encode_handle(value::TAG_ENUMERATOR, handle);
        }
        let mut enums = self
            .caller
            .data()
            .enumerators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = enums.len() as u32;
        enums.push(EnumeratorState::Error);
        value::encode_handle(value::TAG_ENUMERATOR, handle)
    }
    fn enumerator_advance(&mut self, handle: Handle) {
        let mut enums = self
            .caller
            .data()
            .enumerators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(enm) = enums.get_mut(handle as usize) {
            match enm {
                EnumeratorState::StringEnum { length, index } => {
                    if *index < *length {
                        *index += 1;
                    }
                }
                EnumeratorState::ObjectEnum { keys, index } => {
                    if *index < keys.len() {
                        *index += 1;
                    }
                }
                EnumeratorState::Error => {}
            }
        }
    }
    fn enumerator_key(&mut self, handle: Handle) -> Value {
        let mut enums = self
            .caller
            .data()
            .enumerators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(enm) = enums.get_mut(handle as usize) {
            match enm {
                EnumeratorState::StringEnum { index, .. } => {
                    let key = index.to_string();
                    drop(enums);
                    return crate::runtime_render::store_runtime_string(self.caller, key);
                }
                EnumeratorState::ObjectEnum { keys, index } => {
                    let key = keys.get(*index).cloned().unwrap_or_default();
                    drop(enums);
                    return crate::runtime_render::store_runtime_string(self.caller, key);
                }
                EnumeratorState::Error => {
                    *self
                        .caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) =
                        Some("TypeError: value is not enumerable".to_string());
                    return value::encode_undefined();
                }
            }
        }
        value::encode_undefined()
    }
    fn enumerator_done(&mut self, handle: Handle) -> bool {
        let enums = self
            .caller
            .data()
            .enumerators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match enums.get(handle as usize) {
            Some(EnumeratorState::StringEnum { length, index }) => *index >= *length,
            Some(EnumeratorState::ObjectEnum { keys, index }) => *index >= keys.len(),
            Some(EnumeratorState::Error) | None => true,
        }
    }
    fn create_array_iterator(&mut self, arr: Value) -> Value {
        let Some(ptr) = crate::resolve_handle(self.caller, arr) else {
            return value::encode_undefined();
        };
        let length = crate::read_array_length(self.caller, ptr).unwrap_or(0);
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(crate::IteratorState::ArrayIter {
            ptr,
            index: 0,
            length,
        });
        value::encode_handle(value::TAG_ITERATOR, handle)
    }
    fn try_create_set_iterator(&mut self, val: Value) -> Option<Value> {
        if !(value::is_object(val) || value::is_function(val)) {
            return None;
        }
        let ptr = crate::resolve_handle(self.caller, val)?;
        let sh = crate::read_object_property_by_name(self.caller, ptr, "__set_handle__")?;
        let set_handle_u32 = value::decode_f64(sh) as u32;
        {
            let table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if (set_handle_u32 as usize) >= table.len() {
                return None;
            }
        }
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(crate::IteratorState::SetValueIter {
            set_handle: set_handle_u32,
            owner: val,
            index: 0,
        });
        Some(value::encode_handle(value::TAG_ITERATOR, handle))
    }
    fn create_object_iterator(&mut self, iterator: Value) -> Value {
        let Some(iter_ptr) = crate::resolve_handle(self.caller, iterator) else {
            return value::encode_undefined();
        };
        let Some(next) = crate::read_object_property_by_name(self.caller, iter_ptr, "next") else {
            return value::encode_undefined();
        };
        if !value::is_callable(next) {
            return value::encode_undefined();
        }
        let return_method = crate::read_object_property_by_name(self.caller, iter_ptr, "return")
            .filter(|c| value::is_callable(*c));
        let throw_method = crate::read_object_property_by_name(self.caller, iter_ptr, "throw")
            .filter(|c| value::is_callable(*c));
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(crate::IteratorState::ObjectIter {
            iterator,
            next,
            return_method,
            throw_method,
            current_value: value::encode_undefined(),
            has_current: false,
            done: false,
        });
        value::encode_handle(value::TAG_ITERATOR, handle)
    }
    fn create_error_iterator(&mut self) -> Value {
        let mut iterators = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iterators.len() as u32;
        iterators.push(crate::IteratorState::Error);
        value::encode_handle(value::TAG_ITERATOR, handle)
    }
    fn iterator_next_sync_step(&mut self, handle: Value) -> IteratorNextStep {
        if let Some(afs) = self.iterator_lookup_afs(handle) {
            return IteratorNextStep::NeedAsyncFromSync { afs };
        }
        let handle_idx = value::decode_handle(handle) as usize;
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(iter) = iters.get_mut(handle_idx) else {
            return IteratorNextStep::Missing;
        };
        match iter {
            crate::IteratorState::StringIter { string, unit_pos } => {
                wjsm_builtins::string_iter_advance_unit_pos(string, unit_pos);
                IteratorNextStep::Advanced
            }
            crate::IteratorState::ArrayIter { index, .. }
            | crate::IteratorState::IndexValueIter { index, .. }
            | crate::IteratorState::TypedArrayValueIter { index, .. }
            | crate::IteratorState::TypedArrayEntryIter { index, .. } => {
                *index += 1;
                IteratorNextStep::Advanced
            }
            crate::IteratorState::MapKeyIter {
                index, map_handle, ..
            }
            | crate::IteratorState::MapValueIter {
                index, map_handle, ..
            }
            | crate::IteratorState::MapEntryIter {
                index, map_handle, ..
            } => {
                // value() 已把 index 推进到已消费槽位之后；此处仅跳过剩余 tombstone。
                let table = self
                    .caller
                    .data()
                    .map_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    while *index < entry.keys.len() as u32 && entry.deleted[*index as usize] {
                        *index += 1;
                    }
                }
                IteratorNextStep::Advanced
            }
            crate::IteratorState::SetValueIter {
                index, set_handle, ..
            }
            | crate::IteratorState::SetEntryIter {
                index, set_handle, ..
            } => {
                // value() 已把 index 推进到已消费槽位之后；此处仅跳过剩余 tombstone。
                let table = self
                    .caller
                    .data()
                    .set_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    while *index < entry.values.len() as u32 && entry.deleted[*index as usize] {
                        *index += 1;
                    }
                }
                IteratorNextStep::Advanced
            }
            crate::IteratorState::RegExpStringIter { .. } => {
                drop(iters);
                crate::regexp_string_iter_next(self.caller, handle_idx);
                IteratorNextStep::Advanced
            }
            crate::IteratorState::ObjectIter { iterator, next, .. } => {
                let iterator = *iterator;
                let next = *next;
                drop(iters);
                if let Some(afs) = wjsm_builtins::core_async::resolve_async_from_sync_afs_handle(
                    self, handle, next,
                ) {
                    return IteratorNextStep::NeedAsyncFromSync { afs };
                }
                IteratorNextStep::NeedObjectNext { iterator, next }
            }
            crate::IteratorState::Error => IteratorNextStep::ErrorDone,
        }
    }
    fn iterator_store_object_current(
        &mut self,
        handle: Value,
        current: Value,
        done: bool,
        has_current: bool,
    ) {
        let handle_idx = value::decode_handle(handle) as usize;
        if let Some(crate::IteratorState::ObjectIter {
            current_value,
            done: stored_done,
            has_current: stored_has_current,
            ..
        }) = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(handle_idx)
        {
            *current_value = current;
            *stored_done = done;
            *stored_has_current = has_current;
        }
    }
    fn iterator_done_sync(&mut self, handle: Value) -> Option<bool> {
        let handle_idx = value::decode_handle(handle) as usize;
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(iter) = iters.get_mut(handle_idx) else {
            return Some(true);
        };
        match iter {
            crate::IteratorState::StringIter { string, unit_pos } => {
                Some(*unit_pos >= string.utf16_len())
            }
            crate::IteratorState::ArrayIter { index, length, .. } => {
                Some(*index as usize >= *length as usize)
            }
            crate::IteratorState::ObjectIter {
                done, has_current, ..
            } => {
                if *done {
                    return Some(true);
                }
                if *has_current {
                    return Some(*done);
                }
                None
            }
            crate::IteratorState::Error => {
                drop(iters);
                crate::set_runtime_error(
                    self.caller.data(),
                    "TypeError: value is not iterable".to_string(),
                );
                Some(true)
            }
            crate::IteratorState::RegExpStringIter { .. } => {
                drop(iters);
                Some(crate::regexp_string_iter_ensure_current(
                    self.caller,
                    handle_idx,
                ))
            }
            // 其余侧表迭代器：委托原 impl 的同步路径（经 done_async 的 sync 分支）
            _ => {
                drop(iters);
                // 用原 done 逻辑中非 Object 路径：直接调用并 block 不安全；
                // 这些路径全是同步的，走 host helper。
                // 重入同步 done 判定（Map/Set/Headers/TypedArray/IndexValue）
                let iters = self
                    .caller
                    .data()
                    .iterators
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                match iters.get(handle_idx) {
                    Some(crate::IteratorState::MapKeyIter {
                        index, map_handle, ..
                    })
                    | Some(crate::IteratorState::MapValueIter {
                        index, map_handle, ..
                    })
                    | Some(crate::IteratorState::MapEntryIter {
                        index, map_handle, ..
                    }) => {
                        let table = self
                            .caller
                            .data()
                            .map_table
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        Some(if *map_handle < table.len() as u32 {
                            let entry = &table[*map_handle as usize];
                            let mut idx = *index as usize;
                            while idx < entry.keys.len() && entry.deleted[idx] {
                                idx += 1;
                            }
                            idx >= entry.keys.len()
                        } else {
                            true
                        })
                    }
                    Some(crate::IteratorState::SetValueIter {
                        index, set_handle, ..
                    })
                    | Some(crate::IteratorState::SetEntryIter {
                        index, set_handle, ..
                    }) => {
                        let table = self
                            .caller
                            .data()
                            .set_table
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        Some(if *set_handle < table.len() as u32 {
                            let entry = &table[*set_handle as usize];
                            let mut idx = *index as usize;
                            while idx < entry.values.len() && entry.deleted[idx] {
                                idx += 1;
                            }
                            idx >= entry.values.len()
                        } else {
                            true
                        })
                    }
                    Some(crate::IteratorState::IndexValueIter { index, values }) => {
                        Some(*index as usize >= values.len())
                    }
                    Some(crate::IteratorState::TypedArrayValueIter { index, length, .. })
                    | Some(crate::IteratorState::TypedArrayEntryIter { index, length, .. }) => {
                        Some(*index >= *length)
                    }
                    _ => Some(true),
                }
            }
        }
    }
    fn iterator_object_next_pair(&mut self, handle: Value) -> Option<(Value, Value)> {
        let handle_idx = value::decode_handle(handle) as usize;
        let iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match iters.get(handle_idx) {
            Some(crate::IteratorState::ObjectIter {
                iterator,
                next,
                done,
                has_current,
                ..
            }) if !*done && !*has_current => Some((*iterator, *next)),
            _ => None,
        }
    }
    fn iterator_object_return_pair(&mut self, handle: Value) -> Option<(Value, Option<Value>)> {
        let handle_idx = value::decode_handle(handle) as usize;
        let iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match iters.get(handle_idx) {
            Some(crate::IteratorState::ObjectIter {
                iterator,
                return_method,
                done,
                ..
            }) if !*done => Some((*iterator, *return_method)),
            _ => None,
        }
    }
    fn iterator_mark_done(&mut self, handle: Value) {
        let handle_idx = value::decode_handle(handle) as usize;
        if let Some(crate::IteratorState::ObjectIter { done, .. }) = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(handle_idx)
        {
            *done = true;
        }
    }
    fn iterator_lookup_afs(&mut self, handle: Value) -> Option<u32> {
        let table = self
            .caller
            .data()
            .async_from_sync_iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let decoded = value::decode_handle(handle);
        table
            .iter()
            .position(|e| e.outer_iter == handle || e.outer_handle_idx == decoded)
            .map(|i| i as u32)
    }
    fn async_from_sync_outer_iterator(&mut self, afs: u32) -> Option<Value> {
        let table = self
            .caller
            .data()
            .async_from_sync_iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(afs as usize)
            .map(|entry| value::encode_handle(value::TAG_ITERATOR, entry.outer_handle_idx))
    }
    fn async_from_sync_native_handle(&mut self, next: Value) -> Option<u32> {
        if !value::is_native_callable(next) {
            return None;
        }
        let index = value::decode_native_callable_idx(next) as usize;
        let callables = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match callables.get(index) {
            Some(crate::types::NativeCallable::AsyncFromSyncNext { handle }) => Some(*handle),
            _ => None,
        }
    }
    fn advance_async_from_sync<'c>(&'c mut self, afs: u32) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::runtime_builtins::advance_async_from_sync_async(self.caller, afs).await
        })
    }
    fn iterator_current_value(&mut self, handle: Value) -> Value {
        crate::runtime_core_ops::iterator_value_impl(self.caller, handle)
    }
    fn parse_iterator_result(&mut self, result: Value) -> Option<(Value, bool)> {
        if !(value::is_object(result) || value::is_function(result) || value::is_array(result)) {
            return None;
        }
        let ptr = crate::resolve_handle(self.caller, result)?;
        let done = crate::read_object_property_by_name(self.caller, ptr, "done")
            .map(crate::nanbox_to_bool)
            .unwrap_or(false);
        let current_value = crate::read_object_property_by_name(self.caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        Some((current_value, done))
    }
    fn create_typedarray_iterator(&mut self, this: Value, kind: u8) -> Value {
        use crate::types::IteratorState;
        let Some(entry) = crate::typedarray_entry_from_value(self.caller, this) else {
            return value::encode_undefined();
        };
        let length = entry.length;
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        match kind {
            0 => {
                iters.push(IteratorState::TypedArrayEntryIter {
                    entry,
                    index: 0,
                    length,
                });
            }
            1 => {
                let values = (0..length).map(|i| value::encode_f64(i as f64)).collect();
                iters.push(IteratorState::IndexValueIter { values, index: 0 });
            }
            _ => {
                iters.push(IteratorState::TypedArrayValueIter {
                    entry,
                    index: 0,
                    length,
                });
            }
        }
        value::encode_handle(value::TAG_ITERATOR, handle)
    }
    fn create_string_iterator(&mut self, s: wjsm_host::RuntimeString) -> Value {
        let mut iters = self
            .caller
            .data()
            .iterators
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(crate::IteratorState::StringIter {
            string: s,
            unit_pos: 0,
        });
        value::encode_handle(value::TAG_ITERATOR, handle)
    }
    fn alloc_iterator_result(&mut self, value: Value, done: bool) -> Value {
        crate::runtime_async_fn::alloc_iterator_result_from_caller(self.caller, value, done)
    }
    fn exception_reason(&mut self, exc: Value) -> Value {
        crate::exception_reason(self.caller, exc)
    }
    };
}
