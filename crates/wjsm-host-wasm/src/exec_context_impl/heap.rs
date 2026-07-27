use super::*;

impl HeapContext for WasmExecContext<'_, '_> {
    fn read_shadow_arg(&mut self, args_base: i32, index: u32) -> Value {
        let Some(env) = self.env() else {
            return value::encode_undefined();
        };
        crate::runtime_host_helpers::read_shadow_arg_with_env(self.caller, &env, args_base, index)
    }
    fn read_string_utf8(&mut self, val: Value) -> String {
        crate::runtime_render::read_runtime_string_utf8_lossy(self.caller, val)
    }
    fn write_output(&mut self, bytes: &[u8]) {
        let mut out = self
            .caller
            .data()
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        out.extend_from_slice(bytes);
    }
    fn resolve_handle(&mut self, handle: Handle) -> bool {
        self.heap_access()
            .is_some_and(|access| access.resolve_handle(handle).is_ok())
    }
    fn array_length(&mut self, handle: Handle) -> Option<u32> {
        self.heap_access()?.array_length(handle).ok()
    }
    fn array_elem(&mut self, handle: Handle, index: u32) -> Option<Value> {
        let raw = self.heap_access()?.get_element(handle, index).ok()??;
        let val = raw as i64;
        if value::is_array_hole(val) {
            None
        } else {
            Some(val)
        }
    }
    fn get_property(&mut self, handle: Handle, key: &str) -> Option<Value> {
        let name_id = self.property_key(key);
        let access = self.caller.data().heap_access_v2.clone()?;
        access
            .get_property_on_proto_chain(handle, name_id)
            .ok()
            .flatten()
            .map(|v| v as i64)
    }
    fn alloc_object(&mut self, capacity: u32) -> Value {
        let Some(env) = self.env() else {
            return value::encode_undefined();
        };
        crate::runtime_heap::alloc_host_object(self.caller, &env, capacity)
    }
    fn alloc_array(&mut self, capacity: u32) -> Value {
        crate::runtime_host_helpers::alloc_array(self.caller, capacity)
    }
    fn set_property(&mut self, handle: Handle, key: &str, value: Value) {
        let name_id = self.property_key(key);
        if let Some(access) = self.caller.data().heap_access_v2.clone() {
            let _ = access.set_property(handle, name_id, value as u64);
        }
    }
    fn delete_property(&mut self, handle: Handle, key: &str) -> bool {
        let name_id = self.property_key(key);
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
        access.delete_property(handle, name_id).unwrap_or(false)
    }
    fn gc_collect(&mut self) -> GcOutcome {
        let Some(env) = self.env() else {
            return GcOutcome::default();
        };
        let algorithm = self.caller.data().gc_algorithm;
        let stats =
            crate::runtime_gc::active_zgc::collect_dispatch(self.caller, &env, algorithm.as_str());
        self.caller
            .data()
            .store_last_gc_stats(algorithm.as_str(), stats.clone());
        GcOutcome {
            cycle_count: 1,
            bytes_collected: stats.freed_bytes,
            duration_us: u64::try_from(stats.elapsed.as_micros()).unwrap_or(u64::MAX),
        }
    }
    fn heap_used_bytes(&mut self) -> usize {
        self.heap_access()
            .map(|access| access.used_bytes() as usize)
            .unwrap_or(0)
    }
    fn async_emit_begin(&mut self) {
        self.caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .begin_emit();
    }
    fn async_hook_callbacks(&mut self, event: AsyncHookEvent, promise: bool) -> Vec<Value> {
        let hooks = self
            .caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hooks
            .active_hooks()
            .iter()
            .filter_map(|record| {
                if !record.enabled || (promise && !record.track_promises) {
                    return None;
                }
                let callback = match event {
                    AsyncHookEvent::Init => record.init,
                    AsyncHookEvent::Before => record.before,
                    AsyncHookEvent::After => record.after,
                    AsyncHookEvent::Destroy => record.destroy,
                    AsyncHookEvent::PromiseResolve => record.promise_resolve,
                };
                (callback != 0).then_some(callback)
            })
            .collect()
    }
    fn async_emit_end(&mut self) {
        self.caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .end_emit();
    }
    fn push_temp_roots(&mut self, roots: &[Value]) -> usize {
        self.caller
            .data()
            .push_host_temp_roots(roots.iter().copied())
    }
    fn truncate_temp_roots(&mut self, len: usize) {
        self.caller.data().truncate_host_temp_roots(len);
    }
}
