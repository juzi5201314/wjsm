//! wasmtime 后端的 [`wjsm_host::ExecContext`] 实现。
//!
//! 零成本构造：持有 `&mut Caller` + 一次 `WasmEnv` 提取。
//! 所有方法委托到现有 host-wasm helper（heap_access_v2 / runtime_host_helpers 等）。

use crate::runtime_string::RuntimeString;
use crate::types::EnumeratorState;
use crate::{RuntimeState, WasmEnv, value};
use wasmtime::Caller;
use wjsm_host::{
    AsyncHookEvent, BoundEntry, ClosureEntry, ExecContext, ExecFuture, GcOutcome, Handle,
    HeapContext, IteratorNextStep, PromiseSettlement, ProxyEntry, RegExpMatchInfo, TypedArrayView,
    Value,
};

/// Wasmtime 后端的 [`ExecContext`] / [`HeapContext`] 实现。
///
/// `env` 在构造时从 Caller 提取（`WasmEnv: Copy`），避免后续方法因
/// `get_export` 需要 `&mut Caller` 而无法读取线性内存。
pub(crate) struct WasmExecContext<'a, 'b> {
    caller: &'a mut Caller<'b, RuntimeState>,
    env: Option<WasmEnv>,
}

impl<'a, 'b> WasmExecContext<'a, 'b> {
    pub(crate) fn new(caller: &'a mut Caller<'b, RuntimeState>) -> Self {
        let env = WasmEnv::from_caller(caller);
        Self { caller, env }
    }

    fn property_key(&mut self, key: &str) -> u32 {
        let index = crate::property_key::intern_runtime_property_key(
            self.caller.data(),
            RuntimeString::from_utf8_str(key),
        );
        crate::property_key::encode_runtime_string_name_id(index)
    }

    fn heap_access(&mut self) -> Option<&crate::runtime_gc::HeapAccessV2> {
        self.caller.data().heap_access_v2.as_deref()
    }

}

