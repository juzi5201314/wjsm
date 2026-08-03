// ExecContext 方法片段：call
macro_rules! exec_ctx_call {
    () => {
    fn call_js(&mut self, func: Value, this: Value, args: &[Value]) -> anyhow::Result<Value> {
        // 与 get_method / private_fields 等路径一致：block_in_place + 当前 runtime block_on。
        let rt = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            rt.block_on(crate::runtime_host_helpers::call_wasm_callback_async(
                self.caller,
                func,
                this,
                args,
            ))
        })
    }
    fn call_js_async<'c>(
        &'c mut self,
        func: Value,
        this: Value,
        args: &'c [Value],
    ) -> ExecFuture<'c> {
        Box::pin(async move {
            crate::runtime_host_helpers::call_wasm_callback_async(self.caller, func, this, args)
                .await
        })
    }
    fn call_native_callable(&mut self, func: Value, this: Value, args: &[Value]) -> Value {
        crate::call_native_callable_with_args_from_caller(self.caller, func, this, args.to_vec())
            .unwrap_or_else(value::encode_undefined)
    }
    fn dispatch_native_callable(&mut self, idx: u32, this: Value, args: &[Value]) -> Option<Value> {
        let env = self.env()?;
        let callable = value::encode_native_callable_idx(idx);
        crate::runtime_host_helpers::dispatch_native_callable_with_env(
            self.caller,
            &env,
            callable,
            this,
            args,
        )
    }
    fn create_native_callable(&self, callable: wjsm_host::NativeCallableRef) -> Value {
        let internal = match callable {
            wjsm_host::NativeCallableRef::QueuingStrategySize { kind } => {
                let internal_kind = match kind {
                    wjsm_host::QueuingStrategySizeKind::Count => {
                        crate::QueuingStrategySizeKind::Count
                    }
                    wjsm_host::QueuingStrategySizeKind::ByteLength => {
                        crate::QueuingStrategySizeKind::ByteLength
                    }
                };
                crate::NativeCallable::QueuingStrategySize {
                    kind: internal_kind,
                }
            }
            wjsm_host::NativeCallableRef::HeadersMethod { handle, kind } => {
                crate::NativeCallable::HeadersMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::ResponseMethod { handle, kind } => {
                crate::NativeCallable::ResponseMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::RequestMethod { handle, kind } => {
                crate::NativeCallable::RequestMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::AbortControllerAbort { signal_handle } => {
                crate::NativeCallable::AbortControllerAbort { signal_handle }
            }

            wjsm_host::NativeCallableRef::CjsRequireResolve { referrer } => {
                crate::NativeCallable::CjsRequireResolve { referrer }
            }
            wjsm_host::NativeCallableRef::CjsRequireResolvePaths { referrer } => {
                crate::NativeCallable::CjsRequireResolvePaths { referrer }
            }
            wjsm_host::NativeCallableRef::ImportMetaResolve { referrer } => {
                crate::NativeCallable::ImportMetaResolve { referrer }
            }
            wjsm_host::NativeCallableRef::ReadableStreamConstructor => {
                crate::NativeCallable::ReadableStreamConstructor
            }
            wjsm_host::NativeCallableRef::ReadableStreamMethod { handle, kind } => {
                crate::NativeCallable::ReadableStreamMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::ReadableStreamDefaultReaderMethod { handle, kind } => {
                crate::NativeCallable::ReadableStreamDefaultReaderMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::ReadableStreamDefaultControllerMethod { handle, kind } => {
                crate::NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::ReadableStreamByobRequestMethod { handle, kind } => {
                crate::NativeCallable::ReadableStreamByobRequestMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::ReadableStreamAsyncIteratorNext { reader_handle } => {
                crate::NativeCallable::ReadableStreamAsyncIteratorNext { reader_handle }
            }
            wjsm_host::NativeCallableRef::ReadableStreamAsyncIteratorReturn { reader_handle } => {
                crate::NativeCallable::ReadableStreamAsyncIteratorReturn { reader_handle }
            }
            wjsm_host::NativeCallableRef::ReadableStreamPipeToWriteFulfilled {
                readable_handle,
            } => crate::NativeCallable::ReadableStreamPipeToWriteFulfilled { readable_handle },
            wjsm_host::NativeCallableRef::ReadableStreamPipeToWriteRejected {
                readable_handle,
            } => crate::NativeCallable::ReadableStreamPipeToWriteRejected { readable_handle },
            wjsm_host::NativeCallableRef::WritableStreamConstructor => {
                crate::NativeCallable::WritableStreamConstructor
            }
            wjsm_host::NativeCallableRef::WritableStreamMethod { handle, kind } => {
                crate::NativeCallable::WritableStreamMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::WritableStreamDefaultWriterMethod { handle, kind } => {
                crate::NativeCallable::WritableStreamDefaultWriterMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::WritableStreamDefaultControllerMethod { handle, kind } => {
                crate::NativeCallable::WritableStreamDefaultControllerMethod { handle, kind }
            }
            wjsm_host::NativeCallableRef::TransformStreamConstructor => {
                crate::NativeCallable::TransformStreamConstructor
            }
            wjsm_host::NativeCallableRef::TransformStreamMethod { handle, kind } => {
                crate::NativeCallable::TransformStreamMethod { handle, kind }
            }
        };
        crate::create_native_callable(self.caller.data(), internal)
    }
    fn is_callable(&mut self, val: Value) -> bool {
        let Some(env) = self.env() else {
            return value::is_callable(val);
        };
        crate::runtime_host_helpers::is_callable_with_env(self.caller, &env, val)
    }
    fn prepare_callback(&mut self, func: Value) -> Option<wjsm_host::PreparedCallback> {
        let env = self.env()?;
        let target = crate::runtime_host_helpers::resolve_callback_target_with_env(
            self.caller,
            &env,
            func,
        )
        .ok()?;
        match target {
            crate::runtime_host_helpers::CallbackTarget::Wasm { func_idx, env_obj } => Some(
                wjsm_host::PreparedCallback::direct(func, func_idx, env_obj),
            ),
            _ => Some(wjsm_host::PreparedCallback::generic(func)),
        }
    }
    fn call_prepared_async<'c>(
        &'c mut self,
        prepared: &'c wjsm_host::PreparedCallback,
        this: Value,
        args: &'c [Value],
    ) -> ExecFuture<'c> {
        Box::pin(async move {
            if prepared.is_direct() {
                crate::runtime_host_helpers::call_direct_wasm_async(
                    self.caller,
                    prepared.func_idx(),
                    prepared.env_obj(),
                    this,
                    args,
                )
                .await
            } else {
                crate::runtime_host_helpers::call_wasm_callback_async(
                    self.caller,
                    prepared.func(),
                    this,
                    args,
                )
                .await
            }
        })
    }
    fn proxy_entry(&mut self, proxy: Handle) -> Option<ProxyEntry> {
        let table = self
            .caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(proxy as usize)?;
        if entry.revoked {
            return None;
        }
        Some(ProxyEntry {
            target: entry.target,
            handler: entry.handler,
        })
    }
    fn proxy_entry_any(&mut self, proxy: Handle) -> Option<ProxyEntry> {
        let table = self
            .caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(proxy as usize)?;
        Some(ProxyEntry {
            target: entry.target,
            handler: entry.handler,
        })
    }
    fn alloc_proxy(&mut self, target: Value, handler: Value) -> Value {
        let mut table = self
            .caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(crate::ProxyEntry {
            target,
            handler,
            revoked: false,
        });
        value::encode_proxy_handle(handle)
    }
    fn create_proxy_revoker(&mut self, proxy: Value) -> Value {
        let mut callables = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let index = callables.len() as u32;
        callables.push(crate::NativeCallable::ProxyRevoker {
            proxy_handle: value::decode_proxy_handle(proxy),
        });
        value::encode_native_callable_idx(index)
    }
    fn proxy_is_revoked(&mut self, proxy: Handle) -> bool {
        let table = self
            .caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(proxy as usize)
            .map(|e| e.revoked)
            .unwrap_or(false)
    }
    fn closure_entry(&mut self, handle: Handle) -> Option<ClosureEntry> {
        let table = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(handle as usize)?;
        Some(ClosureEntry {
            func_idx: entry.func_idx,
            env_obj: entry.env_obj,
        })
    }
    fn closure_env(&mut self, idx: u32) -> Option<Value> {
        let closures = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        closures.get(idx as usize).map(|e| e.env_obj)
    }
    fn closure_func_idx(&mut self, idx: u32) -> Option<u32> {
        let closures = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        closures.get(idx as usize).map(|e| e.func_idx)
    }
    fn bound_entry(&mut self, handle: Handle) -> Option<BoundEntry> {
        let table = self
            .caller
            .data()
            .bound_objects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(handle as usize)?;
        Some(BoundEntry {
            target_func: entry.target_func,
            bound_this: entry.bound_this,
            bound_args: entry.bound_args.clone(),
        })
    }
    fn create_bound_function(
        &mut self,
        target: Value,
        this_arg: Value,
        bound_args: Vec<Value>,
    ) -> Value {
        let mut bound = self
            .caller
            .data()
            .bound_objects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = bound.len() as u32;
        bound.push(crate::types::BoundRecord {
            target_func: target,
            bound_this: this_arg,
            bound_args,
        });
        value::encode_bound_idx(idx)
    }
    fn create_closure(&mut self, func_idx: u32, env_obj: Value) -> Value {
        let mut closures = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = closures.len() as u32;
        closures.push(crate::ClosureEntry { func_idx, env_obj });
        value::encode_closure_idx(idx)
    }
    fn resolve_func_table_idx(&mut self, fn_val: Value) -> u32 {
        if value::is_function(fn_val) {
            value::decode_function_idx(fn_val)
        } else if value::is_closure(fn_val) {
            let idx = value::decode_closure_idx(fn_val);
            let closures = self
                .caller
                .data()
                .closures
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            closures.get(idx as usize).map(|e| e.func_idx).unwrap_or(0)
        } else {
            crate::runtime_promises::nanbox_to_u32(fn_val)
        }
    }
    fn call_symbol_method_async<'c>(
        &'c mut self,
        target: Value,
        symbol_idx: u32,
        this_arg: Value,
        args: &'c [Value],
    ) -> ExecFuture<'c, Option<Value>> {
        Box::pin(async move {
            crate::call_symbol_method_async(self.caller, target, symbol_idx, this_arg, args).await
        })
    }
    fn function_closure_identity_eq(&mut self, func: Value, closure: Value) -> bool {
        let func_idx = func as u32;
        let closure_idx = closure as u32;
        let closures = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        closures
            .get(closure_idx as usize)
            .map(|c| c.func_idx == func_idx)
            .unwrap_or(false)
    }
    };
}
