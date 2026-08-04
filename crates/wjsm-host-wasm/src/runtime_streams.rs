use super::*;

pub(crate) fn clear_pipe_to<C: RuntimeStateAccess>(ctx: &mut C, readable_handle: u32) {
    let state = ctx.state_mut();
    let mut streams = state
        .readable_stream_table
        .inner
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(stream) = streams.get_mut(readable_handle as usize) {
        wjsm_builtins::streams::runtime::clear_pipe_to(stream);
    }
}

pub(crate) fn finish_writable_stream_close<C: RuntimeStateAccess>(
    ctx: &mut C,
    writable_handle: u32,
    close_promise: i64,
) {
    let closed_promises = {
        let state = ctx.state_mut();
        let mut streams = state
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let writers = state
            .writer_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        streams
            .get_mut(writable_handle as usize)
            .map(|stream| {
                wjsm_builtins::streams::runtime::finish_writable_close(
                    stream,
                    writers.iter(),
                    writable_handle,
                )
            })
            .unwrap_or_default()
    };
    for promise in closed_promises {
        settle_promise(
            ctx.state_mut(),
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
    settle_promise(
        ctx.state_mut(),
        close_promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
}

pub(crate) fn finish_pipe_to_write_with_env<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    readable_handle: u32,
    error: Option<i64>,
) -> i64 {
    let (promise, should_pump) = {
        let state = ctx.state_mut();
        let mut streams = state
            .readable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|failure| failure.into_inner());
        streams
            .get_mut(readable_handle as usize)
            .map(|stream| wjsm_builtins::streams::runtime::finish_pipe_to_write(stream, error))
            .unwrap_or((None, false))
    };
    if let (Some(promise), Some(error)) = (promise, error) {
        settle_promise(ctx.state_mut(), promise, PromiseSettlement::Reject(error));
    } else if should_pump {
        pump_readable_stream_pipe_to_with_env(ctx, env, readable_handle);
    }
    value::encode_undefined()
}

pub(crate) fn pump_readable_stream_pipe_to_with_env<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    readable_handle: u32,
) {
    let (action, controller_handle) = {
        let state = ctx.state_mut();
        let mut streams = state
            .readable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(stream) = streams.get_mut(readable_handle as usize) else {
            return;
        };
        let controller_handle = stream.controller_handle;
        let mut controllers = state
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let controller = controller_handle.and_then(|handle| controllers.get_mut(handle as usize));
        (
            wjsm_builtins::streams::runtime::next_pipe_to_action(stream, controller),
            controller_handle,
        )
    };
    match action {
        wjsm_builtins::streams::runtime::PipeToAction::Write { destination, chunk } => {
            let promise = alloc_promise_with_env(ctx, env, PromiseEntry::pending());
            attach_pipe_reactions(ctx, promise, readable_handle);
            dispatch_write(ctx, env, destination, chunk, promise);
        }
        wjsm_builtins::streams::runtime::PipeToAction::Close {
            destination,
            promise,
        } => {
            if !dispatch_close(ctx, env, destination, promise) {
                finish_writable_stream_close(ctx, destination, promise);
                clear_pipe_to(ctx, readable_handle);
            }
        }
        wjsm_builtins::streams::runtime::PipeToAction::Pull => {
            schedule_pull(ctx, env, controller_handle, readable_handle);
        }
        wjsm_builtins::streams::runtime::PipeToAction::Wait
        | wjsm_builtins::streams::runtime::PipeToAction::Done => {}
    }
}