impl HeapContext for WasmExecContext<'_, '_> {
    fn read_shadow_arg(&mut self, args_base: i32, index: u32) -> Value {
        let Some(env) = self.env else {
            return value::encode_undefined();
        };
        crate::runtime_host_helpers::read_shadow_arg_with_env(
            self.caller,
            &env,
            args_base,
            index,
        )
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
        let Some(env) = self.env else {
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
        let Some(env) = self.env else {
            return GcOutcome::default();
        };
        let algorithm = self.caller.data().gc_algorithm;
        let stats = crate::runtime_gc::active_zgc::collect_dispatch(
            self.caller,
            &env,
            algorithm.as_str(),
        );
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

impl ExecContext for WasmExecContext<'_, '_> {
    fn store_string(&mut self, s: &str) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s.to_string())
    }

    fn store_string_owned(&mut self, s: String) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s)
    }

    fn read_string_bytes(&mut self, val: Value) -> Option<Vec<u8>> {
        crate::runtime_render::read_value_string_bytes(self.caller, val)
    }

    fn read_string_utf8_lossy(&mut self, val: Value) -> String {
        crate::runtime_render::read_runtime_string_utf8_lossy(self.caller, val)
    }

    fn canonicalize_name_id(&mut self, name_id: u32) -> Option<u32> {
        crate::property_key::canonicalize_v2_name_id(self.caller, name_id)
    }

    fn intern_property_key(&mut self, s: &str) -> u32 {
        let index = crate::property_key::intern_runtime_property_key(
            self.caller.data(),
            RuntimeString::from_utf8_str(s),
        );
        crate::property_key::encode_runtime_string_name_id(index)
    }

    fn property_key_string(&mut self, name_id: u32) -> Option<String> {
        match crate::property_key::decode_name_id(name_id) {
            crate::property_key::DecodedNameId::RuntimeString(index) => {
                crate::property_key::runtime_property_key_units(self.caller.data(), index)
                    .map(|rs| rs.to_utf8_lossy())
            }
            crate::property_key::DecodedNameId::MemoryString(index) => {
                let Some(env) = self.env else {
                    return None;
                };
                let bytes =
                    crate::runtime_render::read_string_bytes_mem(self.caller, &env.memory, index);
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            crate::property_key::DecodedNameId::Symbol(_) => None,
        }
    }

    fn name_id_matches(&mut self, name_id: u32, expected: &str) -> bool {
        let key = RuntimeString::from_utf8_str(expected);
        let Some(env) = self.env else {
            return false;
        };
        crate::property_key::name_id_matches_runtime_string(self.caller, &env, name_id, &key)
    }

    fn property_value_to_name_id(&mut self, prop: Value, allow_symbol: bool) -> Option<u32> {
        if !allow_symbol && value::is_symbol(prop) {
            return None;
        }
        crate::property_key::property_key_value_to_name_id(self.caller, prop, true)
    }

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

    fn is_callable(&mut self, val: Value) -> bool {
        let Some(env) = self.env else {
            return value::is_callable(val);
        };
        crate::runtime_host_helpers::is_callable_with_env(self.caller, &env, val)
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

    fn dispatch_native_callable(
        &mut self,
        idx: u32,
        this: Value,
        args: &[Value],
    ) -> Option<Value> {
        let Some(env) = self.env else {
            return None;
        };
        let callable = value::encode_native_callable_idx(idx);
        crate::runtime_host_helpers::dispatch_native_callable_with_env(
            self.caller,
            &env,
            callable,
            this,
            args,
        )
    }

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

    fn define_data_property(&mut self, obj: Value, key: &str, value: Value) {
        let _ = crate::runtime_host_helpers::define_host_data_property(
            self.caller,
            obj,
            key,
            value,
        );
    }

    fn get_property_by_name_id(&mut self, obj: Value, name_id: u32) -> Value {
        wjsm_builtins::get_method::get_by_name_id(self, obj, name_id)
    }

    fn get_method_by_name_id(
        &mut self,
        obj: Value,
        name_id: u32,
    ) -> anyhow::Result<Option<Value>> {
        match wjsm_builtins::get_method::get_method_by_name_id(self, obj, name_id) {
            Ok(v) => Ok(v),
            Err(exc) => Err(anyhow::anyhow!(
                "TypeError: method is not callable (exception={exc})"
            )),
        }
    }

    fn array_write_elem(&mut self, arr: Value, index: u32, value: Value) {
        let Some(env) = self.env else {
            return;
        };
        crate::runtime_host_helpers::set_array_elem_with_env(
            self.caller,
            &env,
            arr,
            index as i32,
            value,
        );
    }

    fn array_read_length(&mut self, arr: Value) -> Option<u32> {
        let Some(env) = self.env else {
            return None;
        };
        let ptr = crate::runtime_values::resolve_array_ptr_with_env(self.caller, &env, arr)?;
        crate::runtime_values::read_array_length_with_env(self.caller, &env, ptr)
    }

    fn array_write_length(&mut self, arr: Value, len: u32) {
        let Some(env) = self.env else {
            return;
        };
        let Some(ptr) = crate::runtime_values::resolve_array_ptr_with_env(self.caller, &env, arr)
        else {
            return;
        };
        crate::runtime_values::write_array_length_with_env(self.caller, &env, ptr, len);
    }

    fn render_value(&mut self, val: Value) -> String {
        crate::runtime_render::render_value(self.caller, val).unwrap_or_default()
    }

    fn alloc_promise(&mut self) -> Value {
        // 与 async_function_start 等历史路径一致：host_helpers::alloc_promise
        // （含 promise prototype + promise_table 插入）。
        crate::runtime_host_helpers::alloc_promise(
            self.caller,
            crate::types::PromiseEntry::pending(),
        )
    }

    fn settle_promise(&mut self, promise: Value, settlement: PromiseSettlement) {
        crate::runtime_promises::settle_promise(self.caller.data(), promise, settlement);
    }

    fn resolve_promise(&mut self, promise: Value, value: Value) {
        crate::runtime_promises::resolve_promise_from_caller(self.caller, promise, value);
    }

    fn alloc_promise_with_entry(&mut self, entry: wjsm_host::PromiseEntry) -> Value {
        // 转换后端无关 PromiseEntry → host-wasm 内部 PromiseEntry
        let internal = convert_promise_entry(entry);
        crate::alloc_promise_from_caller(self.caller, internal)
    }

    fn raw_promise_handle(&self, promise: Value) -> usize {
        crate::raw_promise_handle(promise)
    }

    fn promise_result_species_constructor_handle(&mut self, exemplar: Value) -> Option<Value> {
        crate::promise_result_species_constructor_handle(self.caller, exemplar)
    }

    fn set_promise_proto_from_constructor(&mut self, promise: Value, constructor: Value) {
        // trait 层 constructor 是 Value（非 Option）；内部签名接受 Option<Value>，
        // undefined → None（内建 Promise 快速路径）。
        let ctor_opt = if value::is_undefined(constructor) {
            None
        } else {
            Some(constructor)
        };
        crate::set_promise_proto_from_constructor(self.caller, promise, ctor_opt);
    }

    fn create_promise_resolving_function(
        &mut self,
        promise: Value,
        kind: wjsm_host::PromiseResolvingKind,
    ) -> Value {
        let handle = crate::raw_promise_handle(promise);
        let already_resolved = {
            let mut table = self
                .caller
                .data()
                .promise_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match crate::promise_entry_mut(&mut table, handle) {
                Some(entry) => entry.constructor_resolver.clone(),
                None => None,
            }
        }
        .unwrap_or_else(|| std::sync::Arc::new(std::sync::Mutex::new(false)));
        let internal_kind = match kind {
            wjsm_host::PromiseResolvingKind::Fulfill => crate::PromiseResolvingKind::Fulfill,
            wjsm_host::PromiseResolvingKind::Reject => crate::PromiseResolvingKind::Reject,
        };
        crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            already_resolved,
            internal_kind,
        )
    }

    fn new_promise_capability(&mut self, constructor: Value) -> (Value, Value, Value) {
        crate::new_promise_capability_from_caller(self.caller, constructor)
    }

    fn capture_child_promise_scope(
        &mut self,
        promise: Value,
        parent: Option<wjsm_host::CapturedScope>,
    ) -> Option<wjsm_host::CapturedScope> {
        let parent_internal = parent.map(convert_captured_scope);
        // capture_child_promise_scope 在 host_imports::promise 中是私有的，
        // 但算法逻辑经 async_hooks 直接实现，这里内联以避免可见性问题。
        let (scope, emit_init) = {
            let mut hooks = self
                .caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut scope =
                hooks.capture_promise_scope(promise, parent_internal.map(|s| s.async_id));
            if let (Some(scope), Some(parent_scope)) = (&mut scope, parent_internal) {
                scope.frame_id = parent_scope.frame_id;
            }
            let emit_init = scope.is_some() && hooks.init_hooks_exist();
            (scope, emit_init)
        };
        if emit_init {
            let cached_type = self
                .caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .promise_type_value();
            let type_value = cached_type.unwrap_or_else(|| {
                let value = crate::runtime_render::store_runtime_string(
                    self.caller,
                    "PROMISE".to_string(),
                );
                self.caller
                    .data()
                    .async_hooks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_promise_type_value(value);
                value
            });
            if let Some(scope) = scope {
                self.caller
                    .data()
                    .async_hooks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .queue_promise_event(
                        crate::runtime_async_hooks::PendingPromiseHookEvent::Init {
                            scope,
                            type_value,
                        },
                    );
            }
        }
        scope.map(convert_captured_scope_back)
    }

    fn clear_pending_unhandled_rejection(&self, handle: usize) {
        crate::runtime_microtask::clear_pending_unhandled_rejection(self.caller.data(), handle);
    }

    fn push_pending_unhandled_rejection(&self, handle: usize) {
        self.caller
            .data()
            .pending_unhandled_rejections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(handle);
    }

    fn promise_constructor_handle(&self, promise: Value) -> Option<Value> {
        let handle = crate::raw_promise_handle(promise);
        let table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::promise_entry(&table, handle).and_then(|e| e.constructor_handle)
    }

    fn is_thenable(&mut self, val: Value) -> bool {
        let Some(env) = self.env else {
            return false;
        };
        crate::is_thenable_value(self.caller, &env, val)
    }

    fn create_combinator_reaction_handler(
        &self,
        context: u32,
        index: usize,
        kind: wjsm_host::PromiseCombinatorReactionKind,
    ) -> Value {
        let internal_kind = match kind {
            wjsm_host::PromiseCombinatorReactionKind::AllFulfill => {
                crate::PromiseCombinatorReactionKind::AllFulfill
            }
            wjsm_host::PromiseCombinatorReactionKind::AllReject => {
                crate::PromiseCombinatorReactionKind::AllReject
            }
            wjsm_host::PromiseCombinatorReactionKind::AllSettledFulfill => {
                crate::PromiseCombinatorReactionKind::AllSettledFulfill
            }
            wjsm_host::PromiseCombinatorReactionKind::AllSettledReject => {
                crate::PromiseCombinatorReactionKind::AllSettledReject
            }
            wjsm_host::PromiseCombinatorReactionKind::AnyFulfill => {
                crate::PromiseCombinatorReactionKind::AnyFulfill
            }
            wjsm_host::PromiseCombinatorReactionKind::AnyReject => {
                crate::PromiseCombinatorReactionKind::AnyReject
            }
            // host-wasm 内部无 Race 专用变体；race 用 AllFulfill/AllReject 语义
            // （第一个 settle 的赢，其余忽略）。
            wjsm_host::PromiseCombinatorReactionKind::RaceFulfill => {
                crate::PromiseCombinatorReactionKind::AllFulfill
            }
            wjsm_host::PromiseCombinatorReactionKind::RaceReject => {
                crate::PromiseCombinatorReactionKind::AllReject
            }
        };
        crate::create_combinator_reaction_handler(
            self.caller.data(),
            context as usize,
            index,
            internal_kind,
        )
    }

    fn create_combinator_context(&self, result_promise: Value, result_array: Value) -> u32 {
        crate::create_combinator_context(self.caller.data(), result_promise, result_array)
            as u32
    }

    fn promise_constructor_resolver(
        &self,
        promise: Value,
    ) -> Option<std::sync::Arc<std::sync::Mutex<bool>>> {
        let handle = crate::raw_promise_handle(promise);
        let table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::promise_entry(&table, handle).and_then(|e| e.constructor_resolver.clone())
    }

    fn push_promise_reaction(
        &mut self,
        promise: Value,
        reaction: wjsm_host::PromiseReaction,
        is_fulfill: bool,
    ) {
        let handle = crate::raw_promise_handle(promise);
        let internal = convert_promise_reaction(reaction);
        let mut table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = crate::promise_entry_mut(&mut table, handle) {
            if is_fulfill {
                entry.fulfill_reactions.push(internal);
            } else {
                entry.reject_reactions.push(internal);
            }
        }
    }

    fn queue_promise_reaction_microtask(
        &self,
        promise: Value,
        reaction_type: wjsm_host::ReactionType,
        handler: Value,
        argument: Value,
        scope: Option<wjsm_host::CapturedScope>,
    ) {
        let internal_type = match reaction_type {
            wjsm_host::ReactionType::Fulfill => crate::ReactionType::Fulfill,
            wjsm_host::ReactionType::Reject => crate::ReactionType::Reject,
            wjsm_host::ReactionType::FinallyFulfill => crate::ReactionType::FinallyFulfill,
            wjsm_host::ReactionType::FinallyReject => crate::ReactionType::FinallyReject,
        };
        let internal_scope = scope.map(convert_captured_scope);
        self.caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(crate::Microtask::PromiseReaction {
                promise,
                reaction_type: internal_type,
                handler,
                argument,
                scope: internal_scope,
            });
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
                crate::NativeCallable::QueuingStrategySize { kind: internal_kind }
            }
        };
        crate::create_native_callable(self.caller.data(), internal)
    }

    fn insert_promise_entry(&mut self, handle: usize, entry: wjsm_host::PromiseEntry) {
        let internal = convert_promise_entry(entry);
        let mut table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::insert_promise_entry(&mut table, handle, internal);
    }

    fn mark_promise_handled(&mut self, promise: Value) {
        let handle = crate::raw_promise_handle(promise);
        let mut table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = crate::promise_entry_mut(&mut table, handle) {
            entry.handled = true;
            drop(table);
            crate::runtime_microtask::clear_pending_unhandled_rejection(self.caller.data(), handle);
        }
    }

    fn promise_state(&self, promise: Value) -> wjsm_host::PromiseState {
        let handle = crate::raw_promise_handle(promise);
        let table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match crate::promise_entry(&table, handle).map(|e| &e.state) {
            Some(crate::PromiseState::Pending) => wjsm_host::PromiseState::Pending,
            Some(crate::PromiseState::Fulfilled(v)) => wjsm_host::PromiseState::Fulfilled(*v),
            Some(crate::PromiseState::Rejected(r)) => wjsm_host::PromiseState::Rejected(*r),
            None => wjsm_host::PromiseState::Pending,
        }
    }
    fn promise_capture_scope(&self, promise: Value) -> Option<wjsm_host::CapturedScope> {
        let handle = crate::raw_promise_handle(promise);
        let table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::promise_entry(&table, handle)
            .and_then(|e| e.capture_scope)
            .map(convert_captured_scope_back)
    }

    fn gc_safepoint_poll(&mut self) {
        let Some(env) = self.env else {
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
        let Some(env) = self.env else {
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

    fn regexp_prototype(&mut self) -> Value {
        let Some(env) = self.env else {
            return value::encode_undefined();
        };
        if !value::is_object(self.caller.data().regexp_prototype) {
            crate::runtime_heap::ensure_regexp_prototype_initialized(self.caller, &env);
        }
        self.caller.data().regexp_prototype
    }

    fn resolve_handle_idx(&mut self, val: Value) -> Option<usize> {
        let Some(env) = self.env else {
            return None;
        };
        let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
        crate::runtime_values::resolve_handle_idx_with_env(self.caller, &env, handle_idx)
    }

    fn pending_exit_signal(&mut self) -> Option<i32> {
        crate::runtime_process::pending_process_exit_signal(self.caller.data()).map(|s| s.code)
    }

    fn value_to_number(&mut self, val: Value) -> Value {
        crate::runtime_host_helpers::value_to_number_or_exception(self.caller, val)
    }

    fn to_primitive(&mut self, val: Value, hint_number: bool) -> Value {
        let hint = if hint_number {
            crate::runtime_values::ToPrimitiveHint::Number
        } else {
            crate::runtime_values::ToPrimitiveHint::Default
        };
        crate::runtime_values::to_primitive_with_hint(self.caller, val, hint)
    }

    fn to_number(&mut self, val: Value) -> Value {
        crate::runtime_values::to_number(self.caller, val)
    }

    fn to_boolean(&mut self, val: Value) -> bool {
        crate::runtime_values::to_boolean(self.caller, val)
    }

    fn make_range_error(&mut self, msg: &str) -> Value {
        crate::runtime_host_helpers::make_range_error_exception(self.caller, msg)
    }

    fn create_error_object(&mut self, name: &str, message_arg: Value, options: Value) -> Value {
        crate::runtime_heap::create_error_object(self.caller, name, message_arg, options)
    }

    fn error_proto_to_string(&mut self, this_val: Value) -> Value {
        crate::runtime_heap::error_proto_to_string_impl(self.caller, this_val)
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

    fn store_bigint(&mut self, n: num_bigint::BigInt) -> Value {
        let mut table = self
            .caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(n);
        value::encode_bigint_handle(handle)
    }

    fn read_bigint(&mut self, val: Value) -> Option<num_bigint::BigInt> {
        if !value::is_bigint(val) {
            return None;
        }
        let handle = value::decode_bigint_handle(val) as usize;
        let table = self
            .caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    }

    fn create_symbol(
        &mut self,
        description: Option<String>,
        global_key: Option<String>,
    ) -> Value {
        let mut table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::SymbolEntry {
            description,
            global_key,
        });
        value::encode_symbol_handle(handle)
    }

    fn symbol_entry(&mut self, val: Value) -> Option<(Option<String>, Option<String>)> {
        if !value::is_symbol(val) {
            return None;
        }
        let handle = value::decode_symbol_handle(val) as usize;
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle)
            .map(|e| (e.description.clone(), e.global_key.clone()))
    }

    fn find_global_symbol(&mut self, key: &str) -> Option<Value> {
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (idx, entry) in table.iter().enumerate() {
            if entry.global_key.as_deref() == Some(key) {
                return Some(value::encode_symbol_handle(idx as u32));
            }
        }
        None
    }

    fn symbol_well_known(&mut self, id: i32) -> Value {
        if id < 0 {
            return value::encode_undefined();
        }
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if (id as usize) < table.len() {
            value::encode_symbol_handle(id as u32)
        } else {
            value::encode_undefined()
        }
    }

    fn install_well_known_symbols_on_symbol_constructor(&mut self, ctor: Value) {
        crate::symbol_well_known::install_well_known_symbols_on_symbol_constructor(
            self.caller,
            ctor,
        );
    }

    fn read_memory_string(&mut self, ptr: u32, len: Option<u32>) -> String {
        let Some(env) = self.env else {
            return String::new();
        };
        match len {
            Some(n) => {
                let data = env.memory.data(&*self.caller);
                let start = ptr as usize;
                let end = start.saturating_add(n as usize);
                if end > data.len() {
                    return String::new();
                }
                let bytes = &data[start..end];
                let bytes = if bytes.ends_with(&[0]) {
                    &bytes[..bytes.len() - 1]
                } else {
                    bytes
                };
                String::from_utf8_lossy(bytes).into_owned()
            }
            None => crate::runtime_render::read_string(self.caller, ptr).unwrap_or_default(),
        }
    }

    fn read_memory_string_bytes(&mut self, ptr: u32) -> Vec<u8> {
        crate::runtime_render::read_string_bytes(self.caller, ptr)
    }

    fn create_number_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::NumberPrimitiveMethod { method },
        )
    }

    fn create_bigint_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::BigIntPrimitiveMethod { method },
        )
    }

    fn create_global_builtin(&mut self, name: &str) -> Option<Value> {
        use crate::types::{NativeCallable, TypedArrayConstructorKind};
        let callable = match name {
            "Array" => NativeCallable::ArrayConstructor,
            "Object" => NativeCallable::ObjectConstructor,
            "Function" => NativeCallable::FunctionConstructor,
            "String" => NativeCallable::StringConstructor,
            "Boolean" => NativeCallable::BooleanConstructor,
            "Number" => NativeCallable::NumberConstructor,
            "Symbol" => NativeCallable::SymbolConstructor,
            "BigInt" => NativeCallable::BigIntConstructor,
            "RegExp" => NativeCallable::RegExpConstructor,
            "Error" => NativeCallable::ErrorConstructor,
            "TypeError" => NativeCallable::TypeErrorConstructor,
            "RangeError" => NativeCallable::RangeErrorConstructor,
            "SyntaxError" => NativeCallable::SyntaxErrorConstructor,
            "ReferenceError" => NativeCallable::ReferenceErrorConstructor,
            "URIError" => NativeCallable::URIErrorConstructor,
            "EvalError" => NativeCallable::EvalErrorConstructor,
            "AggregateError" => NativeCallable::AggregateErrorConstructor,
            "Map" => NativeCallable::MapConstructor,
            "Set" => NativeCallable::SetConstructor,
            "WeakMap" => NativeCallable::WeakMapConstructor,
            "WeakSet" => NativeCallable::WeakSetConstructor,
            "WeakRef" => NativeCallable::WeakRefConstructor,
            "FinalizationRegistry" => NativeCallable::FinalizationRegistryConstructor,
            "Date" => NativeCallable::DateConstructorGlobal,
            "Promise" => NativeCallable::PromiseConstructor,
            "Headers" => NativeCallable::HeadersConstructor,
            "Request" => NativeCallable::RequestConstructor,
            "Response" => NativeCallable::ResponseConstructor,
            "ReadableStream" => NativeCallable::ReadableStreamConstructor,
            "WritableStream" => NativeCallable::WritableStreamConstructor,
            "TransformStream" => NativeCallable::TransformStreamConstructor,
            "CountQueuingStrategy" => NativeCallable::CountQueuingStrategyConstructor,
            "ByteLengthQueuingStrategy" => NativeCallable::ByteLengthQueuingStrategyConstructor,
            "AbortController" => NativeCallable::AbortControllerConstructor,
            "ArrayBuffer" => NativeCallable::ArrayBufferConstructorGlobal,
            "SharedArrayBuffer" => NativeCallable::SharedArrayBufferConstructor,
            "Atomics" => NativeCallable::AtomicsGlobal,
            "DataView" => NativeCallable::DataViewConstructorGlobal,
            "Int8Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int8)
            }
            "Uint8Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8)
            }
            "Uint8ClampedArray" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8Clamped)
            }
            "Int16Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int16)
            }
            "Uint16Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint16)
            }
            "Int32Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int32)
            }
            "Uint32Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint32)
            }
            "Float32Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float32)
            }
            "Float64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float64)
            }
            "BigInt64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigInt64)
            }
            "BigUint64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigUint64)
            }
            "Proxy" => NativeCallable::ProxyConstructor,
            "gc" => NativeCallable::GcCollect,
            "agent_start" => NativeCallable::AgentStart,
            "agent_broadcast" => NativeCallable::AgentBroadcast,
            "agent_receive_broadcast" => NativeCallable::AgentReceiveBroadcast,
            "agent_get_report" => NativeCallable::AgentGetReport,
            "agent_report" => NativeCallable::AgentReport,
            "agent_sleep" => NativeCallable::AgentSleep,
            "agent_monotonic_now" => NativeCallable::AgentMonotonicNow,
            _ => return None,
        };
        Some(crate::create_native_callable(self.caller.data(), callable))
    }

    fn native_eval_function_param_count(&mut self, val: Value) -> Option<usize> {
        if !value::is_native_callable(val) {
            return None;
        }
        let idx = value::decode_native_callable_idx(val) as usize;
        let table = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match table.get(idx) {
            Some(crate::types::NativeCallable::EvalFunction(func)) => Some(func.params.len()),
            _ => None,
        }
    }

    fn is_process_hrtime_callable(&mut self, val: Value) -> bool {
        if !value::is_native_callable(val) {
            return false;
        }
        let idx = value::decode_native_callable_idx(val) as usize;
        let table = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        matches!(
            table.get(idx),
            Some(crate::types::NativeCallable::ProcessHrtime)
        )
    }

    fn create_process_hrtime_bigint(&mut self) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::ProcessHrtimeBigint,
        )
    }

    fn call_native_callable(&mut self, func: Value, this: Value, args: &[Value]) -> Value {
        crate::call_native_callable_with_args_from_caller(
            self.caller,
            func,
            this,
            args.to_vec(),
        )
        .unwrap_or_else(value::encode_undefined)
    }

    fn set_object_proto(&mut self, obj: Value, proto: Value) {
        let Some(env) = self.env else {
            return;
        };
        crate::runtime_heap::set_object_proto_header(self.caller, &env, obj, proto);
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

    fn name_id_to_property_key_value(&mut self, name_id: u32) -> Option<Value> {
        use wjsm_host::property_key::{decode_name_id, DecodedNameId};
        match decode_name_id(name_id) {
            DecodedNameId::MemoryString(index) => Some(value::encode_string_ptr(index)),
            DecodedNameId::Symbol(index) => Some(value::encode_symbol_handle(index)),
            DecodedNameId::RuntimeString(index) => {
                // 从 runtime property key 表取回 RuntimeString，再编码为 runtime string handle
                let key = crate::property_key::runtime_property_key_units(self.caller.data(), index)?;
                Some(crate::runtime_render::store_runtime_string(self.caller, key))
            }
        }
    }

    fn reflect_get_sync(&mut self, target: Value, prop: Value, receiver: Value) -> Value {
        let rt = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            rt.block_on(
                crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                    self.caller, target, prop, receiver,
                ),
            )
        })
    }

    fn invoke_getter_sync(&mut self, getter: Value, receiver: Value) -> Value {
        // 算法在 wjsm-builtins；此处仅桥接 ExecContext trait 方法。
        wjsm_builtins::get_method::invoke_getter(self, getter, receiver)
    }

    fn array_named_prop_get(&mut self, arr: Value, name_id: u32) -> Option<Value> {
        let Some(key) = crate::property_key::canonicalize_v2_name_id(self.caller, name_id) else {
            return None;
        };
        crate::array_named_props::ArrayNamedPropsStore::get_slot(self.caller, arr, key)
            .map(|slot| slot.value)
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
        let is_accessor =
            property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0;
        Some((
            property.value as i64,
            is_accessor,
            property.getter as i64,
        ))
    }

    fn string_utf16_len(&mut self, val: Value) -> Option<u32> {
        if !value::is_string(val) {
            return None;
        }
        Some(
            crate::runtime_values::get_string_value(self.caller, val).utf16_len() as u32,
        )
    }

    fn resolve_object_ptr(&mut self, val: Value) -> Option<usize> {
        crate::runtime_values::resolve_handle(self.caller, val)
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
            if let Some(val) =
                crate::runtime_values::read_object_property_by_name_id(self.caller, current, name_id)
            {
                return Some(val);
            }
            let env = self.env?;
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
                    i64::from_le_bytes(
                        data[slot_offset + 16..slot_offset + 24]
                            .try_into()
                            .unwrap(),
                    )
                };
                return Some(self.invoke_getter_sync(getter, receiver));
            }
            let env = self.env?;
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

    fn handle_index_of(&mut self, val: Value) -> Option<Handle> {
        Some(crate::runtime_values::handle_index_of(self.caller, val) as u32)
    }

    fn value_to_key_string(&mut self, val: Value) -> Result<String, Value> {
        crate::runtime_json::json_parse_to_string(self.caller, val)
    }

    fn primitive_symbol_get_property(&mut self, boxed: Value, name_id: u32) -> Value {
        crate::runtime_heap::primitive_symbol_get_property_impl(self.caller, boxed, name_id)
    }

    fn primitive_regexp_get_property(&mut self, boxed: Value, name_id: u32) -> Value {
        crate::runtime_regexp::primitive_regexp_get_property_impl(self.caller, boxed, name_id)
    }

    fn primitive_regexp_set_property(&mut self, boxed: Value, name_id: u32, val: Value) {
        crate::runtime_regexp::primitive_regexp_set_property_impl(
            self.caller, boxed, name_id, val,
        );
    }

    fn regexp_create(&mut self, pattern: String, flags: String) -> Value {
        crate::runtime_regexp::regexp_create_from_parts(self.caller, pattern, flags)
    }

    fn regexp_test(&mut self, regex: Value, str_val: Value) -> Value {
        crate::runtime_regexp::regexp_test_impl(self.caller, regex, str_val)
    }

    fn regexp_exec(&mut self, regex: Value, str_val: Value) -> Value {
        crate::runtime_regexp::regexp_exec_impl(self.caller, regex, str_val)
    }

    fn generator_prototype(&mut self) -> Value {
        self.caller.data().generator_prototype
    }

    fn create_generator_method(&mut self, generator: Value, kind: u8) -> Value {
        let kind = match kind {
            1 => crate::types::GeneratorCompletionType::Return,
            2 => crate::types::GeneratorCompletionType::Throw,
            _ => crate::types::GeneratorCompletionType::Next,
        };
        crate::runtime_generator::create_generator_method(self.caller.data(), generator, kind)
    }

    fn create_generator_identity(&mut self, generator: Value) -> Value {
        crate::runtime_generator::create_generator_identity(self.caller.data(), generator)
    }

    fn init_generator_entry(&mut self, generator: Value, continuation: Value) -> Value {
        crate::runtime_generator::init_generator_entry(
            self.caller.data(),
            generator,
            continuation,
        )
    }

    fn generator_next(&mut self, generator: Value, value: Value) -> Value {
        crate::runtime_generator::generator_yield_from_caller(self.caller, generator, value)
    }

    fn generator_return(&mut self, generator: Value, value: Value) -> Value {
        crate::runtime_generator::generator_return_from_caller(self.caller, generator, value)
    }

    fn generator_throw(&mut self, generator: Value, value: Value) -> Value {
        crate::runtime_generator::generator_throw_from_caller(self.caller, generator, value)
    }

    fn async_generator_prototype(&mut self) -> Value {
        self.caller.data().async_gen_prototype
    }

    fn create_async_generator_method(&mut self, generator: Value, kind: u8) -> Value {
        let kind = match kind {
            1 => crate::types::AsyncGeneratorCompletionType::Return,
            2 => crate::types::AsyncGeneratorCompletionType::Throw,
            _ => crate::types::AsyncGeneratorCompletionType::Next,
        };
        crate::runtime_async_fn::create_async_generator_method(
            self.caller.data(),
            generator,
            kind,
        )
    }

    fn create_async_generator_identity(&mut self, generator: Value) -> Value {
        crate::runtime_builtins::create_async_generator_identity(self.caller.data(), generator)
    }

    fn init_async_generator_entry(&mut self, generator: Value, continuation: Value) {
        use crate::types::{AsyncGeneratorEntry, AsyncGeneratorState};
        use std::collections::VecDeque;
        let handle = value::decode_object_handle(generator) as usize;
        let mut table = self
            .caller
            .data()
            .async_generator_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if table.len() <= handle {
            table.resize_with(handle + 1, || AsyncGeneratorEntry {
                state: AsyncGeneratorState::Completed,
                continuation: value::encode_undefined(),
                active_request: None,
                waiting_resume_promise: None,
                queue: VecDeque::new(),
            });
        }
        table[handle] = AsyncGeneratorEntry {
            state: AsyncGeneratorState::SuspendedStart,
            continuation,
            active_request: None,
            waiting_resume_promise: None,
            queue: VecDeque::new(),
        };
    }

    fn async_generator_next(&mut self, generator: Value, value: Value) -> Value {
        use crate::types::AsyncGeneratorState;

        let resume_promise = self.alloc_promise();
        let handle = value::decode_object_handle(generator) as usize;
        {
            let table = self
                .caller
                .data()
                .async_generator_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get(handle)
                && matches!(entry.state, AsyncGeneratorState::Completed)
            {
                drop(table);
                let result = self.alloc_iterator_result(value::encode_undefined(), true);
                self.resolve_promise(resume_promise, result);
                return resume_promise;
            }
        }
        let request_to_fulfill = {
            let mut table = self
                .caller
                .data()
                .async_generator_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get_mut(handle) else {
                return resume_promise;
            };
            if !matches!(entry.state, AsyncGeneratorState::Executing) {
                return resume_promise;
            }
            entry.state = AsyncGeneratorState::SuspendedYield;
            entry.waiting_resume_promise = Some(resume_promise);
            entry.active_request.take()
        };
        if let Some(request) = request_to_fulfill {
            let result = self.alloc_iterator_result(value, false);
            self.resolve_promise(request.promise, result);
        }
        crate::runtime_async_fn::pump_async_generator_from_caller(self.caller, generator);
        resume_promise
    }

    fn async_generator_return(&mut self, generator: Value, value: Value) -> Value {
        use crate::types::{
            AsyncGeneratorCompletionType, AsyncGeneratorHostAction, AsyncGeneratorState,
        };
        use std::collections::VecDeque;
        use wjsm_host::PromiseSettlement;

        let handle = value::decode_object_handle(generator) as usize;
        let action = {
            let mut table = self
                .caller
                .data()
                .async_generator_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get_mut(handle) else {
                return value::encode_undefined();
            };
            match entry.state {
                AsyncGeneratorState::SuspendedStart => {
                    entry.state = AsyncGeneratorState::Completed;
                    AsyncGeneratorHostAction::Immediate {
                        active: None,
                        queued: VecDeque::new(),
                    }
                }
                _ => {
                    entry.state = AsyncGeneratorState::Completed;
                    AsyncGeneratorHostAction::Immediate {
                        active: entry.active_request.take(),
                        queued: std::mem::take(&mut entry.queue),
                    }
                }
            }
        };
        let AsyncGeneratorHostAction::Immediate { active, queued } = action;
        if let Some(request) = active {
            let result = self.alloc_iterator_result(value, true);
            self.resolve_promise(request.promise, result);
        }
        for request in queued {
            match request.completion_type {
                AsyncGeneratorCompletionType::Throw => {
                    self.settle_promise(
                        request.promise,
                        PromiseSettlement::Reject(request.value),
                    );
                }
                AsyncGeneratorCompletionType::Return => {
                    let result = self.alloc_iterator_result(request.value, true);
                    self.resolve_promise(request.promise, result);
                }
                AsyncGeneratorCompletionType::Next => {
                    let result = self.alloc_iterator_result(value::encode_undefined(), true);
                    self.resolve_promise(request.promise, result);
                }
            }
        }
        value::encode_undefined()
    }

    fn async_generator_throw(&mut self, generator: Value, value: Value) -> Value {
        use crate::types::{
            AsyncGeneratorCompletionType, AsyncGeneratorHostAction, AsyncGeneratorState,
        };
        use std::collections::VecDeque;
        use wjsm_host::PromiseSettlement;

        let handle = value::decode_object_handle(generator) as usize;
        let action = {
            let mut table = self
                .caller
                .data()
                .async_generator_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get_mut(handle) else {
                return value::encode_undefined();
            };
            match entry.state {
                AsyncGeneratorState::SuspendedStart => {
                    entry.state = AsyncGeneratorState::Completed;
                    AsyncGeneratorHostAction::Immediate {
                        active: None,
                        queued: VecDeque::new(),
                    }
                }
                _ => {
                    entry.state = AsyncGeneratorState::Completed;
                    AsyncGeneratorHostAction::Immediate {
                        active: entry.active_request.take(),
                        queued: std::mem::take(&mut entry.queue),
                    }
                }
            }
        };
        let AsyncGeneratorHostAction::Immediate { active, queued } = action;
        if let Some(request) = active {
            self.settle_promise(request.promise, PromiseSettlement::Reject(value));
        }
        for request in queued {
            match request.completion_type {
                AsyncGeneratorCompletionType::Throw => {
                    self.settle_promise(
                        request.promise,
                        PromiseSettlement::Reject(request.value),
                    );
                }
                AsyncGeneratorCompletionType::Return => {
                    let result = self.alloc_iterator_result(request.value, true);
                    self.resolve_promise(request.promise, result);
                }
                AsyncGeneratorCompletionType::Next => {
                    let result = self.alloc_iterator_result(value::encode_undefined(), true);
                    self.resolve_promise(request.promise, result);
                }
            }
        }
        value::encode_undefined()
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

    fn alloc_continuation(
        &mut self,
        fn_table_idx: u32,
        outer_promise: Value,
        captured_var_count: usize,
    ) -> u32 {
        crate::runtime_async_fn::alloc_continuation_handle(
            self.caller.data(),
            fn_table_idx,
            outer_promise,
            captured_var_count,
        )
    }

    fn continuation_set_var(&mut self, cont_handle: u32, slot: usize, val: Value) {
        let mut table = self
            .caller
            .data()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(cont_handle as usize) {
            while entry.captured_vars.len() <= slot {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[slot] = val;
        }
    }

    fn continuation_get_var(&mut self, cont_handle: u32, slot: usize) -> Value {
        let table = self
            .caller
            .data()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(cont_handle as usize)
            .and_then(|e| e.captured_vars.get(slot).copied())
            .unwrap_or_else(value::encode_undefined)
    }

    fn enqueue_async_resume(
        &mut self,
        fn_table_idx: u32,
        continuation: Value,
        state: u32,
        resume_val: Value,
        completion: u8,
    ) {
        let mut queue = self
            .caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        queue.push_back(crate::Microtask::AsyncResume {
            fn_table_idx,
            continuation,
            state,
            resume_val,
            completion,
            scope: crate::runtime_async_hooks::capture_from_caller(self.caller),
        });
    }

    fn async_function_initial_call<'c>(
        &'c mut self,
        fn_table_idx: u32,
        continuation: Value,
        resume_val: Value,
    ) -> ExecFuture<'c, bool> {
        Box::pin(async move {
            let Some(env) = self.env else {
                return false;
            };
            let func_ref = env.func_table.get(&mut *self.caller, fn_table_idx as u64);
            let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
            let Some(func) = func else {
                return false;
            };
            let mut results = [wasmtime::Val::I64(0)];
            let _ = func
                .call_async(
                    &mut *self.caller,
                    &[
                        wasmtime::Val::I64(continuation),
                        wasmtime::Val::I64(resume_val),
                        wasmtime::Val::I32(0),
                        wasmtime::Val::I32(0),
                    ],
                    &mut results,
                )
                .await;
            let cont_handle = value::decode_object_handle(continuation) as usize;
            let outer_promise = {
                let c_table = self
                    .caller
                    .data()
                    .continuation_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                c_table.get(cont_handle).map(|e| e.outer_promise)
            };
            if let Some(outer_promise) = outer_promise
                && crate::is_promise_settled(self.caller.data(), outer_promise)
            {
                let mut c_table = self
                    .caller
                    .data()
                    .continuation_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = c_table.get_mut(cont_handle) {
                    entry.completed = true;
                }
            }
            true
        })
    }

    fn async_function_suspend(
        &mut self,
        continuation: Value,
        awaited_promise: Value,
        state: Value,
    ) {
        use crate::types::{PromiseReaction, PromiseState, ReactionType};
        let cont_handle = value::decode_object_handle(continuation) as usize;
        // WASM 传入的 state 是 raw i64 整数（非 NaN-box），与历史路径一致。
        let state_u = state as u32;
        let cont_fn_idx = {
            let mut c_table = self
                .caller
                .data()
                .continuation_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = c_table.get_mut(cont_handle) else {
                return;
            };
            while entry.captured_vars.len() < 4 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_f64(0.0);
            entry.fn_table_idx
        };

        let awaited_handle = value::decode_object_handle(awaited_promise) as usize;
        let mut p_table = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = crate::runtime_promises::promise_entry_mut(&mut p_table, awaited_handle)
        {
            entry.handled = true;
            crate::clear_pending_unhandled_rejection(self.caller.data(), awaited_handle);
            match &entry.state {
                PromiseState::Pending => {
                    entry.fulfill_reactions.push(PromiseReaction::new_async(
                        cont_fn_idx,
                        continuation,
                        ReactionType::Fulfill,
                        state_u,
                    ));
                    entry.reject_reactions.push(PromiseReaction::new_async(
                        cont_fn_idx,
                        continuation,
                        ReactionType::Reject,
                        state_u,
                    ));
                }
                PromiseState::Fulfilled(val) => {
                    let val = *val;
                    let reactions = vec![PromiseReaction::new_async(
                        cont_fn_idx,
                        continuation,
                        ReactionType::Fulfill,
                        state_u,
                    )];
                    drop(p_table);
                    crate::queue_promise_reactions(self.caller.data(), reactions, val, false, None);
                }
                PromiseState::Rejected(reason) => {
                    let reason = *reason;
                    let reactions = vec![PromiseReaction::new_async(
                        cont_fn_idx,
                        continuation,
                        ReactionType::Reject,
                        state_u,
                    )];
                    drop(p_table);
                    crate::queue_promise_reactions(
                        self.caller.data(),
                        reactions,
                        reason,
                        true,
                        None,
                    );
                }
            }
        } else {
            drop(p_table);
            self.enqueue_async_resume(
                cont_fn_idx,
                continuation,
                state_u,
                awaited_promise,
                0,
            );
        }
    }

    fn alloc_iterator_result(&mut self, value: Value, done: bool) -> Value {
        crate::runtime_async_fn::alloc_iterator_result_from_caller(self.caller, value, done)
    }

    fn object_get_prototype_of_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::proxy_or_target_get_prototype_of_impl_async(self.caller, obj).await
        })
    }

    fn object_is_extensible_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, bool> {
        Box::pin(async move {
            crate::proxy_or_target_is_extensible_impl_async(self.caller, obj).await
        })
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
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_values_async(self, obj).await
        })
    }

    fn object_get_own_property_names_async<'c>(&'c mut self, obj: Value) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            wjsm_builtins::proxy_reflect_async::object_get_own_property_names_async(self, obj)
                .await
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

    fn debug_break<'c>(&'c mut self, line: i32, col: i32, flags: i32) -> ExecFuture<'c, ()> {
        Box::pin(async move {
            crate::inspector::pause::debug_break_body(self.caller, line, col, flags).await
        })
    }

    // ── Phase 2 ──

    fn get_runtime_string(&mut self, val: Value) -> wjsm_host::RuntimeString {
        crate::get_string_value(self.caller, val)
    }

    fn store_runtime_string(&mut self, s: wjsm_host::RuntimeString) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s)
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
        let slot = access.get_property_slot(handle, key).ok().flatten()?;
        Some((
            slot.value as i64,
            slot.flags,
            slot.getter as i64,
            slot.setter as i64,
        ))
    }

    fn set_property_by_name_id(&mut self, handle: Handle, name_id: u32, val: Value) -> bool {
        let Some(access) = self.caller.data().heap_access_v2.clone() else {
            return false;
        };
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

    fn closure_func_idx(&mut self, idx: u32) -> Option<u32> {
        let closures = self
            .caller
            .data()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        closures.get(idx as usize).map(|e| e.func_idx)
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

    fn array_push(&mut self, arr: Value, val: Value) -> Value {
        let handle = value::decode_handle(arr);
        match crate::push_v2_array_element(self.caller, handle, val as u64) {
            Ok(length) => value::encode_f64(length as f64),
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 Array.prototype.push: {error}"),
                );
                value::encode_undefined()
            }
        }
    }

    fn array_push_hole(&mut self, arr: Value) -> Value {
        let handle = value::decode_handle(arr);
        match crate::push_v2_array_element(
            self.caller,
            handle,
            value::encode_array_hole() as u64,
        ) {
            Ok(length) => value::encode_f64(length as f64),
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 Array hole push: {error}"),
                );
                value::encode_undefined()
            }
        }
    }

    fn resolve_array(&mut self, arr: Value) -> bool {
        crate::resolve_array_ptr(self.caller, arr).is_some()
    }

    fn array_elem_at(&mut self, arr: Value, index: u32) -> Option<Value> {
        let handle = value::decode_handle(arr);
        if self
            .caller
            .data()
            .heap_access_v2()
            .resolve_handle(handle)
            .is_ok()
        {
            return self
                .caller
                .data()
                .heap_access_v2()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|v| v as i64)
                .and_then(|v| {
                    if value::is_array_hole(v) {
                        None
                    } else {
                        Some(v)
                    }
                });
        }
        let ptr = crate::resolve_array_ptr(self.caller, arr)?;
        crate::read_array_elem(self.caller, ptr, index)
    }

    fn array_write_hole(&mut self, arr: Value, index: u32) {
        self.array_write_elem(arr, index, value::encode_array_hole());
    }

    fn array_ensure_capacity(&mut self, arr: Value, needed: u32) -> bool {
        let handle = value::decode_handle(arr);
        crate::ensure_v2_array_capacity(self.caller, handle, needed).is_ok()
    }

    fn array_species_create(&mut self, exemplar: Value, length: u32) -> Value {
        crate::runtime_host_helpers::array_species_create(self.caller, exemplar, length)
    }

    fn array_species_create_async<'c>(
        &'c mut self,
        exemplar: Value,
        length: u32,
    ) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::runtime_host_helpers::array_species_create_async(self.caller, exemplar, length)
                .await
        })
    }

    fn typedarray_resolve(&mut self, this: Value) -> Option<TypedArrayView> {
        let (buf, off, len, esize, kind, shared) =
            crate::host_imports::typedarray_new_methods::ta_resolve(self.caller, this)?;
        Some(TypedArrayView {
            buffer_handle: buf as u32,
            byte_offset: off as u32,
            length: len,
            element_size: esize,
            element_kind: kind,
            is_shared: shared,
        })
    }

    fn typedarray_read_elem(&mut self, view: &TypedArrayView, index: u32) -> Option<Value> {
        if view.is_shared {
            crate::host_imports::typedarray_new_methods::sab_read(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
            )
        } else {
            crate::host_imports::typedarray_new_methods::ta_read(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
            )
        }
    }

    fn typedarray_write_elem(&mut self, view: &TypedArrayView, index: u32, val: Value) {
        if view.is_shared {
            let _ = crate::host_imports::typedarray_new_methods::sab_write(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
                val,
            );
        } else {
            let _ = crate::host_imports::typedarray_new_methods::ta_write(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
                val,
            );
        }
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

    fn map_table_create(&mut self) -> u32 {
        self.caller.data().alloc_map_entry()
    }

    fn set_table_create(&mut self) -> u32 {
        self.caller.data().alloc_set_entry()
    }

    fn map_set(&mut self, handle: u32, key: Value, val: Value) {
        let mut table = self
            .caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle as usize) else {
            return;
        };
        for i in 0..entry.keys.len() {
            if crate::same_value_zero(self.caller, entry.keys[i], key) {
                entry.values[i] = val;
                return;
            }
        }
        entry.keys.push(key);
        entry.values.push(val);
    }

    fn map_get(&mut self, handle: u32, key: Value) -> Option<Value> {
        let table = self
            .caller
            .data()
            .map_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(handle as usize)?;
        for i in 0..entry.keys.len() {
            if crate::same_value_zero(self.caller, entry.keys[i], key) {
                return Some(entry.values[i]);
            }
        }
        None
    }

    fn map_set_has(&mut self, handle: u32, key: Value, is_set: bool) -> bool {
        if is_set {
            let table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get(handle as usize) else {
                return false;
            };
            entry
                .values
                .iter()
                .any(|&v| crate::same_value_zero(self.caller, v, key))
        } else {
            self.map_get(handle, key).is_some()
        }
    }

    fn map_set_delete(&mut self, handle: u32, key: Value, is_set: bool) -> bool {
        if is_set {
            let mut table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get_mut(handle as usize) else {
                return false;
            };
            if let Some(pos) = entry
                .values
                .iter()
                .position(|&v| crate::same_value_zero(self.caller, v, key))
            {
                entry.values.remove(pos);
                return true;
            }
            false
        } else {
            let mut table = self
                .caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = table.get_mut(handle as usize) else {
                return false;
            };
            if let Some(pos) = entry
                .keys
                .iter()
                .position(|&k| crate::same_value_zero(self.caller, k, key))
            {
                entry.keys.remove(pos);
                entry.values.remove(pos);
                return true;
            }
            false
        }
    }

    fn map_set_clear(&mut self, handle: u32, is_set: bool) {
        if is_set {
            let mut table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.values.clear();
            }
        } else {
            let mut table = self
                .caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.keys.clear();
                entry.values.clear();
            }
        }
    }

    fn map_set_size(&mut self, handle: u32, is_set: bool) -> u32 {
        if is_set {
            let table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(handle as usize)
                .map(|e| e.values.len() as u32)
                .unwrap_or(0)
        } else {
            let table = self
                .caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(handle as usize)
                .map(|e| e.keys.len() as u32)
                .unwrap_or(0)
        }
    }

    fn set_add(&mut self, handle: u32, key: Value) {
        let mut table = self
            .caller
            .data()
            .set_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get_mut(handle as usize) else {
            return;
        };
        if !entry
            .values
            .iter()
            .any(|&v| crate::same_value_zero(self.caller, v, key))
        {
            entry.values.push(key);
        }
    }

    fn map_set_entries_snapshot(&mut self, handle: u32, is_set: bool) -> Vec<(Value, Value)> {
        if is_set {
            let table = self
                .caller
                .data()
                .set_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(handle as usize)
                .map(|e| e.values.iter().map(|&v| (v, v)).collect())
                .unwrap_or_default()
        } else {
            let table = self
                .caller
                .data()
                .map_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table
                .get(handle as usize)
                .map(|e| {
                    e.keys
                        .iter()
                        .zip(e.values.iter())
                        .map(|(&k, &v)| (k, v))
                        .collect()
                })
                .unwrap_or_default()
        }
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
            self.define_data_property(
                receiver,
                "__set_handle__",
                value::encode_f64(handle as f64),
            );
        } else {
            self.define_data_property(
                receiver,
                "__map_handle__",
                value::encode_f64(handle as f64),
            );
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
        table
            .get(handle as usize)
            .map(|e| e.data.len() as u32)
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

    fn dataview_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        byte_length: u32,
        is_shared: bool,
    ) -> Option<u32> {
        let mut table = self
            .caller
            .data()
            .dataview_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::DataViewEntry {
            buffer_handle,
            buffer_object,
            byte_offset,
            byte_length,
            is_shared,
        });
        Some(handle)
    }

    fn dataview_resolve(&mut self, handle: u32) -> Option<(u32, u32, u32, bool)> {
        let table = self
            .caller
            .data()
            .dataview_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let e = table.get(handle as usize)?;
        Some((e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared))
    }

    fn typedarray_table_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        length: u32,
        element_size: u8,
        element_kind: u8,
        is_shared: bool,
    ) -> u32 {
        let mut table = self
            .caller
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::TypedArrayEntry {
            buffer_handle,
            buffer_object,
            byte_offset,
            length,
            element_size,
            element_kind,
            is_shared,
        });
        handle
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

    fn create_collection_method(&mut self, kind: &str) -> Value {
        use crate::types::{
            MapSetMethodKind, NativeCallable, WeakMapMethodKind, WeakSetMethodKind,
        };
        let callable = match kind {
            "map_set" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::MapSet,
            },
            "map_get" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::MapGet,
            },
            "map_has" | "set_has" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Has,
            },
            "map_delete" | "set_delete" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Delete,
            },
            "map_clear" | "set_clear" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Clear,
            },
            "map_size" | "set_size" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Size,
            },
            "map_for_each" | "set_for_each" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::ForEach,
            },
            "map_keys" | "set_keys" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Keys,
            },
            "map_values" | "set_values" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Values,
            },
            "map_entries" | "set_entries" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Entries,
            },
            "set_add" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::SetAdd,
            },
            "weakmap_set" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Set,
            },
            "weakmap_get" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Get,
            },
            "weakmap_has" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Has,
            },
            "weakmap_delete" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Delete,
            },
            "weakset_add" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Add,
            },
            "weakset_has" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Has,
            },
            "weakset_delete" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Delete,
            },
            other => {
                if let Some(v) = self.create_global_builtin(other) {
                    return v;
                }
                return value::encode_undefined();
            }
        };
        crate::create_native_callable(self.caller.data(), callable)
    }

    fn create_date_method(&mut self, kind: &str) -> Value {
        use crate::types::DateMethodKind;
        let kind = match kind {
            "get_date" => DateMethodKind::GetDate,
            "get_day" => DateMethodKind::GetDay,
            "get_full_year" => DateMethodKind::GetFullYear,
            "get_hours" => DateMethodKind::GetHours,
            "get_milliseconds" => DateMethodKind::GetMilliseconds,
            "get_minutes" => DateMethodKind::GetMinutes,
            "get_month" => DateMethodKind::GetMonth,
            "get_seconds" => DateMethodKind::GetSeconds,
            "get_time" => DateMethodKind::GetTime,
            "get_timezone_offset" => DateMethodKind::GetTimezoneOffset,
            "get_utc_date" => DateMethodKind::GetUTCDate,
            "get_utc_day" => DateMethodKind::GetUTCDay,
            "get_utc_full_year" => DateMethodKind::GetUTCFullYear,
            "get_utc_hours" => DateMethodKind::GetUTCHours,
            "get_utc_milliseconds" => DateMethodKind::GetUTCMilliseconds,
            "get_utc_minutes" => DateMethodKind::GetUTCMinutes,
            "get_utc_month" => DateMethodKind::GetUTCMonth,
            "get_utc_seconds" => DateMethodKind::GetUTCSeconds,
            "set_date" => DateMethodKind::SetDate,
            "set_full_year" => DateMethodKind::SetFullYear,
            "set_hours" => DateMethodKind::SetHours,
            "set_milliseconds" => DateMethodKind::SetMilliseconds,
            "set_minutes" => DateMethodKind::SetMinutes,
            "set_month" => DateMethodKind::SetMonth,
            "set_seconds" => DateMethodKind::SetSeconds,
            "set_time" => DateMethodKind::SetTime,
            "set_utc_date" => DateMethodKind::SetUTCDate,
            "set_utc_full_year" => DateMethodKind::SetUTCFullYear,
            "set_utc_hours" => DateMethodKind::SetUTCHours,
            "set_utc_milliseconds" => DateMethodKind::SetUTCMilliseconds,
            "set_utc_minutes" => DateMethodKind::SetUTCMinutes,
            "set_utc_month" => DateMethodKind::SetUTCMonth,
            "set_utc_seconds" => DateMethodKind::SetUTCSeconds,
            "to_string" => DateMethodKind::ToString,
            "to_date_string" => DateMethodKind::ToDateString,
            "to_time_string" => DateMethodKind::ToTimeString,
            "to_locale_string" => DateMethodKind::ToLocaleString,
            "to_locale_date_string" => DateMethodKind::ToLocaleDateString,
            "to_locale_time_string" => DateMethodKind::ToLocaleTimeString,
            "to_iso_string" => DateMethodKind::ToISOString,
            "to_utc_string" => DateMethodKind::ToUTCString,
            "to_json" => DateMethodKind::ToJSON,
            "value_of" => DateMethodKind::ValueOf,
            _ => return value::encode_undefined(),
        };
        crate::runtime_builtins::create_date_method(self.caller.data(), kind)
    }

    fn date_read_ms(&mut self, this: Value) -> f64 {
        crate::runtime_date::read_date_ms(self.caller, this)
    }

    fn date_args_to_ms(&mut self, args: &[Value], is_utc: bool) -> f64 {
        crate::runtime_date::date_args_to_ms(args, is_utc)
    }

    fn date_now_ms(&mut self) -> f64 {
        chrono::Utc::now().timestamp_millis() as f64
    }

    fn new_target(&mut self) -> Value {
        self.caller
            .data()
            .new_target
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_date_prototype(&mut self, obj: Value) {
        if let Some(proto) = crate::runtime_heap::native_callable_date_prototype(
            self.caller,
            &crate::types::NativeCallable::DateConstructorGlobal,
        ) {
            self.set_object_proto(obj, proto);
        }
    }

    fn js_global_get(&mut self) -> Value {
        self.caller
            .data()
            .js_global_object
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn js_global_set(&mut self, obj: Value) {
        self.caller
            .data()
            .js_global_object
            .store(obj, std::sync::atomic::Ordering::Relaxed);
    }

    fn install_process_global(&mut self, global: Value) {
        let _ = crate::install_process_global_from_caller(self.caller, global);
    }

    fn install_node_web_globals(&mut self, global: Value) {
        let _ = crate::runtime_node_globals::install_node_web_globals_from_caller(
            self.caller,
            global,
        );
    }

    fn push_host_temp_roots(&mut self, vals: &[Value]) -> usize {
        self.caller.data().push_host_temp_roots(vals.iter().copied())
    }

    fn truncate_host_temp_roots(&mut self, len: usize) {
        self.caller.data().truncate_host_temp_roots(len);
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

    fn queue_microtask(&mut self, callback: Value) {
        let scope = {
            let mut hooks = self
                .caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            hooks.capture_for_scheduled_callback(0, true)
        };
        let mut queue = self
            .caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        queue.push_back(crate::Microtask::MicrotaskCallback { callback, scope });
    }

    fn register_module_namespace(&mut self, module_id: u32, namespace: Value) {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.register_static_namespace(module_id, namespace);
    }

    fn dynamic_import(&mut self, module_id: u32) -> Value {
        use std::sync::{Arc, Mutex};
        let promise = crate::alloc_promise(self.caller, crate::PromiseEntry::pending());
        let then_fn = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Fulfill,
        );
        let catch_fn = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Reject,
        );
        let _ = crate::define_host_data_property_from_caller(self.caller, promise, "then", then_fn);
        let _ = crate::define_host_data_property_from_caller(self.caller, promise, "catch", catch_fn);
        let namespace_obj = {
            let registry = self
                .caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.get_namespace_by_module_id(module_id)
        };
        match namespace_obj {
            Some(ns_obj) => {
                crate::resolve_promise_from_caller(self.caller, promise, ns_obj);
            }
            None => {
                let error_msg = format!("Cannot find module with id {}", module_id);
                let error_val = crate::runtime_error_value(self.caller.data(), error_msg);
                crate::settle_promise(
                    self.caller.data(),
                    promise,
                    crate::PromiseSettlement::Reject(error_val),
                );
            }
        }
        promise
    }

    fn alloc_null_proto_object(&mut self, capacity: u32) -> Value {
        let Some(env) = self.env else {
            return value::encode_undefined();
        };
        crate::runtime_heap::alloc_host_null_proto_object(self.caller, &env, capacity)
    }

    fn to_object(&mut self, val: Value) -> Value {
        crate::to_object(self.caller, val)
    }

    fn is_extensible(&mut self, obj: Value) -> bool {
        crate::is_extensible_impl(self.caller, obj)
    }

    fn prevent_extensions(&mut self, obj: Value) -> bool {
        crate::prevent_extensions_impl(self.caller, obj)
    }

    fn own_property_entries(&mut self, handle: Handle) -> Vec<(u32, u32)> {
        let access = self.caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            return access
                .own_property_slots(handle)
                .unwrap_or_default()
                .into_iter()
                .map(|(raw_name_id, flags)| {
                    // V2 堆槽位中的 name 是已 canonicalize 的 property key，
                    // 但 builtins 层面期望的是编译期 name_id（用于跨模块对照），
                    // 这里保留 raw 值，builtins 端需要时再 canonicalize。
                    (raw_name_id, flags)
                })
                .collect();
        }
        Vec::new()
    }

    fn update_property_flags(&mut self, handle: Handle, name_id: u32, flags: u32) -> bool {
        self.caller
            .data()
            .heap_access_v2()
            .update_property_flags(handle, name_id, flags)
            .is_ok()
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

    fn read_property_by_string_key(&mut self, obj: Value, key: &str) -> Value {
        crate::host_imports::read_property_by_string_key_impl(self.caller, obj, key)
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
        crate::host_imports::reflect_get_own_property_descriptor_impl(
            self.caller,
            target,
            prop,
        )
    }

    fn define_property_or_throw(&mut self, target: Value, key: Value, desc: Value) -> bool {
        crate::host_imports::object_define_property_or_throw(self.caller, target, key, desc)
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

    fn value_to_proto_handle(&mut self, proto: Value) -> u32 {
        crate::host_imports::proto_handle_from_value(self.caller, proto)
    }

    fn set_prototype_handle(&mut self, obj: Value, proto_handle: u32) -> bool {
        let handle = crate::handle_index_of(self.caller, obj) as u32;
        let access = self.caller.data().heap_access_v2();
        if access.resolve_handle(handle).is_ok() {
            return access.set_prototype(handle, proto_handle).is_ok();
        }
        let Some(env) = self.env else {
            return false;
        };
        crate::runtime_gc::heap_access::write_proto(self.caller, &env, handle, proto_handle)
            .is_some()
    }

    fn handle_is_live(&mut self, handle: Handle) -> bool {
        crate::obj_table_handle_live(self.caller, handle)
    }

    fn encode_handle_as_value(&mut self, handle: Handle) -> Value {
        crate::encode_handle_as_js_value(self.caller, handle)
            .unwrap_or_else(value::encode_undefined)
    }

    fn weak_target_handle(&mut self, target: Value) -> Option<Handle> {
        crate::weak_target_handle_index_of(self.caller, target)
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

    fn finalization_registry_table_push(
        &mut self,
        object_handle: Handle,
        callback: Value,
    ) -> u32 {
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

    fn finalization_registry_unregister_token(
        &mut self,
        registry_idx: u32,
        token: Value,
    ) -> bool {
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

    fn create_string_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::NativeCallable::StringPrimitiveMethod { method },
        )
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

    fn obj_proto_to_string(&mut self, receiver: Value) -> Value {
        crate::obj_proto_to_string_impl(self.caller, receiver)
    }

    fn regexp_is_global(&mut self, regex: Value) -> bool {
        if !value::is_regexp(regex) {
            return false;
        }
        let handle = value::decode_regexp_handle(regex);
        let table = self
            .caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .map(|e| e.flags.contains('g'))
            .unwrap_or(false)
    }

    fn value_to_display_string(&mut self, val: Value) -> String {
        crate::eval_to_string(self.caller, val)
    }

    fn call_symbol_method_async<'c>(
        &'c mut self,
        target: Value,
        symbol_idx: u32,
        this_arg: Value,
        args: &'c [Value],
    ) -> ExecFuture<'c, Option<Value>> {
        Box::pin(async move {
            crate::call_symbol_method_async(self.caller, target, symbol_idx, this_arg, args)
                .await
        })
    }

    fn regexp_collect_matches(
        &mut self,
        regex: Value,
        subject: &str,
        global: bool,
    ) -> Vec<RegExpMatchInfo> {
        if !value::is_regexp(regex) {
            return Vec::new();
        }
        let entry = {
            let table = self
                .caller
                .data()
                .regex_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match table.get(value::decode_regexp_handle(regex) as usize) {
                Some(e) => e.clone(),
                None => return Vec::new(),
            }
        };
        let map_match = |m: regress::Match| RegExpMatchInfo {
            start: m.start(),
            end: m.end(),
            captures: (0..m.captures.len() + 1).map(|i| m.group(i)).collect(),
            named: m
                .named_groups()
                .map(|(name, range)| (name.to_string(), range))
                .collect(),
        };
        if global {
            entry
                .compiled
                .find_iter(subject)
                .map(map_match)
                .collect()
        } else {
            entry
                .compiled
                .find(subject)
                .map(map_match)
                .into_iter()
                .collect()
        }
    }

    fn regexp_string_match_default(&mut self, receiver: Value, regexp: Value) -> Value {
        crate::regexp_string_match_default(self.caller, receiver, regexp)
    }

    fn regexp_string_search_default(&mut self, receiver: Value, regexp: Value) -> Value {
        crate::regexp_string_search_default(self.caller, receiver, regexp)
    }

    fn regexp_string_split_default(
        &mut self,
        receiver: Value,
        sep: Value,
        limit: Value,
    ) -> Value {
        crate::regexp_string_split_default(self.caller, receiver, sep, limit)
    }

    fn regexp_match_all_default(&mut self, this_val: Value, regexp: Value) -> Value {
        crate::regexp_match_all_default(self.caller, this_val, regexp)
    }

    fn has_in_property(&mut self, object: Value, prop: Value) -> Value {
        crate::host_imports::op_in_impl(self.caller, object, prop)
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

    fn read_data_property(&mut self, obj: Value, key: &str) -> Value {
        crate::read_host_data_property_v2(self.caller, obj, key)
            .unwrap_or_else(value::encode_undefined)
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

    fn iterator_from_fallback_async<'c>(
        &'c mut self,
        val: Value,
    ) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::host_imports::iterator_from_impl_async(self.caller, val).await
        })
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
            | crate::IteratorState::MapKeyIter { index, .. }
            | crate::IteratorState::MapValueIter { index, .. }
            | crate::IteratorState::SetValueIter { index, .. }
            | crate::IteratorState::SetEntryIter { index, .. }
            | crate::IteratorState::MapEntryIter { index, .. }
            | crate::IteratorState::HeadersKeyIter { index, .. }
            | crate::IteratorState::HeadersValueIter { index, .. }
            | crate::IteratorState::HeadersEntryIter { index, .. }
            | crate::IteratorState::IndexValueIter { index, .. }
            | crate::IteratorState::TypedArrayValueIter { index, .. }
            | crate::IteratorState::TypedArrayEntryIter { index, .. } => {
                *index += 1;
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
                if let Some(afs) = crate::host_imports::resolve_async_from_sync_afs_handle(
                    self.caller,
                    handle,
                    next,
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
                done,
                has_current,
                ..
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
                    self.caller, handle_idx,
                ))
            }
            // 其余侧表迭代器：委托原 impl 的同步路径（经 done_async 的 sync 分支）
            _ => {
                drop(iters);
                // 用原 done 逻辑中非 Object 路径：直接调用并 block 不安全；
                // 这些路径全是同步的，走 host helper。
                let v = {
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
                                *index as usize >= table[*map_handle as usize].keys.len()
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
                                *index as usize >= table[*set_handle as usize].values.len()
                            } else {
                                true
                            })
                        }
                        Some(crate::IteratorState::HeadersKeyIter {
                            index,
                            headers_handle,
                        })
                        | Some(crate::IteratorState::HeadersValueIter {
                            index,
                            headers_handle,
                        })
                        | Some(crate::IteratorState::HeadersEntryIter {
                            index,
                            headers_handle,
                        }) => {
                            let table = self
                                .caller
                                .data()
                                .headers_table
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            Some(if *headers_handle < table.len() as u32 {
                                *index as usize >= table[*headers_handle as usize].pairs.len()
                            } else {
                                true
                            })
                        }
                        Some(crate::IteratorState::IndexValueIter { index, values }) => {
                            Some(*index as usize >= values.len())
                        }
                        Some(crate::IteratorState::TypedArrayValueIter {
                            index, length, ..
                        })
                        | Some(crate::IteratorState::TypedArrayEntryIter {
                            index, length, ..
                        }) => Some(*index >= *length),
                        _ => Some(true),
                    }
                };
                v
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

    fn iterator_object_return_pair(
        &mut self,
        handle: Value,
    ) -> Option<(Value, Option<Value>)> {
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

    fn iterator_materialize_afs_next<'c>(&'c mut self, afs: u32) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::host_imports::materialize_async_from_sync_next(self.caller, afs).await
        })
    }

    fn iterator_current_value(&mut self, handle: Value) -> Value {
        crate::host_imports::iterator_value_impl(self.caller, handle)
    }

    fn promise_reject_exception(&mut self, exc: Value) -> Value {
        let promise =
            crate::alloc_promise_from_caller(self.caller, crate::PromiseEntry::pending());
        let reason = crate::exception_reason(self.caller, exc);
        crate::settle_promise(
            self.caller.data(),
            promise,
            crate::PromiseSettlement::Reject(reason),
        );
        promise
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

    fn is_promise_value(&mut self, val: Value) -> bool {
        crate::is_promise_value(self.caller.data(), val)
    }

    fn promise_settled(&mut self, promise: Value) -> Option<Result<Value, Value>> {
        let promise_handle = crate::raw_promise_handle(promise);
        let table_p = self
            .caller
            .data()
            .promise_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match crate::promise_entry(&table_p, promise_handle).map(|e| &e.state) {
            Some(crate::PromiseState::Fulfilled(v)) => Some(Ok(*v)),
            Some(crate::PromiseState::Rejected(r)) => Some(Err(*r)),
            _ => None,
        }
    }

    fn exception_reason(&mut self, exc: Value) -> Value {
        crate::exception_reason(self.caller, exc)
    }

    fn alloc_rejected_promise(&mut self, reason: Value) -> Value {
        let promise =
            crate::alloc_promise_from_caller(self.caller, crate::PromiseEntry::pending());
        crate::settle_promise(
            self.caller.data(),
            promise,
            crate::PromiseSettlement::Reject(reason),
        );
        promise
    }
}

