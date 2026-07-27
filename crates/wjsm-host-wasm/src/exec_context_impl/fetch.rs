// ExecContext 方法片段：fetch
macro_rules! exec_ctx_fetch {
    () => {
    fn alloc_headers(&mut self, entry: wjsm_host::HeadersEntry) -> u32 {
        let mut table = self
            .caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(entry);
        handle
    }
    fn with_headers<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::HeadersEntry) -> R,
    ) -> Option<R> {
        self.caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(handle as usize)
            .map(f)
    }
    fn alloc_fetch_response(&mut self, entry: wjsm_host::FetchResponseEntry) -> u32 {
        let mut table = self
            .caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(entry);
        handle
    }
    fn with_fetch_response<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::FetchResponseEntry) -> R,
    ) -> Option<R> {
        self.caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(handle as usize)
            .map(f)
    }
    fn alloc_fetch_request(&mut self, entry: wjsm_host::FetchRequestEntry) -> u32 {
        let mut table = self
            .caller
            .data()
            .fetch_request_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(entry);
        handle
    }
    fn with_fetch_request<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::FetchRequestEntry) -> R,
    ) -> Option<R> {
        self.caller
            .data()
            .fetch_request_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(handle as usize)
            .map(f)
    }
    fn alloc_abort_signal(&mut self, entry: wjsm_host::AbortSignalEntry) -> u32 {
        let mut table = self
            .caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(entry);
        handle
    }
    fn with_abort_signal<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut wjsm_host::AbortSignalEntry) -> R,
    ) -> Option<R> {
        self.caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(handle as usize)
            .map(f)
    }
    fn http_fetch_begin<'a>(
        &'a mut self,
        request: wjsm_host::HttpRequestSpec,
    ) -> wjsm_host::ExecFuture<'a> {
        let guard = self
            .caller
            .data()
            .async_op_counter
            .as_ref()
            .map(|counter| counter.begin());
        Box::pin(async move {
            let result = crate::host_imports::perform_http_fetch(
                self.caller,
                request.method,
                request.url,
                request.headers_handle,
                request.body,
                request.redirect,
                request.signal_handle,
                request.resource_timing,
            )
            .await
            .map_err(anyhow::Error::msg);
            drop(guard);
            result
        })
    }
    fn create_arraybuffer_from_bytes(&mut self, bytes: &[u8]) -> Value {
        crate::host_imports::create_arraybuffer_with_bytes(self.caller, bytes)
    }
    fn consume_fetch_body_to_bytes(
        &mut self,
        http_handle: u32,
        promise: Value,
        kind: wjsm_host::ResponseMethodKind,
    ) -> bool {
        crate::host_imports::consume_fetch_body_to_bytes(
            self.caller,
            http_handle,
            promise,
            kind,
        )
    }
    fn fetch_resource_timing_enabled(&mut self) -> bool {
        crate::runtime_node_perf_hooks::resource_entries_enabled(self.caller.data())
    }
    fn performance_now(&mut self) -> f64 {
        self.caller.data().performance_origin.elapsed().as_secs_f64() * 1_000.0
    }
    fn commit_fetch_resource_timing(&mut self, timing: &wjsm_host::FetchResourceTimingState) {
        crate::runtime_node_perf_hooks::queue_resource_entry(
            self.caller.data(),
            crate::runtime_node_perf_hooks::NativeResourceTiming {
                name: timing.requested_url.clone(),
                start_time: timing.start_time,
                request_start_time: timing.request_start_time,
                response_start_time: timing.response_start_time,
                end_time: self.caller.data().performance_origin.elapsed().as_secs_f64() * 1_000.0,
                response_status: timing.response_status,
                encoded_body_size: timing.encoded_body_size,
                decoded_body_size: timing.decoded_body_size,
            },
        );
    }
    };
}
