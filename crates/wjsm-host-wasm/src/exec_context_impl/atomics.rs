// ExecContext 方法片段：atomics
macro_rules! exec_ctx_atomics {
    () => {
    fn buffer_atomic_load(&mut self, view: &TypedArrayView, byte_offset: u64) -> Option<i64> {
        crate::runtime_atomics::atomic_load(self.caller, view, byte_offset)
    }
    fn buffer_atomic_store(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        value: i64,
    ) -> Option<()> {
        crate::runtime_atomics::atomic_store(self.caller, view, byte_offset, value)
    }
    fn buffer_atomic_rmw(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        op: AtomicsRmwOp,
        operand: i64,
    ) -> Option<i64> {
        crate::runtime_atomics::atomic_rmw(self.caller, view, byte_offset, op, operand)
    }
    fn buffer_atomic_compare_exchange(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        expected: i64,
        replacement: i64,
    ) -> Option<i64> {
        crate::runtime_atomics::atomic_compare_exchange(
            self.caller,
            view,
            byte_offset,
            expected,
            replacement,
        )
    }
    fn atomics_notify(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        count: Option<u32>,
    ) -> u32 {
        crate::runtime_atomics::notify(self.caller, view, byte_offset, count)
    }
    fn atomics_wait_async_op<'c>(
        &'c mut self,
        view: TypedArrayView,
        byte_offset: u64,
        expected: i64,
        timeout_ms: f64,
    ) -> ExecFuture<'c> {
        Box::pin(async move {
            Ok(crate::runtime_atomics::wait_async_op(
                self.caller,
                view,
                byte_offset,
                expected,
                timeout_ms,
            ))
        })
    }
    fn atomics_wait_sync<'c>(
        &'c mut self,
        view: TypedArrayView,
        byte_offset: u64,
        expected: i64,
        timeout_ms: f64,
    ) -> ExecFuture<'c> {
        Box::pin(async move {
            Ok(crate::runtime_atomics::wait_sync(
                self.caller,
                view,
                byte_offset,
                expected,
                timeout_ms,
            )
            .await)
        })
    }
    fn shared_arraybuffer_create(&mut self, byte_length: u32) -> Option<u32> {
        use std::sync::{Arc, RwLock};
        let shared = self.caller.data().shared_state.as_ref()?;
        let mut table = shared.sab_table.lock().ok()?;
        let handle = table.len() as u32;
        table.push(crate::shared_buffer::SharedArrayBufferEntry {
            data: Arc::new(RwLock::new(vec![0u8; byte_length as usize])),
            byte_length: byte_length as u64,
            max_byte_length: None,
        });
        Some(handle)
    }
    fn shared_arraybuffer_byte_length(&mut self, handle: u32) -> Option<u32> {
        let shared = self.caller.data().shared_state.as_ref()?;
        let table = shared.sab_table.lock().ok()?;
        table.get(handle as usize).map(|e| e.byte_length as u32)
    }
    fn shared_arraybuffer_create_object(
        &mut self,
        target: Value,
        byte_length: u64,
        max_byte_length: Option<u64>,
    ) -> Value {
        crate::shared_buffer::create_shared_array_buffer_object(
            self.caller,
            target,
            byte_length,
            max_byte_length,
        )
    }
    fn shared_arraybuffer_info(&mut self, this: Value) -> Option<(u32, u64, Option<u64>)> {
        crate::shared_buffer::shared_array_buffer_info(self.caller, this)
    }
    fn shared_arraybuffer_grow(&mut self, this: Value, new_length: u64) -> bool {
        crate::shared_buffer::grow_shared_array_buffer_backing(self.caller, this, new_length)
    }
    fn shared_arraybuffer_slice(&mut self, this: Value, start: u64, end: u64) -> Option<Value> {
        crate::shared_buffer::slice_shared_array_buffer_backing(self.caller, this, start, end)
    }
    };
}