// ── Promise 类型转换 helper（wjsm-host ↔ host-wasm 内部类型）──

fn convert_promise_entry(entry: wjsm_host::PromiseEntry) -> crate::PromiseEntry {
    let state = match entry.state {
        wjsm_host::PromiseState::Pending => crate::PromiseState::Pending,
        wjsm_host::PromiseState::Fulfilled(v) => crate::PromiseState::Fulfilled(v),
        wjsm_host::PromiseState::Rejected(r) => crate::PromiseState::Rejected(r),
    };
    crate::PromiseEntry {
        state,
        fulfill_reactions: entry.fulfill_reactions.into_iter().map(convert_promise_reaction).collect(),
        reject_reactions: entry.reject_reactions.into_iter().map(convert_promise_reaction).collect(),
        handled: entry.handled,
        constructor_resolver: entry.constructor_resolver,
        constructor_handle: entry.constructor_handle,
        is_promise: entry.is_promise,
        capture_scope: entry.capture_scope.map(convert_captured_scope),
    }
}

fn convert_promise_reaction(reaction: wjsm_host::PromiseReaction) -> crate::PromiseReaction {
    let rt = match reaction.reaction_type {
        wjsm_host::ReactionType::Fulfill => crate::ReactionType::Fulfill,
        wjsm_host::ReactionType::Reject => crate::ReactionType::Reject,
        wjsm_host::ReactionType::FinallyFulfill => crate::ReactionType::FinallyFulfill,
        wjsm_host::ReactionType::FinallyReject => crate::ReactionType::FinallyReject,
    };
    crate::PromiseReaction::new(reaction.handler, reaction.target_promise, rt)
}

fn convert_captured_scope(scope: wjsm_host::CapturedScope) -> crate::CapturedScope {
    crate::CapturedScope {
        async_id: scope.async_id,
        trigger_async_id: scope.trigger_async_id,
        resource: scope.resource,
        frame_id: scope.frame_id.map(crate::runtime_async_hooks::FrameId),
    }
}

fn convert_captured_scope_back(scope: crate::CapturedScope) -> wjsm_host::CapturedScope {
    wjsm_host::CapturedScope {
        async_id: scope.async_id,
        trigger_async_id: scope.trigger_async_id,
        resource: scope.resource,
        frame_id: scope.frame_id.map(|f| f.0),
    }
}



