// ExecContext 方法片段：promise
macro_rules! exec_ctx_promise {
    () => {
    fn alloc_promise(&mut self) -> Value {
        // 与 async_function_start 等历史路径一致：host_helpers::alloc_promise
        // （含 promise prototype + promise_table 插入）。
        crate::runtime_host_helpers::alloc_promise(
            self.caller,
            crate::types::PromiseEntry::pending(),
        )
    }
    fn alloc_dynamic_import_promise(&mut self) -> Value {
        use std::sync::{Arc, Mutex};

        let promise = crate::runtime_host_helpers::alloc_promise(
            self.caller,
            crate::types::PromiseEntry::pending(),
        );
        let then = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Fulfill,
        );
        let catch = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Reject,
        );
        let _ = crate::define_host_data_property_from_caller(self.caller, promise, "then", then);
        let _ = crate::define_host_data_property_from_caller(self.caller, promise, "catch", catch);
        promise
    }
    fn alloc_promise_with_entry(&mut self, entry: wjsm_host::PromiseEntry) -> Value {
        // 转换后端无关 PromiseEntry → host-wasm 内部 PromiseEntry
        let internal = convert_promise_entry(entry);
        crate::alloc_promise_from_caller(self.caller, internal)
    }
    fn alloc_aggregate_error(&mut self, errors: Value) -> Value {
        crate::runtime_host_helpers::alloc_aggregate_error(self.caller, errors)
    }
    fn alloc_all_settled_result(&mut self, status: &str, value_name: &str, value: Value) -> Value {
        crate::runtime_host_helpers::alloc_promise_all_settled_result(
            self.caller,
            status,
            value_name,
            value,
        )
    }
    fn settle_promise(&mut self, promise: Value, settlement: PromiseSettlement) {
        crate::runtime_promises::settle_promise(self.caller.data(), promise, settlement);
    }
    fn resolve_promise(&mut self, promise: Value, value: Value) {
        crate::runtime_promises::resolve_promise_from_caller(self.caller, promise, value);
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
                let value =
                    crate::runtime_render::store_runtime_string(self.caller, "PROMISE".to_string());
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
        let Some(env) = self.env() else {
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
        crate::create_combinator_context(self.caller.data(), result_promise, result_array) as u32
    }
    fn set_combinator_remaining(&self, context: u32, remaining: usize) {
        crate::set_combinator_remaining(self.caller.data(), context as usize, remaining);
    }
    fn increment_combinator_outstanding_settlements(&self, context: u32) {
        crate::increment_combinator_outstanding_settlements(self.caller.data(), context as usize);
    }
    fn mark_combinator_settled(&self, context: u32) {
        crate::mark_combinator_settled(self.caller.data(), context as usize);
    }
    fn try_recycle_combinator_context(&self, context: u32) {
        crate::try_recycle_combinator_context(self.caller.data(), context as usize);
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
    fn promise_reject_exception(&mut self, exc: Value) -> Value {
        let promise = crate::alloc_promise_from_caller(self.caller, crate::PromiseEntry::pending());
        let reason = crate::exception_reason(self.caller, exc);
        crate::settle_promise(
            self.caller.data(),
            promise,
            crate::PromiseSettlement::Reject(reason),
        );
        promise
    }
    fn alloc_rejected_promise(&mut self, reason: Value) -> Value {
        let promise = crate::alloc_promise_from_caller(self.caller, crate::PromiseEntry::pending());
        crate::settle_promise(
            self.caller.data(),
            promise,
            crate::PromiseSettlement::Reject(reason),
        );
        promise
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
    };
}
