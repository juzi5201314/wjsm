// ExecContext 方法片段：streams
macro_rules! exec_ctx_streams {
    () => {
    fn stream_create_uint8array(&mut self, bytes: &[u8]) -> Value {
        let env = self.env().expect("WasmEnv");
        crate::host_imports::create_uint8array_with_env(self.caller, &env, bytes)
    }
    fn stream_typedarray_u8_bytes(&mut self, typedarray: Value) -> Option<Vec<u8>> {
        crate::host_imports::typedarray_u8_bytes(self.caller, typedarray)
    }
    fn stream_write_u8_bytes(&mut self, view: Value, bytes: &[u8]) -> Option<usize> {
        crate::host_imports::write_u8_bytes_to_view(self.caller, view, bytes)
    }
    fn stream_transfer_byob_view(
        &mut self,
        view: Value,
        bytes_written: usize,
    ) -> Option<Value> {
        let env = self.env()?;
        crate::host_imports::transfer_byob_view_with_env(
            self.caller,
            &env,
            view,
            bytes_written,
        )
    }
    fn schedule_readable_pull(
        &mut self,
        callback: Value,
        this_value: Value,
        controller: Value,
    ) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::ReadableStreamPull {
                callback,
                this_val: this_value,
                controller,
            });
    }
    fn schedule_readable_pipe_pump(&mut self, readable_handle: u32) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::ReadableStreamPipeToPump { readable_handle });
    }
    fn schedule_writable_sink_write(
        &mut self,
        callback: Value,
        this_value: Value,
        chunk: Value,
        controller: Value,
        write_promise: Value,
    ) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::WritableStreamSinkWrite {
                callback,
                this_val: this_value,
                chunk,
                controller,
                write_promise,
            });
    }
    fn schedule_writable_sink_close(
        &mut self,
        callback: Option<Value>,
        this_value: Value,
        controller: Value,
        writable_stream_handle: u32,
        close_promise: Value,
    ) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::WritableStreamSinkClose {
                callback,
                this_val: this_value,
                controller,
                writable_stream_handle,
                close_promise,
            });
    }
    fn schedule_transform_stream_transform(
        &mut self,
        callback: Value,
        this_value: Value,
        chunk: Value,
        controller: Value,
        write_promise: Value,
    ) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::TransformStreamTransform {
                callback,
                this_val: this_value,
                chunk,
                controller,
                write_promise,
            });
    }
    fn schedule_transform_stream_flush(
        &mut self,
        params: wjsm_host::TransformStreamFlushParams,
    ) {
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(crate::Microtask::TransformStreamFlush {
                callback: params.callback,
                this_val: params.this_value,
                controller: params.controller,
                writable_stream_handle: params.writable_stream_handle,
                readable_stream_handle: params.readable_stream_handle,
                readable_controller_handle: params.readable_controller_handle,
                close_promise: params.close_promise,
            });
    }
    fn mark_response_body_used(
        &mut self,
        response_handle: Option<u32>,
        response_obj: Option<Value>,
    ) {
        crate::host_imports::mark_response_body_used_from_caller(
            self.caller,
            response_handle,
            response_obj,
        );
    }
    fn mark_writable_stream_signal_aborted(&mut self, stream_handle: u32, reason: Value) {
        crate::host_imports::mark_writable_stream_signal_aborted(
            self.caller,
            stream_handle,
            reason,
        );
    }
    fn create_writable_abort_signal(&mut self) -> Value {
        crate::host_imports::create_writable_abort_signal_object(self.caller)
    }
    fn fetch_body_reader_read(
        &mut self,
        reader_handle: u32,
        http_handle: u32,
        byob_view: Option<Value>,
    ) -> Option<Value> {
        crate::host_imports::call_fetch_body_reader_read(
            self.caller,
            reader_handle,
            http_handle,
            byob_view,
        )
    }
    fn cancel_http_response(&mut self, http_handle: u32) {
        crate::host_imports::cancel_http_response_from_caller(self.caller, http_handle);
    }
    fn alloc_readable_stream(&mut self, entry: wjsm_host::ReadableStreamEntry) -> u32 {
        self.caller.data().readable_stream_table.alloc(entry)
    }
    fn with_readable_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::ReadableStreamEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .readable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn bind_readable_stream_object(&mut self, object: Handle, handle: u32) {
        self.caller
            .data()
            .readable_stream_table
            .bind_obj_handle(object, handle);
    }
    fn alloc_reader(&mut self, entry: wjsm_host::ReaderEntry) -> u32 {
        self.caller.data().reader_table.alloc(entry)
    }
    fn with_reader<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::ReaderEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn with_readers<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<wjsm_host::ReaderEntry>]) -> R,
    ) -> R {
        let mut table = self
            .caller
            .data()
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(&mut table.entries)
    }
    fn bind_reader_object(&mut self, object: Handle, handle: u32) {
        self.caller.data().reader_table.bind_obj_handle(object, handle);
    }
    fn alloc_stream_controller(&mut self, entry: wjsm_host::StreamControllerEntry) -> u32 {
        self.caller.data().stream_controller_table.alloc(entry)
    }
    fn with_stream_controller<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::StreamControllerEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn bind_stream_controller_object(&mut self, object: Handle, handle: u32) {
        self.caller
            .data()
            .stream_controller_table
            .bind_obj_handle(object, handle);
    }
    fn alloc_byob_request(&mut self, entry: wjsm_host::ByobRequestEntry) -> u32 {
        self.caller.data().byob_request_table.alloc(entry)
    }
    fn with_byob_request<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::ByobRequestEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .byob_request_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn bind_byob_request_object(&mut self, object: Handle, handle: u32) {
        self.caller
            .data()
            .byob_request_table
            .bind_obj_handle(object, handle);
    }
    fn alloc_writable_stream(&mut self, entry: wjsm_host::WritableStreamEntry) -> u32 {
        self.caller.data().writable_stream_table.alloc(entry)
    }
    fn with_writable_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::WritableStreamEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn bind_writable_stream_object(&mut self, object: Handle, handle: u32) {
        self.caller
            .data()
            .writable_stream_table
            .bind_obj_handle(object, handle);
    }
    fn alloc_writer(&mut self, entry: wjsm_host::WriterEntry) -> u32 {
        self.caller.data().writer_table.alloc(entry)
    }
    fn with_writer<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::WriterEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .writer_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn with_writers<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<wjsm_host::WriterEntry>]) -> R,
    ) -> R {
        let mut table = self
            .caller
            .data()
            .writer_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(&mut table.entries)
    }
    fn bind_writer_object(&mut self, object: Handle, handle: u32) {
        self.caller.data().writer_table.bind_obj_handle(object, handle);
    }
    fn alloc_transform_stream(&mut self, entry: wjsm_host::TransformStreamEntry) -> u32 {
        self.caller.data().transform_stream_table.alloc(entry)
    }
    fn with_transform_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::TransformStreamEntry) -> R,
    ) -> Option<R> {
        let mut table = self
            .caller
            .data()
            .transform_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.get_mut(handle as usize).map(f)
    }
    fn with_transform_streams<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<wjsm_host::TransformStreamEntry>]) -> R,
    ) -> R {
        let mut table = self
            .caller
            .data()
            .transform_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(&mut table.entries)
    }
    fn bind_transform_stream_object(&mut self, object: Handle, handle: u32) {
        self.caller
            .data()
            .transform_stream_table
            .bind_obj_handle(object, handle);
    }
    };
}
