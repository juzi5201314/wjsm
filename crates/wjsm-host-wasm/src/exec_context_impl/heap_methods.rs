// ExecContext 方法片段：heap
macro_rules! exec_ctx_heap {
    () => {
    fn alloc_null_proto_object(&mut self, capacity: u32) -> Value {
        let Some(env) = self.env() else {
            return value::encode_undefined();
        };
        crate::runtime_heap::alloc_host_null_proto_object(self.caller, &env, capacity)
    }
    fn gc_safepoint_poll(&mut self) {
        let Some(env) = self.env() else {
            return;
        };
        if let Some(global) = env.gc_alloc_bytes {
            let _ = global.set(&mut *self.caller, wasmtime::Val::I32(0));
        }
        let algorithm = self.caller.data().gc_algorithm.as_str();
        let mut stats =
            crate::runtime_gc::active_zgc::collect_dispatch(self.caller, &env, algorithm);
        let next_trigger = {
            let heap_limit = self.caller.data().heap_access_v2().heap_limit_bytes();
            let mut scheduler = self
                .caller
                .data()
                .gc_scheduler
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            scheduler.after_cycle(
                stats.heap_used_bytes,
                0,
                heap_limit.min(usize::MAX as u64) as usize,
            );
            scheduler.trigger_bytes.min(i32::MAX as usize).max(1) as i32
        };
        if let Some(global) = env.gc_trigger_bytes {
            let _ = global.set(&mut *self.caller, wasmtime::Val::I32(next_trigger));
        }
        if algorithm != "zgc" {
            stats.pause_ns_max = 0;
            stats.pause_ns_total = 0;
            stats.pause_count = 0;
        }
        self.caller.data().store_last_gc_stats(algorithm, stats);
    }
    fn gc_barrier_flush(&mut self) {
        let Some(env) = self.env() else {
            return;
        };
        let (_, _, barrier_event_buf_base) = self.caller.data().heap_layout_boundaries();
        if barrier_event_buf_base != 0
            && let Some(global) = env.barrier_buf_ptr
        {
            let _ = global.set(
                &mut *self.caller,
                wasmtime::Val::I32(barrier_event_buf_base as i32),
            );
        }
    }
    fn prototype_of(&mut self, handle: Handle) -> Option<Handle> {
        self.heap_access()?.prototype(handle).ok()
    }
    fn resolve_handle_idx(&mut self, val: Value) -> Option<usize> {
        let env = self.env()?;
        let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
        crate::runtime_values::resolve_handle_idx_with_env(self.caller, &env, handle_idx)
    }
    fn handle_index_of(&mut self, val: Value) -> Option<Handle> {
        Some(crate::runtime_values::handle_index_of(self.caller, val) as u32)
    }
    fn handle_is_live(&mut self, handle: Handle) -> bool {
        crate::obj_table_handle_live(self.caller, handle)
    }
    fn encode_handle_as_value(&mut self, handle: Handle) -> Value {
        crate::encode_handle_as_js_value(self.caller, handle)
            .unwrap_or_else(value::encode_undefined)
    }
    fn resolve_object_ptr(&mut self, val: Value) -> Option<usize> {
        crate::runtime_values::resolve_handle(self.caller, val)
    }
    fn arraybuffer_create(&mut self, byte_length: u32) -> Option<u32> {
        let mut table = self
            .caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::ArrayBufferEntry {
            data: vec![0u8; byte_length as usize],
        });
        Some(handle)
    }
    fn arraybuffer_byte_length(&mut self, handle: u32) -> Option<u32> {
        let table = self
            .caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(handle as usize).map(|e| e.data.len() as u32)
    }
    fn arraybuffer_slice(&mut self, handle: u32, start: u32, end: u32) -> Option<u32> {
        let slice = {
            let table = self
                .caller
                .data()
                .arraybuffer_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = table.get(handle as usize)?;
            let s = (start as usize).min(entry.data.len());
            let e = (end as usize).min(entry.data.len()).max(s);
            entry.data[s..e].to_vec()
        };
        let mut table = self
            .caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let new_handle = table.len() as u32;
        table.push(crate::types::ArrayBufferEntry { data: slice });
        Some(new_handle)
    }
    fn arraybuffer_read_bytes(
        &mut self,
        handle: u32,
        offset: usize,
        len: usize,
    ) -> Option<Vec<u8>> {
        let table = self
            .caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(handle as usize)?;
        if offset + len > entry.data.len() {
            return None;
        }
        Some(entry.data[offset..offset + len].to_vec())
    }
    fn arraybuffer_write_bytes(&mut self, handle: u32, offset: usize, bytes: &[u8]) -> bool {
        let mut table = self
            .caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle as usize) else {
            return false;
        };
        if offset + bytes.len() > entry.data.len() {
            return false;
        }
        entry.data[offset..offset + bytes.len()].copy_from_slice(bytes);
        true
    }
    fn resolve_buffer_backing(&mut self, buffer: Value) -> Option<(u32, u32, bool)> {
        match crate::shared_buffer::resolve_buffer_backing(self.caller, buffer) {
            Some(crate::shared_buffer::BufferBacking::SharedArrayBuffer {
                handle,
                byte_length,
                ..
            }) => Some((handle, byte_length, true)),
            Some(crate::shared_buffer::BufferBacking::ArrayBuffer {
                handle,
                byte_length,
            }) => Some((handle, byte_length, false)),
            None => None,
        }
    }
    fn buffer_read_bytes(
        &mut self,
        handle: u32,
        is_shared: bool,
        offset: usize,
        len: usize,
    ) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        if crate::shared_buffer::dataview_read_bytes(
            self.caller,
            handle,
            is_shared,
            offset,
            &mut buf,
        ) {
            Some(buf)
        } else {
            None
        }
    }
    fn buffer_write_bytes(
        &mut self,
        handle: u32,
        is_shared: bool,
        offset: usize,
        bytes: &[u8],
    ) -> bool {
        crate::shared_buffer::dataview_set_bytes(self.caller, handle, is_shared, offset, bytes)
    }
    fn push_host_temp_roots(&mut self, vals: &[Value]) -> usize {
        self.caller
            .data()
            .push_host_temp_roots(vals.iter().copied())
    }
    fn truncate_host_temp_roots(&mut self, len: usize) {
        self.caller.data().truncate_host_temp_roots(len);
    }
    };
}
