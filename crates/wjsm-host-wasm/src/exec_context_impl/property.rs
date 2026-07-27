// ExecContext 方法片段：property
macro_rules! exec_ctx_property {
    () => {
    fn get_property_by_name_id(&mut self, obj: Value, name_id: u32) -> Value {
        wjsm_builtins::get_method::get_by_name_id(self, obj, name_id)
    }
    fn get_method_by_name_id(&mut self, obj: Value, name_id: u32) -> anyhow::Result<Option<Value>> {
        match wjsm_builtins::get_method::get_method_by_name_id(self, obj, name_id) {
            Ok(v) => Ok(v),
            Err(exc) => Err(anyhow::anyhow!(
                "TypeError: method is not callable (exception={exc})"
            )),
        }
    }
    fn set_property_by_name_id(&mut self, handle: Handle, name_id: u32, val: Value) -> bool {
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
        if access.object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
            let array = value::encode_handle(value::TAG_ARRAY, handle);
            let flags = crate::array_named_props::ArrayNamedPropsStore::get_slot(
                self.caller,
                array,
                name_id,
            )
            .map_or(
                constants::FLAG_CONFIGURABLE
                    | constants::FLAG_ENUMERABLE
                    | constants::FLAG_WRITABLE,
                |slot| slot.flags,
            );
            crate::array_named_props::ArrayNamedPropsStore::set_with_flags(
                self.caller,
                array,
                name_id,
                val,
                flags,
            );
            return true;
        }
        // name_id 可能来自编译期 MemoryString，需 canonicalize 到 V2 property key
        // 才能与 V2 堆中的属性槽 name 匹配。
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return false;
        };
        match access.set_property(handle, key, val as u64) {
            Ok(()) => true,
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 private property write: {error}"),
                );
                false
            }
        }
    }
    fn delete_property_by_name_id(&mut self, handle: Handle, name_id: u32) -> bool {
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
        if access.object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
            let array = value::encode_handle(value::TAG_ARRAY, handle);
            return crate::array_named_props::ArrayNamedPropsStore::remove(
                self.caller,
                array,
                name_id,
            )
            .unwrap_or(true);
        }
        // name_id 可能来自编译期 MemoryString，需 canonicalize 到 V2 property key。
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return false;
        };
        match access.delete_property(handle, key) {
            Ok(deleted) => deleted,
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 delete property: {error}"),
                );
                false
            }
        }
    }
    fn define_data_property(&mut self, obj: Value, key: &str, value: Value) {
        let _ =
            crate::runtime_host_helpers::define_host_data_property(self.caller, obj, key, value);
    }
    fn define_data_property_by_name_id(
        &mut self,
        obj: Value,
        name_id: u32,
        value: Value,
        flags: i32,
    ) {
        let _ = crate::runtime_host_helpers::define_host_data_property_by_name_id_with_flags(
            self.caller,
            obj,
            name_id,
            value,
            flags,
        );
    }
    fn define_data_property_with_flags(
        &mut self,
        handle: Handle,
        name_id: u32,
        val: Value,
        flags: u32,
    ) -> bool {
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
        if access.object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
            crate::array_named_props::ArrayNamedPropsStore::set_with_flags(
                self.caller,
                value::encode_handle(value::TAG_ARRAY, handle),
                name_id,
                val,
                flags as i32,
            );
            return true;
        }
        // name_id 可能来自编译期 MemoryString，需 canonicalize 到 V2 property key。
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return false;
        };
        match access.define_data_property(handle, key, val as u64, flags) {
            Ok(()) => true,
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 private property define: {error}"),
                );
                false
            }
        }
    }
    fn define_accessor_property_with_flags(
        &mut self,
        handle: Handle,
        name_id: u32,
        getter: Value,
        setter: Value,
        flags: u32,
    ) -> bool {
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
        if access.object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
            crate::array_named_props::ArrayNamedPropsStore::set_descriptor(
                self.caller,
                value::encode_handle(value::TAG_ARRAY, handle),
                name_id,
                value::encode_undefined(),
                getter,
                setter,
                flags as i32 | constants::FLAG_IS_ACCESSOR,
            );
            return true;
        }
        // name_id 可能来自编译期 MemoryString，需 canonicalize 到 V2 property key。
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return false;
        };
        match access.define_accessor_property_with_flags(
            handle,
            key,
            getter as u64,
            setter as u64,
            flags,
        ) {
            Ok(()) => true,
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 private accessor define: {error}"),
                );
                false
            }
        }
    }
    fn set_object_proto(&mut self, obj: Value, proto: Value) {
        let Some(env) = self.env() else {
            return;
        };
        crate::runtime_heap::set_object_proto_header(self.caller, &env, obj, proto);
    }
    fn read_property_by_name_id_proto_walk(
        &mut self,
        obj_ptr: usize,
        name_id: u32,
    ) -> Option<Value> {
        use std::collections::HashSet;
        let mut visited = HashSet::new();
        let mut current = obj_ptr;
        loop {
            if !visited.insert(current) {
                return None;
            }
            if let Some(val) = crate::runtime_values::read_object_property_by_name_id(
                self.caller,
                current,
                name_id,
            ) {
                return Some(val);
            }
            let env = self.env()?;
            let proto_handle = {
                let data = env.memory.data(&*self.caller);
                if current + 4 > data.len() {
                    return None;
                }
                u32::from_le_bytes([
                    data[current],
                    data[current + 1],
                    data[current + 2],
                    data[current + 3],
                ])
            };
            if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
                return None;
            }
            current = crate::runtime_values::resolve_handle_idx_with_env(
                self.caller,
                &env,
                proto_handle as usize,
            )?;
        }
    }
    fn read_property_by_string_key(&mut self, obj: Value, key: &str) -> Value {
        let key = crate::store_runtime_string(&*self.caller, key.to_string());
        crate::host_imports::read_property_by_string_key_raw(self.caller, obj, key)
    }
    fn get_by_name_id_on_proto_chain(
        &mut self,
        receiver: Value,
        obj_ptr: usize,
        name_id: u32,
    ) -> Option<Value> {
        use crate::constants;
        use std::collections::HashSet;
        use wasmtime::Extern;

        let mut visited = HashSet::new();
        let mut current = obj_ptr;
        loop {
            if !visited.insert(current) {
                return None;
            }
            if let Some((slot_offset, flags, val)) =
                crate::runtime_values::find_property_slot_by_name_id(self.caller, current, name_id)
            {
                if (flags & constants::FLAG_IS_ACCESSOR) == 0 {
                    return Some(val);
                }
                let getter = {
                    let Some(Extern::Memory(memory)) = self.caller.get_export("memory") else {
                        return Some(value::encode_undefined());
                    };
                    let data = memory.data(&*self.caller);
                    if slot_offset + 24 > data.len() {
                        return Some(value::encode_undefined());
                    }
                    i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap())
                };
                return Some(self.invoke_getter_sync(getter, receiver));
            }
            let env = self.env()?;
            let proto_handle = {
                let data = env.memory.data(&*self.caller);
                if current + 4 > data.len() {
                    return None;
                }
                u32::from_le_bytes([
                    data[current],
                    data[current + 1],
                    data[current + 2],
                    data[current + 3],
                ])
            };
            if proto_handle & 0x8000_0000 != 0 {
                let proxy_idx = (proto_handle & 0x7FFF_FFFF) as usize;
                let proxy_val = value::encode_proxy_handle(proxy_idx as u32);
                let prop = crate::property_key::name_id_to_property_key_value(name_id)?;
                return Some(self.reflect_get_sync(proxy_val, prop, receiver));
            }
            current = crate::runtime_values::resolve_handle_idx_with_env(
                self.caller,
                &env,
                proto_handle as usize,
            )?;
        }
    }
    fn get_property_slot_on_proto(
        &mut self,
        handle: Handle,
        name_id: u32,
    ) -> Option<(Value, bool, Value)> {
        let key = crate::property_key::canonicalize_v2_name_id(self.caller, name_id)?;
        let property = self
            .caller
            .data()
            .heap_access_v2()
            .get_property_slot_on_proto_chain(handle, key)
            .ok()
            .flatten()?;
        let is_accessor = property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0;
        Some((property.value as i64, is_accessor, property.getter as i64))
    }
    fn lookup_property_on_proto(
        &mut self,
        handle: Handle,
        name_id: u32,
    ) -> wjsm_host::PropertyLookup {
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return wjsm_host::PropertyLookup::Missing;
        };
        match self
            .caller
            .data()
            .heap_access_v2()
            .get_property_slot_on_proto_chain(handle, key)
        {
            Ok(Some(property)) => wjsm_host::PropertyLookup::Slot {
                value: property.value as i64,
                is_accessor: property.flags
                    & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32
                    != 0,
                getter: property.getter as i64,
            },
            Ok(None) => wjsm_host::PropertyLookup::Missing,
            Err(crate::runtime_gc::HeapAccessV2Error::ProxyPrototype { handle }) => {
                wjsm_host::PropertyLookup::Proxy(value::encode_proxy_handle(
                    handle & 0x7FFF_FFFF,
                ))
            }
            Err(_) => wjsm_host::PropertyLookup::Missing,
        }
    }
    fn array_named_prop_get(&mut self, arr: Value, name_id: u32) -> Option<Value> {
        let key = crate::property_key::canonicalize_v2_name_id(self.caller, name_id)?;
        let slot = crate::array_named_props::ArrayNamedPropsStore::get_slot(self.caller, arr, key)?;
        Some(if slot.flags & constants::FLAG_IS_ACCESSOR != 0 {
            self.invoke_getter_sync(slot.getter, arr)
        } else {
            slot.value
        })
    }
    fn get_own_property_slot(
        &mut self,
        handle: Handle,
        name_id: u32,
    ) -> Option<(Value, u32, Value, Value)> {
        let access = self.caller.data().heap_access_v2().clone();
        // name_id 可能来自编译期 MemoryString，需 canonicalize 到 V2 property key
        // 才能与 V2 堆中的属性槽 name 匹配。
        let key = crate::property_key::canonicalize_v2_name_id(self.caller, name_id)?;
        if access.object_type(handle).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
            let array = value::encode_handle(value::TAG_ARRAY, handle);
            if let Some(slot) =
                crate::array_named_props::ArrayNamedPropsStore::get_slot(self.caller, array, key)
            {
                return Some((
                    slot.value,
                    slot.flags as u32,
                    slot.getter,
                    slot.setter,
                ));
            }
        }
        let slot = access.get_property_slot(handle, key).ok().flatten()?;
        Some((
            slot.value as i64,
            slot.flags,
            slot.getter as i64,
            slot.setter as i64,
        ))
    }
    fn ensure_property_storage(&mut self, value: Value) -> bool {
        let Some(handle) = self.handle_index_of(value) else {
            return false;
        };
        let access = self.caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            return true;
        }
        if !value::is_function(value) && !value::is_closure(value) && !value::is_bound(value) {
            return false;
        }
        let prototype = value::decode_object_handle(self.caller.data().function_prototype);
        let capacity = 4u32;
        let bytes = u64::from(capacity)
            * u64::from(wjsm_ir::constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE)
            + u64::from(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE);
        let Ok((address, _)) = crate::runtime_heap::allocate_v2_object_bytes_with_context(
            self.caller,
            bytes,
        ) else {
            return false;
        };
        access
            .publish_object(handle, address, prototype, capacity)
            .is_ok()
    }
    fn read_property_for_render(&mut self, obj: Value, key: &str) -> Option<Value> {
        // 与原 render_value 内联路径一致：数组走 decode_array_handle，
        // 其余对象走通用 handle 解析；own + 原型链数据槽，不触发 getter。
        let ptr_opt = if value::is_array(obj) {
            crate::runtime_values::resolve_handle_idx(
                self.caller,
                value::decode_array_handle(obj) as usize,
            )
        } else {
            crate::runtime_values::resolve_handle(self.caller, obj)
        };
        let ptr = ptr_opt?;
        crate::runtime_values::read_object_property_by_name(self.caller, ptr, key)
    }
    fn read_data_property(&mut self, obj: Value, key: &str) -> Value {
        crate::read_host_data_property_v2(self.caller, obj, key)
            .unwrap_or_else(value::encode_undefined)
    }
    fn own_enumerable_data_slots(&mut self, obj: Value) -> Option<Vec<(u32, Value)>> {
        let handle = value::decode_object_handle(obj) as u32;
        let access = self.caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            return Some(
                access
                    .own_property_slots(handle)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, flags)| {
                        flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0
                            && flags & wjsm_ir::constants::FLAG_PRIVATE as u32 == 0
                    })
                    .filter_map(|(key, _)| {
                        access
                            .get_property(handle, key)
                            .ok()
                            .flatten()
                            .map(|prop_value| (key, prop_value as i64))
                    })
                    .filter(|(_, prop_value)| !value::is_undefined(*prop_value))
                    .collect(),
            );
        }
        // legacy 线性内存路径
        let ptr = crate::runtime_values::resolve_handle(self.caller, obj)?;
        let env = self.env()?;
        let data = env.memory.data(&*self.caller);
        if ptr + 16 > data.len() {
            return None;
        }
        let num_props = u32::from_le_bytes([
            data[ptr + 12],
            data[ptr + 13],
            data[ptr + 14],
            data[ptr + 15],
        ]) as usize;
        let mut slots = Vec::with_capacity(num_props);
        for i in 0..num_props {
            let slot_off = ptr + 16 + i * 32;
            if slot_off + 32 > data.len() {
                continue;
            }
            let flags = i32::from_le_bytes([
                data[slot_off + 4],
                data[slot_off + 5],
                data[slot_off + 6],
                data[slot_off + 7],
            ]);
            if (flags & wjsm_ir::constants::FLAG_ENUMERABLE) == 0
                || (flags & wjsm_ir::constants::FLAG_PRIVATE) != 0
            {
                continue;
            }
            let name_id = u32::from_le_bytes([
                data[slot_off],
                data[slot_off + 1],
                data[slot_off + 2],
                data[slot_off + 3],
            ]);
            let prop_val =
                i64::from_le_bytes(data[slot_off + 8..slot_off + 16].try_into().unwrap());
            if value::is_undefined(prop_val) {
                continue;
            }
            slots.push((name_id, prop_val));
        }
        Some(slots)
    }
    fn own_property_entries(&mut self, handle: Handle) -> Vec<(u32, u32)> {
        let access = self.caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            // V2 堆槽位中的 name 是已 canonicalize 的 property key；builtins
            // 端需要编译期 name_id 时自行再 canonicalize，此处透传 raw 值。
            return access.own_property_slots(handle).unwrap_or_default();
        }
        Vec::new()
    }
    fn collect_own_property_names(&mut self, obj: Value, enumerable_only: bool) -> Vec<String> {
        crate::collect_own_property_names_from_value(self.caller, obj, enumerable_only)
    }
    fn collect_own_property_symbols(&mut self, obj: Value) -> Vec<Value> {
        if !value::is_js_object(obj) {
            return Vec::new();
        }
        let Some(ptr) = crate::resolve_handle(self.caller, obj) else {
            return Vec::new();
        };
        crate::collect_own_property_key_values(self.caller, ptr, true)
    }
    fn has_own_property_by_name_id(&mut self, handle: Handle, name_id: u32) -> bool {
        self.caller
            .data()
            .heap_access_v2()
            .get_property(handle, name_id)
            .ok()
            .flatten()
            .is_some()
    }
    fn get_own_property_descriptor_value(&mut self, target: Value, prop: Value) -> Value {
        wjsm_builtins::proxy_reflect::reflect_get_own_property_descriptor_impl(self, target, prop)
    }
    fn define_property_or_throw(&mut self, target: Value, key: Value, desc: Value) -> bool {
        let descriptor = match crate::parse_descriptor(self.caller, desc) {
            Ok(descriptor) => descriptor,
            Err(message) => {
                crate::set_runtime_error(self.caller.data(), message);
                return false;
            }
        };
        let Some(name_id) =
            crate::property_key::property_key_value_to_name_id(self.caller, key, true)
        else {
            crate::set_runtime_error(
                self.caller.data(),
                "TypeError: Invalid property key".to_string(),
            );
            return false;
        };
        match crate::runtime_host_helpers::define_property_on_normal_object(
            self.caller,
            target,
            name_id,
            &descriptor,
        ) {
            Ok(_) => true,
            Err(message) => {
                crate::set_runtime_error(self.caller.data(), message);
                false
            }
        }
    }
    fn update_property_flags(&mut self, handle: Handle, name_id: u32, flags: u32) -> bool {
        self.caller
            .data()
            .heap_access_v2()
            .update_property_flags(handle, name_id, flags)
            .is_ok()
    }
    fn callable_get_property(&mut self, value: Value, name_id: u32) -> Value {
        crate::runtime_linker::function_value_get_property_impl(
            self.caller,
            value,
            name_id as i32,
        )
    }
    fn native_callable_get_property(&mut self, value: Value, name_id: u32) -> Value {
        crate::runtime_linker::native_callable_get_property_impl(
            self.caller,
            value,
            name_id as i32,
        )
    }
    fn primitive_symbol_get_property(&mut self, boxed: Value, name_id: u32) -> Value {
        crate::runtime_heap::primitive_symbol_get_property_impl(self.caller, boxed, name_id)
    }
    fn primitive_regexp_get_property(&mut self, boxed: Value, name_id: u32) -> Value {
        crate::runtime_regexp::primitive_regexp_get_property_impl(self.caller, boxed, name_id)
    }
    fn primitive_regexp_set_property(&mut self, boxed: Value, name_id: u32, val: Value) {
        crate::runtime_regexp::primitive_regexp_set_property_impl(self.caller, boxed, name_id, val);
    }
    fn is_array_prototype_join(&mut self, candidate: Value) -> bool {
        let Some(env) = self.env() else {
            return false;
        };
        let handle = env.array_proto_handle.get(&mut *self.caller).i32().unwrap_or(-1);
        if handle < 0 {
            return false;
        }
        let prototype = value::encode_object_handle(handle as u32);
        let join = self.read_property_by_string_key(prototype, "join");
        let same = wjsm_builtins::core::strict_eq_impl(self, candidate, join);
        !value::is_falsy(same)
    }
    fn invoke_getter_sync(&mut self, getter: Value, receiver: Value) -> Value {
        // 算法在 wjsm-builtins；此处仅桥接 ExecContext trait 方法。
        wjsm_builtins::get_method::invoke_getter(self, getter, receiver)
    }
    fn reflect_get_sync(&mut self, target: Value, prop: Value, receiver: Value) -> Value {
        let rt = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            rt.block_on(
                crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                    self.caller,
                    target,
                    prop,
                    receiver,
                ),
            )
        })
    }
    fn reflect_own_keys(&mut self, target: Value) -> Value {
        // 复用 host-wasm 现有 collect_own_property_key_values，保留
        // 整数索引排序 + MemoryString→RuntimeString 转换语义。
        let Some(ptr) = crate::resolve_handle(self.caller, target) else {
            return value::encode_undefined();
        };
        let keys = crate::collect_own_property_key_values(self.caller, ptr, false);
        let len = keys.len() as u32;
        let arr = crate::runtime_host_helpers::alloc_array(self.caller, len);
        for (i, key) in keys.into_iter().enumerate() {
            crate::runtime_host_helpers::set_array_elem(self.caller, arr, i as i32, key);
        }
        if let Some(arr_ptr) = crate::resolve_array_ptr(self.caller, arr) {
            crate::write_array_length(self.caller, arr_ptr, len);
        }
        arr
    }
    fn object_proto_handle(&mut self, obj: Value) -> Option<u32> {
        let handle = crate::handle_index_of(self.caller, obj) as u32;
        let access = self.caller.data().heap_access_v2();
        if access.resolve_handle(handle).is_ok() {
            return access.prototype(handle).ok();
        }
        let ptr = crate::resolve_handle(self.caller, obj)?;
        let Some(wasmtime::Extern::Memory(memory)) = self.caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*self.caller);
        if ptr + 4 > data.len() {
            return None;
        }
        Some(u32::from_le_bytes([
            data[ptr],
            data[ptr + 1],
            data[ptr + 2],
            data[ptr + 3],
        ]))
    }
    fn object_get_prototype_of_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::proxy_or_target_get_prototype_of_impl_async(self.caller, obj).await
        })
    }
    fn object_is_extensible_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, bool> {
        Box::pin(
            async move { crate::proxy_or_target_is_extensible_impl_async(self.caller, obj).await },
        )
    }
    fn object_prevent_extensions_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, bool> {
        Box::pin(async move {
            crate::proxy_or_target_prevent_extensions_impl_async(self.caller, obj).await
        })
    }
    fn object_keys_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_enumerable_own_keys_async(self, obj).await
        })
    }
    fn object_entries_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_entries_async(self, obj).await
        })
    }
    fn object_values_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(
            async move { wjsm_builtins::proxy_reflect_async::object_values_async(self, obj).await },
        )
    }
    fn object_get_own_property_names_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_get_own_property_names_async(self, obj).await
        })
    }
    fn object_get_own_property_symbols_async<'c>(
        &'c mut self,
        obj: Value,
    ) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_get_own_property_symbols_async(self, obj)
                .await
        })
    }
    fn object_assign_async<'c>(
        &'c mut self,
        target: Value,
        args_base: i32,
        args_count: i32,
    ) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            let args: Vec<Value> = (0..args_count.max(0))
                .map(|i| self.read_shadow_arg(args_base, i as u32))
                .collect();
            wjsm_builtins::proxy_reflect_async::object_assign_impl_async(self, target, &args).await
        })
    }
    fn value_to_proto_handle(&mut self, proto: Value) -> u32 {
        crate::host_imports::proto_handle_from_value(self.caller, proto)
    }
    fn set_prototype_handle(&mut self, obj: Value, proto_handle: u32) -> bool {
        let handle = crate::handle_index_of(self.caller, obj) as u32;
        let access = self.caller.data().heap_access_v2();
        if access.resolve_handle(handle).is_ok() {
            return access.set_prototype(handle, proto_handle).is_ok();
        }
        let Some(env) = self.env() else {
            return false;
        };
        crate::runtime_gc::heap_access::write_proto(self.caller, &env, handle, proto_handle)
            .is_some()
    }
    fn is_extensible(&mut self, obj: Value) -> bool {
        crate::is_extensible_impl(self.caller, obj)
    }
    fn prevent_extensions(&mut self, obj: Value) -> bool {
        crate::prevent_extensions_impl(self.caller, obj)
    }
    fn to_object(&mut self, val: Value) -> Value {
        crate::to_object(self.caller, val)
    }
    fn value_to_key_string(&mut self, val: Value) -> Result<String, Value> {
        crate::runtime_json::json_parse_to_string(self.caller, val)
    }
    };
}
