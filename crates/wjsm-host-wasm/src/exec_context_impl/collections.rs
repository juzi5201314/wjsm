// ExecContext 方法片段：collections
macro_rules! exec_ctx_collections {
    () => {
    fn map_table_create(&mut self) -> u32 {
        self.caller.data().alloc_map_entry()
    }
    fn set_table_create(&mut self) -> u32 {
        self.caller.data().alloc_set_entry()
    }
    fn map_set(&mut self, handle: u32, key: Value, val: Value) {
        crate::runtime_collections::map_set_impl(self.caller, handle, key, val);
    }
    fn map_get(&mut self, handle: u32, key: Value) -> Option<Value> {
        crate::runtime_collections::map_get_impl(self.caller, handle, key)
    }
    fn map_set_has(&mut self, handle: u32, key: Value, is_set: bool) -> bool {
        crate::runtime_collections::map_set_has_impl(self.caller, handle, key, is_set)
    }
    fn map_set_delete(&mut self, handle: u32, key: Value, is_set: bool) -> bool {
        crate::runtime_collections::map_set_delete_impl(self.caller, handle, key, is_set)
    }
    fn map_set_clear(&mut self, handle: u32, is_set: bool) {
        crate::runtime_collections::map_set_clear_impl(self.caller, handle, is_set);
    }
    fn map_set_size(&mut self, handle: u32, is_set: bool) -> u32 {
        crate::runtime_collections::map_set_size_impl(self.caller, handle, is_set)
    }
    fn set_add(&mut self, handle: u32, key: Value) {
        crate::runtime_collections::set_add_impl(self.caller, handle, key);
    }
    fn map_set_entries_snapshot(&mut self, handle: u32, is_set: bool) -> Vec<(Value, Value)> {
        crate::runtime_collections::map_set_entries_snapshot_impl(self.caller, handle, is_set)
    }
    fn map_set_first_key(&mut self, handle: u32, is_set: bool) -> Option<Value> {
        crate::runtime_collections::map_set_first_key_impl(self.caller, handle, is_set)
    }
    fn create_map_set_iterator(&mut self, handle: u32, is_set: bool, kind: u8) -> Value {
        use crate::types::MapSetMethodKind;
        let method_kind = match kind {
            0 => MapSetMethodKind::Keys,
            1 => MapSetMethodKind::Values,
            _ => MapSetMethodKind::Entries,
        };
        // 构造临时 receiver 供 map_set_create_iterator 解析 handle
        let receiver = self.alloc_object(1);
        if is_set {
            self.define_data_property(receiver, "__set_handle__", value::encode_f64(handle as f64));
        } else {
            self.define_data_property(receiver, "__map_handle__", value::encode_f64(handle as f64));
        }
        crate::runtime_collections::map_set_create_iterator(self.caller, receiver, method_kind)
    }
    fn weakmap_table_create(&mut self) -> u32 {
        let mut table = self
            .caller
            .data()
            .weakmap_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::WeakMapEntry {
            map: std::collections::HashMap::new(),
        });
        handle
    }
    fn weakmap_set(&mut self, handle: u32, key_handle: Handle, val: Value) {
        let mut table = self
            .caller
            .data()
            .weakmap_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.map.insert(key_handle, val);
        }
    }
    fn weakmap_get(&mut self, handle: u32, key_handle: Handle) -> Option<Value> {
        let table = self
            .caller
            .data()
            .weakmap_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .and_then(|e| e.map.get(&key_handle).copied())
    }
    fn weakmap_has(&mut self, handle: u32, key_handle: Handle) -> bool {
        let table = self
            .caller
            .data()
            .weakmap_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .is_some_and(|e| e.map.contains_key(&key_handle))
    }
    fn weakmap_delete(&mut self, handle: u32, key_handle: Handle) -> bool {
        let mut table = self
            .caller
            .data()
            .weakmap_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get_mut(handle as usize)
            .is_some_and(|e| e.map.remove(&key_handle).is_some())
    }
    fn weakset_table_create(&mut self) -> u32 {
        let mut table = self
            .caller
            .data()
            .weakset_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::WeakSetEntry {
            set: std::collections::HashSet::new(),
        });
        handle
    }
    fn weakset_add(&mut self, handle: u32, key_handle: Handle) {
        let mut table = self
            .caller
            .data()
            .weakset_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.set.insert(key_handle);
        }
    }
    fn weakset_has(&mut self, handle: u32, key_handle: Handle) -> bool {
        let table = self
            .caller
            .data()
            .weakset_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .is_some_and(|e| e.set.contains(&key_handle))
    }
    fn weakset_delete(&mut self, handle: u32, key_handle: Handle) -> bool {
        let mut table = self
            .caller
            .data()
            .weakset_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get_mut(handle as usize)
            .is_some_and(|e| e.set.remove(&key_handle))
    }
    fn weakref_table_push(&mut self, target_handle: Handle) -> u32 {
        let mut table = self
            .caller
            .data()
            .weakref_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = table.len() as u32;
        table.push(crate::WeakRefEntry {
            target_handle: Some(target_handle),
        });
        idx
    }
    fn weakref_table_get_target(&mut self, index: u32) -> Option<Handle> {
        let table = self
            .caller
            .data()
            .weakref_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(index as usize).and_then(|e| e.target_handle)
    }
    fn weak_target_handle(&mut self, target: Value) -> Option<Handle> {
        crate::weak_target_handle_index_of(self.caller, target)
    }
    fn finalization_registry_table_push(&mut self, object_handle: Handle, callback: Value) -> u32 {
        let mut table = self
            .caller
            .data()
            .finalization_registry_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = table.len() as u32;
        table.push(crate::FinalizationRegistryEntry {
            object_handle,
            callback,
            registrations: Vec::new(),
        });
        idx
    }
    fn finalization_registry_add(
        &mut self,
        registry_idx: u32,
        target_handle: Handle,
        held_value: Value,
        unregister_token: Option<Value>,
    ) {
        let mut table = self
            .caller
            .data()
            .finalization_registry_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(registry_idx as usize) {
            entry.registrations.push(crate::FinalizationRegistration {
                target_handle,
                held_value,
                unregister_token,
            });
        }
    }
    fn finalization_registry_unregister_token(&mut self, registry_idx: u32, token: Value) -> bool {
        let mut table = self
            .caller
            .data()
            .finalization_registry_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(registry_idx as usize) else {
            return false;
        };
        let initial = entry.registrations.len();
        entry.registrations.retain(|r| match &r.unregister_token {
            Some(t) => !crate::same_value_zero(self.caller, *t, token),
            None => true,
        });
        entry.registrations.len() < initial
    }
    fn create_weakref_method(&mut self, kind: &str) -> Value {
        let nc = match kind {
            "weakref_deref" => crate::NativeCallable::WeakRefDerefMethod,
            "fr_register" => crate::NativeCallable::FinalizationRegistryRegisterMethod,
            "fr_unregister" => crate::NativeCallable::FinalizationRegistryUnregisterMethod,
            _ => return value::encode_undefined(),
        };
        crate::create_native_callable(self.caller.data(), nc)
    }
    fn release_unowned_map_entry(&mut self, handle: u32) {
        self.caller.data().release_unowned_map_entry(handle);
    }
    fn release_unowned_set_entry(&mut self, handle: u32) {
        self.caller.data().release_unowned_set_entry(handle);
    }
    fn bind_map_owner(&mut self, handle: u32, owner: Handle) {
        let mut table = self
            .caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.owner = Some(owner);
        }
    }
    fn bind_set_owner(&mut self, handle: u32, owner: Handle) {
        let mut table = self
            .caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.owner = Some(owner);
        }
    }
    };
}
