// ExecContext 方法片段：error
macro_rules! exec_ctx_error {
    () => {
    fn throw_exception(&mut self, val: Value) {
        let rendered = crate::runtime_render::render_value(self.caller, val)
            .unwrap_or_else(|_| "unknown".to_string());
        crate::runtime_promises::set_runtime_error(self.caller.data(), rendered);
        let _ = crate::runtime_host_helpers::make_exception_value(self.caller, val);
    }
    fn set_last_error(&mut self, msg: String) {
        crate::runtime_promises::set_runtime_error(self.caller.data(), msg);
    }
    fn take_last_error(&mut self) -> Option<String> {
        self.caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
    fn make_type_error(&mut self, msg: &str) -> Value {
        crate::runtime_host_helpers::make_type_error_exception(self.caller, msg)
    }
    fn make_exception(&mut self, value: Value) -> Value {
        crate::runtime_host_helpers::make_exception_value(self.caller, value)
    }
    fn make_syntax_error(&mut self, msg: &str) -> Value {
        crate::runtime_host_helpers::make_syntax_error_exception(self.caller, msg)
    }
    fn make_range_error(&mut self, msg: &str) -> Value {
        crate::runtime_host_helpers::make_range_error_exception(self.caller, msg)
    }
    fn create_error_object(&mut self, name: &str, message_arg: Value, options: Value) -> Value {
        crate::runtime_heap::create_error_object(self.caller, name, message_arg, options)
    }
    fn push_exception(&mut self, name: &str, message: &str, error_obj: Value) -> Value {
        let mut errors = self
            .caller
            .data()
            .error_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = errors.len() as u32;
        errors.push(crate::ErrorEntry {
            name: name.to_string(),
            message: message.to_string(),
            value: error_obj,
        });
        value::encode_handle(value::TAG_EXCEPTION, idx)
    }
    fn pending_exit_signal(&mut self) -> Option<i32> {
        crate::runtime_process::pending_process_exit_signal(self.caller.data()).map(|s| s.code)
    }
    };
}
