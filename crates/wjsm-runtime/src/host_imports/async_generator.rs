use crate::*;

pub(crate) fn register_async_generator_imports(mut store: &mut Store<RuntimeState>) -> Vec<Extern> {
    let async_generator_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64| -> i64 {
            let generator = alloc_object(&mut caller, 4);
            // 设置 [[Prototype]] = AsyncGenerator.prototype
            let async_gen_proto = caller.data().async_gen_prototype;
            if !value::is_undefined(async_gen_proto) {
                if let Some(ptr) = resolve_handle(&mut caller, generator) {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .expect("memory");
                    let data = memory.data_mut(&mut caller);
                    data[ptr..ptr + 4]
                        .copy_from_slice(&value::decode_object_handle(async_gen_proto).to_le_bytes());
                }
            }
            if !value::is_object(generator) {
                return value::encode_undefined();
            }
            let next = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Next,
            );
            let ret = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Return,
            );
            let throw = create_async_generator_method(
                caller.data(),
                generator,
                AsyncGeneratorCompletionType::Throw,
            );
            let _ = define_host_data_property_from_caller(&mut caller, generator, "next", next);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "return", ret);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "throw", throw);
            let async_iter = create_async_generator_identity(caller.data(), generator);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                generator,
                "Symbol.asyncIterator",
                async_iter,
            );

            let handle = value::decode_object_handle(generator) as usize;
            let mut table = caller
                .data()
                .async_generator_table
                .lock()
                .expect("async generator table mutex");
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
            generator
        },
    );

    // ── Import 138: async_generator_next(i64, i64) -> i64 ───────────────────
    let async_generator_next_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let resume_promise = alloc_promise(&mut caller, PromiseEntry::pending());
            let handle = value::decode_object_handle(generator) as usize;
            {
                let table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                if let Some(entry) = table.get(handle) {
                    if matches!(entry.state, AsyncGeneratorState::Completed) {
                        drop(table);
                        let result =
                            alloc_iterator_result_from_caller(&mut caller, value::encode_undefined(), true);
                        resolve_promise_from_caller(&mut caller, resume_promise, result);
                        return resume_promise;
                    }
                }
            }
            let request_to_fulfill = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(handle) else {
                    return resume_promise;
                };
                entry.state = AsyncGeneratorState::SuspendedYield;
                entry.waiting_resume_promise = Some(resume_promise);
                entry.active_request.take()
            };
            if let Some(request) = request_to_fulfill {
                let result = alloc_iterator_result_from_caller(&mut caller, value, false);
                resolve_promise_from_caller(&mut caller, request.promise, result);
            }
            pump_async_generator_from_caller(&mut caller, generator);
            resume_promise
        },
    );

    // ── Import 139: async_generator_return(i64, i64) -> i64 ─────────────────
    let async_generator_return_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let handle = value::decode_object_handle(generator) as usize;
            let action = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
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
            match action {
                AsyncGeneratorHostAction::Immediate { active, queued } => {
                    if let Some(request) = active {
                        let result = alloc_iterator_result_from_caller(&mut caller, value, true);
                        resolve_promise_from_caller(&mut caller, request.promise, result);
                    }
                    for request in queued {
                        match request.completion_type {
                            AsyncGeneratorCompletionType::Throw => settle_promise(
                                caller.data(),
                                request.promise,
                                PromiseSettlement::Reject(request.value),
                            ),
                            _ => {
                                let result = alloc_iterator_result_from_caller(
                                    &mut caller,
                                    value::encode_undefined(),
                                    true,
                                );
                                resolve_promise_from_caller(&mut caller, request.promise, result);
                            }
                        }
                    }
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 140: async_generator_throw(i64, i64) -> i64 ──────────────────
    let async_generator_throw_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            let handle = value::decode_object_handle(generator) as usize;
            let action = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
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
            match action {
                AsyncGeneratorHostAction::Immediate { active, queued } => {
                    if let Some(request) = active {
                        settle_promise(
                            caller.data(),
                            request.promise,
                            PromiseSettlement::Reject(value),
                        );
                    }
                    for request in queued {
                        settle_promise(
                            caller.data(),
                            request.promise,
                            PromiseSettlement::Reject(value),
                        );
                    }
                }
            }
            value::encode_undefined()
        },
    );

    vec![
        async_generator_start_fn.into(),
        async_generator_next_fn.into(),
        async_generator_return_fn.into(),
        async_generator_throw_fn.into(),
    ]
}