fn attach_pipe_reactions<C: RuntimeStateAccess>(ctx: &mut C, promise: i64, readable_handle: u32) {
    let fulfill = create_native_callable(
        ctx.state_mut(),
        NativeCallable::ReadableStreamPipeToWriteFulfilled { readable_handle },
    );
    let reject = create_native_callable(
        ctx.state_mut(),
        NativeCallable::ReadableStreamPipeToWriteRejected { readable_handle },
    );
    let state = ctx.state_mut();
    let handle = raw_promise_handle(promise);
    let mut promises = state
        .promise_table
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = promise_entry_mut(&mut promises, handle) {
        entry.handled = true;
        entry.fulfill_reactions.push(PromiseReaction::new(
            fulfill,
            promise,
            ReactionType::Fulfill,
        ));
        entry
            .reject_reactions
            .push(PromiseReaction::new(reject, promise, ReactionType::Reject));
    }
}

fn schedule_pull<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    controller_handle: Option<u32>,
    readable_handle: u32,
) {
    let Some(controller_handle) = controller_handle else {
        return;
    };
    let pull = {
        let state = ctx.state_mut();
        let controllers = state
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        controllers
            .get(controller_handle as usize)
            .and_then(|controller| {
                controller
                    .pull_callback
                    .map(|callback| (callback, controller.underlying_source))
            })
    };
    let Some((callback, this_value)) = pull else {
        return;
    };
    let controller = create_controller_object_with_env(ctx, env, controller_handle);
    let state = ctx.state_mut();
    let mut queue = state
        .microtask_queue
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    queue.push_back(Microtask::ReadableStreamPull {
        callback,
        this_val: this_value.unwrap_or_else(value::encode_undefined),
        controller,
    });
    queue.push_back(Microtask::ReadableStreamPipeToPump { readable_handle });
}

