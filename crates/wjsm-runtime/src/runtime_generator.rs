use super::*;

pub(crate) fn create_generator_method(
    state: &RuntimeState,
    generator: i64,
    kind: GeneratorCompletionType,
) -> i64 {
    create_native_callable(state, NativeCallable::GeneratorMethod { generator, kind })
}

pub(crate) fn create_generator_identity(state: &RuntimeState, generator: i64) -> i64 {
    create_native_callable(state, NativeCallable::GeneratorIdentity { generator })
}

pub(crate) fn ensure_generator_slot(state: &RuntimeState, handle: usize) {
    let mut table = state
        .generator_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if table.len() <= handle {
        table.resize_with(handle + 1, || GeneratorEntry {
            state: GeneratorState::Completed,
            continuation: value::encode_undefined(),
        });
    }
}

pub(crate) fn init_generator_entry(state: &RuntimeState, generator: i64, continuation: i64) -> i64 {
    let handle = value::decode_object_handle(generator) as usize;
    ensure_generator_slot(state, handle);
    let mut table = state
        .generator_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table[handle] = GeneratorEntry {
        state: GeneratorState::SuspendedStart,
        continuation,
    };
    generator
}

pub(crate) fn generator_completed_result(
    caller: &mut Caller<'_, RuntimeState>,
    kind: GeneratorCompletionType,
    value: i64,
) -> i64 {
    match kind {
        GeneratorCompletionType::Next => {
            alloc_iterator_result_from_caller(caller, value::encode_undefined(), true)
        }
        GeneratorCompletionType::Return => alloc_iterator_result_from_caller(caller, value, true),
        GeneratorCompletionType::Throw => make_exception_value(caller, value),
    }
}

pub(crate) fn generator_already_executing(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    make_type_error_exception(caller, "Generator is already executing")
}

pub(crate) fn set_generator_state(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    state: GeneratorState,
) {
    let handle = value::decode_object_handle(generator) as usize;
    let mut table = caller
        .data()
        .generator_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(handle) {
        entry.state = state;
    }
}

pub(crate) fn generator_state(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
) -> Option<GeneratorState> {
    let handle = value::decode_object_handle(generator) as usize;
    let table = caller
        .data()
        .generator_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table.get(handle).map(|entry| entry.state)
}

pub(crate) async fn resume_generator_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    continuation: i64,
    state: u32,
    resume_val: i64,
    completion: u8,
) -> i64 {
    let mut effective_resume_val = resume_val;
    {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let mut c_table = caller
            .data()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = c_table.get_mut(cont_handle) else {
            return value::encode_undefined();
        };
        if state != u32::MAX {
            entry.captured_vars[0] = value::encode_f64(state as f64);
        }
        if completion == 2 {
            entry.pending_return = Some(resume_val);
            entry.captured_vars[1] = value::encode_f64(2.0);
        } else if let Some(pending) = entry.pending_return.take() {
            effective_resume_val = pending;
            entry.captured_vars[1] = value::encode_f64(2.0);
        } else {
            entry.captured_vars[1] = value::encode_f64(completion as f64);
        }
    }

    let env = WasmEnv::from_caller(&mut *caller).expect("WasmEnv in generator resume");
    let fn_table_idx = {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        let table = caller
            .data()
            .continuation_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(cont_handle).map(|entry| entry.fn_table_idx)
    };
    let Some(fn_table_idx) = fn_table_idx else {
        return value::encode_undefined();
    };
    let func_ref = env.func_table.get(&mut *caller, fn_table_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        return value::encode_undefined();
    };
    let mut results = [Val::I64(0)];
    let call_result = func
        .call_async(
            &mut *caller,
            &[
                Val::I64(continuation),
                Val::I64(effective_resume_val),
                Val::I32(0),
                Val::I32(0),
            ],
            &mut results,
        )
        .await;
    match call_result {
        Ok(()) => {
            let result = match results[0] {
                Val::I64(v) => v,
                _ => value::encode_undefined(),
            };
            if value::is_exception(result) {
                set_generator_state(caller, generator, GeneratorState::Completed);
            }
            result
        }
        Err(trap) => {
            let msg = format!("WASM trap: {:?}", trap);
            set_runtime_error(caller.data(), msg);
            set_generator_state(caller, generator, GeneratorState::Completed);
            value::encode_undefined()
        }
    }
}

pub(crate) async fn call_generator_method_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    kind: GeneratorCompletionType,
    argument: i64,
) -> i64 {
    let Some(state) = generator_state(caller, generator) else {
        return value::encode_undefined();
    };
    match state {
        GeneratorState::Executing => generator_already_executing(caller),
        GeneratorState::Completed => generator_completed_result(caller, kind, argument),
        GeneratorState::SuspendedStart => match kind {
            GeneratorCompletionType::Next => {
                let handle = value::decode_object_handle(generator) as usize;
                let continuation = {
                    let table = caller
                        .data()
                        .generator_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let Some(entry) = table.get(handle) else {
                        return value::encode_undefined();
                    };
                    entry.continuation
                };
                set_generator_state(caller, generator, GeneratorState::Executing);
                resume_generator_from_caller_async(
                    caller,
                    generator,
                    continuation,
                    0,
                    value::encode_undefined(),
                    0,
                )
                .await
            }
            GeneratorCompletionType::Return => {
                set_generator_state(caller, generator, GeneratorState::Completed);
                alloc_iterator_result_from_caller(caller, argument, true)
            }
            GeneratorCompletionType::Throw => {
                set_generator_state(caller, generator, GeneratorState::Completed);
                make_exception_value(caller, argument)
            }
        },
        GeneratorState::SuspendedYield => {
            let handle = value::decode_object_handle(generator) as usize;
            let continuation = {
                let table = caller
                    .data()
                    .generator_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(entry) = table.get(handle) else {
                    return value::encode_undefined();
                };
                entry.continuation
            };
            set_generator_state(caller, generator, GeneratorState::Executing);
            let completion = match kind {
                GeneratorCompletionType::Next => 0,
                GeneratorCompletionType::Throw => 1,
                GeneratorCompletionType::Return => 2,
            };
            resume_generator_from_caller_async(
                caller,
                generator,
                continuation,
                u32::MAX,
                argument,
                completion,
            )
            .await
        }
    }
}

pub(crate) fn generator_yield_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    value: i64,
) -> i64 {
    set_generator_state(caller, generator, GeneratorState::SuspendedYield);
    alloc_iterator_result_from_caller(caller, value, false)
}

pub(crate) fn generator_return_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    value: i64,
) -> i64 {
    set_generator_state(caller, generator, GeneratorState::Completed);
    alloc_iterator_result_from_caller(caller, value, true)
}

pub(crate) fn generator_throw_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    generator: i64,
    value: i64,
) -> i64 {
    set_generator_state(caller, generator, GeneratorState::Completed);
    make_exception_value(caller, value)
}
