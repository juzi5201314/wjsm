use super::*;

pub(crate) fn create_async_generator_method(
    state: &RuntimeState,
    generator: i64,
    kind: AsyncGeneratorCompletionType,
) -> i64 {
    create_native_callable(
        state,
        NativeCallable::AsyncGeneratorMethod { generator, kind },
    )
}

pub(crate) fn alloc_iterator_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    done: bool,
) -> i64 {
    let obj = { let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv"); alloc_host_object(caller, &_wjsm_env, 2) };
    let _ = define_host_data_property_from_caller(caller, obj, "value", val);
    let _ = define_host_data_property_from_caller(caller, obj, "done", value::encode_bool(done));
    obj
}

pub(crate) fn enqueue_async_resume_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let fn_table_idx = {
        let mut table = caller
            .data()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        let Some(entry) = table.get_mut(cont_handle) else {
            return;
        };
        while entry.captured_vars.len() < 2 {
            entry.captured_vars.push(value::encode_undefined());
        }
        entry.captured_vars[0] = value::encode_f64(state as f64);
        entry.captured_vars[1] = value::encode_bool(is_rejected);
        entry.fn_table_idx
    };
    caller
        .data()
        .microtask_queue
        .lock()
        .expect("microtask queue mutex")
        .push_back(Microtask::AsyncResume {
            fn_table_idx,
            continuation,
            state,
            resume_val,
            is_rejected,
        });
}

pub(crate) enum AsyncGeneratorPumpAction {
    Resume {
        continuation: i64,
        state: u32,
        value: i64,
        is_rejected: bool,
    },
    SettleResumePromise {
        promise: i64,
        value: i64,
        is_rejected: bool,
    },
    Fulfill {
        promise: i64,
        value: i64,
        done: bool,
    },
    Reject {
        promise: i64,
        reason: i64,
    },
}

pub(crate) fn pump_async_generator_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
) {
    let handle = value::decode_object_handle(generator) as usize;
    let action = {
        let mut table = caller
            .data()
            .async_generator_table
            .lock()
            .expect("async generator table mutex");
        let Some(entry) = table.get_mut(handle) else {
            return;
        };
        match entry.state {
            AsyncGeneratorState::Executing | AsyncGeneratorState::Completed => None,
            AsyncGeneratorState::SuspendedYield => {
                let Some(resume_promise) = entry.waiting_resume_promise.take() else {
                    return;
                };
                if entry.queue.is_empty() {
                    entry.waiting_resume_promise = Some(resume_promise);
                    None
                } else {
                    let request = entry.queue.pop_front().expect("non-empty queue");
                    entry.active_request = Some(request);
                    entry.state = AsyncGeneratorState::Executing;
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                is_rejected: false,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            Some(AsyncGeneratorPumpAction::SettleResumePromise {
                                promise: resume_promise,
                                value: request.value,
                                is_rejected: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
                            })
                        }
                    }
                }
            }
            AsyncGeneratorState::SuspendedStart => {
                if entry.queue.is_empty() {
                    None
                } else {
                    let request = entry.queue.pop_front().expect("non-empty queue");
                    match request.completion_type {
                        AsyncGeneratorCompletionType::Next => {
                            entry.active_request = Some(request);
                            entry.state = AsyncGeneratorState::Executing;
                            Some(AsyncGeneratorPumpAction::Resume {
                                continuation: entry.continuation,
                                state: 0,
                                value: request.value,
                                is_rejected: false,
                            })
                        }
                        AsyncGeneratorCompletionType::Return => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Fulfill {
                                promise: request.promise,
                                value: request.value,
                                done: true,
                            })
                        }
                        AsyncGeneratorCompletionType::Throw => {
                            entry.state = AsyncGeneratorState::Completed;
                            Some(AsyncGeneratorPumpAction::Reject {
                                promise: request.promise,
                                reason: request.value,
                            })
                        }
                    }
                }
            }
        }
    };
    match action {
        Some(AsyncGeneratorPumpAction::Resume {
            continuation,
            state,
            value,
            is_rejected,
        }) => enqueue_async_resume_from_caller(caller, continuation, state, value, is_rejected),
        Some(AsyncGeneratorPumpAction::SettleResumePromise {
            promise,
            value,
            is_rejected,
        }) => {
            if is_rejected {
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(value));
            } else {
                resolve_promise_from_caller(caller, promise, value);
            }
        }
        Some(AsyncGeneratorPumpAction::Fulfill {
            promise,
            value,
            done,
        }) => {
            let result = alloc_iterator_result_from_caller(caller, value, done);
            resolve_promise_from_caller(caller, promise, result);
        }
        Some(AsyncGeneratorPumpAction::Reject { promise, reason }) => {
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        }
        None => {}
    }
}

pub(crate) fn resume_async_function<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) {
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        if let Some(entry) = c_table.get_mut(cont_handle) {
            while entry.captured_vars.len() < 2 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_bool(is_rejected);
        }
    }
    let func_ref = env.func_table.get(&mut *ctx, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else { return };
    let mut results = [Val::I64(0)];
    let _ = func.call(
        &mut *ctx,
        &[
            Val::I64(continuation),
            Val::I64(resume_val),
            Val::I32(0),
            Val::I32(0),
        ],
        &mut results,
    );
    let cont_handle = value::decode_object_handle(continuation) as usize;
    let outer_promise = {
        let c_table = ctx
            .state_mut()
            .continuation_table
            .lock()
            .expect("continuation table mutex");
        c_table.get(cont_handle).map(|entry| entry.outer_promise)
    };
    if let Some(outer_promise) = outer_promise {
        let settled = is_promise_settled(ctx.state_mut(), outer_promise);
        if settled {
            let mut c_table = ctx
                .state_mut()
                .continuation_table
                .lock()
                .expect("continuation table mutex");
            if let Some(entry) = c_table.get_mut(cont_handle) {
                entry.completed = true;
            }
        }
    }
}