fn dispatch_write<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    writable_handle: u32,
    chunk: i64,
    promise: i64,
) {
    let transform = {
        let state = ctx.state_mut();
        let transforms = state
            .transform_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        transforms
            .iter()
            .find(|entry| entry.writable_stream_handle == Some(writable_handle))
            .map(|entry| {
                (
                    entry.transform_callback,
                    entry.readable_controller_handle,
                    entry.transformer_this,
                )
            })
    };
    if let Some((callback, controller_handle, this_value)) = transform {
        dispatch_transform_write(
            ctx,
            env,
            callback,
            controller_handle,
            this_value,
            chunk,
            promise,
        );
        return;
    }
    let sink = {
        let state = ctx.state_mut();
        let streams = state
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let controller = streams
            .get(writable_handle as usize)
            .and_then(|stream| stream.controller_handle);
        drop(streams);
        let controllers = state
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        controller.and_then(|handle| {
            controllers
                .get(handle as usize)
                .map(|entry| (entry.write_callback, entry.underlying_source, handle))
        })
    };
    if let Some((Some(callback), this_value, controller_handle)) = sink {
        let controller = create_writable_controller_object(ctx, env, controller_handle);
        ctx.state_mut()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(Microtask::WritableStreamSinkWrite {
                callback,
                this_val: this_value.unwrap_or_else(value::encode_undefined),
                chunk,
                controller,
                write_promise: promise,
            });
    } else {
        settle_promise(
            ctx.state_mut(),
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
}

fn dispatch_transform_write<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    callback: Option<i64>,
    controller_handle: Option<u32>,
    this_value: Option<i64>,
    chunk: i64,
    promise: i64,
) {
    let Some(controller_handle) = controller_handle else {
        settle_promise(
            ctx.state_mut(),
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
        return;
    };
    if let Some(callback) = callback {
        let controller = create_controller_object_with_env(ctx, env, controller_handle);
        ctx.state_mut()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(Microtask::TransformStreamTransform {
                callback,
                this_val: this_value.unwrap_or_else(value::encode_undefined),
                chunk,
                controller,
                write_promise: promise,
            });
        return;
    }
    let stream_handle = {
        let state = ctx.state_mut();
        let mut controllers = state
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(controller) = controllers.get_mut(controller_handle as usize) else {
            return;
        };
        controller.chunk_queue.push_back(chunk);
        controller.stream_handle
    };
    let pending = {
        let state = ctx.state_mut();
        let mut readers = state
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        readers
            .iter_mut()
            .find(|reader| reader.stream_handle == stream_handle)
            .and_then(|reader| reader.pending_read_promise.take())
    };
    if let Some(read_promise) = pending {
        let result =
            crate::host_imports::build_reader_result_with_env(ctx, env, false, Some(chunk));
        settle_promise(
            ctx.state_mut(),
            read_promise,
            PromiseSettlement::Fulfill(result),
        );
    }
    settle_promise(
        ctx.state_mut(),
        promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
}

fn dispatch_close<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    writable_handle: u32,
    promise: i64,
) -> bool {
    let transform = {
        let state = ctx.state_mut();
        let transforms = state
            .transform_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        transforms
            .iter()
            .find(|entry| entry.writable_stream_handle == Some(writable_handle))
            .map(|entry| {
                (
                    entry.flush_callback,
                    entry.readable_controller_handle,
                    entry.readable_stream_handle,
                    entry.transformer_this,
                )
            })
    };
    if let Some((callback, Some(controller_handle), Some(readable_handle), this_value)) = transform
    {
        let controller = create_controller_object_with_env(ctx, env, controller_handle);
        ctx.state_mut()
            .microtask_queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(Microtask::TransformStreamFlush {
                callback,
                this_val: this_value.unwrap_or_else(value::encode_undefined),
                controller,
                writable_stream_handle: writable_handle,
                readable_stream_handle: readable_handle,
                readable_controller_handle: controller_handle,
                close_promise: promise,
            });
        return true;
    }
    let sink = {
        let state = ctx.state_mut();
        let streams = state
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let controller = streams
            .get(writable_handle as usize)
            .and_then(|stream| stream.controller_handle);
        drop(streams);
        let controllers = state
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        controller.and_then(|handle| {
            controllers
                .get(handle as usize)
                .map(|entry| (entry.sink_close_callback, entry.underlying_source, handle))
        })
    };
    let Some((callback, this_value, controller_handle)) = sink else {
        return false;
    };
    let controller = create_writable_controller_object(ctx, env, controller_handle);
    ctx.state_mut()
        .microtask_queue
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push_back(Microtask::WritableStreamSinkClose {
            callback,
            this_val: this_value.unwrap_or_else(value::encode_undefined),
            controller,
            writable_stream_handle: writable_handle,
            close_promise: promise,
        });
    true
}

fn create_controller_object_with_env<C>(ctx: &mut C, env: &WasmEnv, handle: u32) -> i64
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    let object = crate::runtime_heap::alloc_host_object(ctx, env, 7);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        object,
        "__controller_handle__",
        value::encode_f64(handle as f64),
    );
    for (name, kind) in [
        (
            "enqueue",
            ReadableStreamDefaultControllerMethodKind::Enqueue,
        ),
        ("close", ReadableStreamDefaultControllerMethodKind::Close),
        ("error", ReadableStreamDefaultControllerMethodKind::Error),
    ] {
        let callable = create_native_callable(
            ctx.state_mut(),
            NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind },
        );
        let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
            ctx, env, object, name, callable,
        );
    }
    for (name, kind) in [
        (
            "desiredSize",
            ReadableStreamDefaultControllerMethodKind::GetDesiredSize,
        ),
        (
            "byobRequest",
            ReadableStreamDefaultControllerMethodKind::GetByobRequest,
        ),
    ] {
        let getter = create_native_callable(
            ctx.state_mut(),
            NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind },
        );
        let _ = crate::runtime_host_helpers::define_host_accessor_property_with_env(
            ctx,
            env,
            object,
            name,
            getter,
            value::encode_undefined(),
        );
    }
    object
}

fn create_writable_controller_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle: u32,
) -> i64 {
    let object = crate::runtime_heap::alloc_host_object(ctx, env, 3);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        object,
        "__controller_handle__",
        value::encode_f64(handle as f64),
    );
    object
}
