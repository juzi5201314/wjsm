// ExecContext 方法片段：async_gen
macro_rules! exec_ctx_async_gen {
    () => {
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
        crate::runtime_generator::init_generator_entry(self.caller.data(), generator, continuation)
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
        crate::runtime_async_fn::create_async_generator_method(self.caller.data(), generator, kind)
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
                    self.settle_promise(request.promise, PromiseSettlement::Reject(request.value));
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
                    self.settle_promise(request.promise, PromiseSettlement::Reject(request.value));
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
            let Some(env) = self.env() else {
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
        if let Some(entry) =
            crate::runtime_promises::promise_entry_mut(&mut p_table, awaited_handle)
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
            self.enqueue_async_resume(cont_fn_idx, continuation, state_u, awaited_promise, 0);
        }
    }
    fn debug_break<'c>(&'c mut self, line: i32, col: i32, flags: i32) -> ExecFuture<'c, ()> {
        Box::pin(async move {
            crate::inspector::pause::debug_break_body(self.caller, line, col, flags).await
        })
    }
    };
}
