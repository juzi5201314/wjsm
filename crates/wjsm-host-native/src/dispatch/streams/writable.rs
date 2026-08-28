use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{
    ObjectKind, StreamCallable, StreamProperty, StreamReaction, StreamTask, TransformState,
    WritableControllerMethod, WritableControllerState, WritableMethod, WritableState,
    WritableStatus, WriterMethod, WriterState, callable_property, new_promise, read_named,
    register_object, resolved, type_error,
};
use crate::NativeAgentState;

pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let sink = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let high_water_mark = if value::is_js_object(strategy) {
        let raw = read_named(ctx, state, strategy, "highWaterMark");
        super::super::runtime::to_number(state, raw).unwrap_or(1.0)
    } else {
        1.0
    };
    if !high_water_mark.is_finite() || high_water_mark < 0.0 {
        return type_error(
            ctx,
            state,
            "highWaterMark must be a non-negative finite number",
        );
    }
    let write = value::is_js_object(sink)
        .then(|| callable_property(ctx, state, sink, "write"))
        .flatten();
    let close = value::is_js_object(sink)
        .then(|| callable_property(ctx, state, sink, "close"))
        .flatten();
    let abort = value::is_js_object(sink)
        .then(|| callable_property(ctx, state, sink, "abort"))
        .flatten();
    let Some((stream, controller)) = create_writable(state, sink, write, close, abort, None) else {
        return super::super::fail_dispatch(ctx);
    };
    if value::is_js_object(sink)
        && let Some(start) = callable_property(ctx, state, sink, "start")
    {
        let result = state
            .invoke_callable(ctx, start, sink, &[controller])
            .unwrap_or_else(|| super::super::fail_dispatch(ctx));
        if value::is_exception(result) {
            return result;
        }
    }
    stream
}

pub(super) fn construct_transform(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let transformer = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let transform = value::is_js_object(transformer)
        .then(|| callable_property(ctx, state, transformer, "transform"))
        .flatten();
    let flush = value::is_js_object(transformer)
        .then(|| callable_property(ctx, state, transformer, "flush"))
        .flatten();
    let Some((readable, controller_object)) = super::readable::create_stream(
        state,
        1.0,
        false,
        value::encode_undefined(),
        None,
        None,
        false,
    ) else {
        return super::super::fail_dispatch(ctx);
    };
    let Some(ObjectKind::Readable(readable_handle)) = state
        .streams
        .objects
        .get(&value::decode_handle(readable))
        .copied()
    else {
        return super::super::fail_dispatch(ctx);
    };
    let controller = state.streams.readables[readable_handle as usize].controller;
    let transform_handle = state.streams.transforms.len() as u32;
    let Some((writable, _)) =
        create_writable(state, transformer, None, None, None, Some(transform_handle))
    else {
        return super::super::fail_dispatch(ctx);
    };
    let Some(ObjectKind::Writable(writable_handle)) = state
        .streams
        .objects
        .get(&value::decode_handle(writable))
        .copied()
    else {
        return super::super::fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 3, false) else {
        return super::super::fail_dispatch(ctx);
    };
    if state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::TransformStreamConstructor)
        .is_err()
    {
        return super::super::fail_dispatch(ctx);
    }
    state.streams.transforms.push(TransformState {
        readable: readable_handle,
        writable: writable_handle,
        controller,
        transformer,
        transform,
        flush,
    });
    register_object(state, object, ObjectKind::Transform(transform_handle));
    if value::is_js_object(transformer)
        && let Some(start) = callable_property(ctx, state, transformer, "start")
    {
        let result = state
            .invoke_callable(ctx, start, transformer, &[controller_object])
            .unwrap_or_else(|| super::super::fail_dispatch(ctx));
        if value::is_exception(result) {
            return result;
        }
    }
    object
}

fn create_writable(
    state: &mut NativeAgentState,
    sink: i64,
    write: Option<i64>,
    close: Option<i64>,
    abort: Option<i64>,
    transform: Option<u32>,
) -> Option<(i64, i64)> {
    let object = state.allocate_object(5, false).ok()?;
    state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::WritableStreamConstructor)
        .ok()?;
    let controller_object = state.allocate_object(3, false).ok()?;
    let signal = state.allocate_object(1, false).ok()?;
    super::super::modules::set_named_property(state, signal, "aborted", value::encode_bool(false))
        .ok()?;
    let stream = state.streams.writables.len() as u32;
    let controller = state.streams.writable_controllers.len() as u32;
    state.streams.writables.push(WritableState {
        object,
        controller,
        status: WritableStatus::Writable,
        locked: false,
        transform,
        pipe_source: None,
    });
    state
        .streams
        .writable_controllers
        .push(WritableControllerState {
            object: controller_object,
            stream,
            sink,
            write,
            close,
            abort,
            signal,
        });
    register_object(state, object, ObjectKind::Writable(stream));
    register_object(
        state,
        controller_object,
        ObjectKind::WritableController(controller),
    );
    Some((object, controller_object))
}

pub(super) fn writable_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let stream = state.streams.writables.get(handle as usize)?;
    let method = match key {
        "abort" => WritableMethod::Abort,
        "close" => WritableMethod::Close,
        "getWriter" => WritableMethod::GetWriter,
        "locked" => {
            return Some(StreamProperty::Value(value::encode_bool(stream.locked)));
        }
        _ => return None,
    };
    Some(StreamProperty::Callable(StreamCallable::Writable(
        handle, method,
    )))
}

pub(super) fn writer_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let writer = state.streams.writers.get(handle as usize)?;
    match key {
        "abort" => Some(StreamProperty::Callable(StreamCallable::Writer(
            handle,
            WriterMethod::Abort,
        ))),
        "close" => Some(StreamProperty::Callable(StreamCallable::Writer(
            handle,
            WriterMethod::Close,
        ))),
        "closed" => Some(StreamProperty::Value(value::encode_object_handle(
            writer.closed_promise,
        ))),
        "desiredSize" => Some(StreamProperty::Value(value::encode_f64(1.0))),
        "ready" => Some(StreamProperty::Value(value::encode_object_handle(
            writer.ready_promise,
        ))),
        "releaseLock" => Some(StreamProperty::Callable(StreamCallable::Writer(
            handle,
            WriterMethod::ReleaseLock,
        ))),
        "write" => Some(StreamProperty::Callable(StreamCallable::Writer(
            handle,
            WriterMethod::Write,
        ))),
        _ => None,
    }
}

pub(super) fn controller_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let controller = state.streams.writable_controllers.get(handle as usize)?;
    match key {
        "error" => Some(StreamProperty::Callable(
            StreamCallable::WritableController(handle, WritableControllerMethod::Error),
        )),
        "signal" => Some(StreamProperty::Value(controller.signal)),
        _ => None,
    }
}

pub(super) fn call_writable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    method: WritableMethod,
    args: &[i64],
) -> i64 {
    match method {
        WritableMethod::Abort => abort(ctx, state, stream, args),
        WritableMethod::Close => start_close(ctx, state, stream),
        WritableMethod::GetWriter => get_writer(ctx, state, stream),
    }
}

pub(super) fn call_writer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    writer: u32,
    method: WriterMethod,
    args: &[i64],
) -> i64 {
    let Some(stream) = state
        .streams
        .writers
        .get(writer as usize)
        .map(|writer| writer.stream)
    else {
        return super::super::fail_dispatch(ctx);
    };
    match method {
        WriterMethod::Abort => abort(ctx, state, stream, args),
        WriterMethod::Close => start_close(ctx, state, stream),
        WriterMethod::ReleaseLock => {
            release_writer(state, writer);
            value::encode_undefined()
        }
        WriterMethod::Write => {
            let chunk = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            start_write(ctx, state, stream, chunk)
        }
    }
}

pub(super) fn call_controller(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    controller: u32,
    method: WritableControllerMethod,
    args: &[i64],
) -> i64 {
    let Some(stream) = state
        .streams
        .writable_controllers
        .get(controller as usize)
        .map(|controller| controller.stream)
    else {
        return super::super::fail_dispatch(ctx);
    };
    match method {
        WritableControllerMethod::Error => {
            let reason = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            error_writable(state, stream, reason);
            value::encode_undefined()
        }
    }
}

fn get_writer(ctx: &mut NativeVmContext, state: &mut NativeAgentState, stream: u32) -> i64 {
    let Some(locked) = state
        .streams
        .writables
        .get(stream as usize)
        .map(|stream| stream.locked)
    else {
        return super::super::fail_dispatch(ctx);
    };
    if locked {
        return type_error(ctx, state, "WritableStream is already locked");
    }
    let Some((_, closed_promise)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    let Some((ready, ready_promise)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    super::super::promise::settle_promise(state, ready_promise, value::encode_undefined(), false);
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 8, false) else {
        return super::super::fail_dispatch(ctx);
    };
    let writer = state.streams.writers.len() as u32;
    state.streams.writers.push(WriterState {
        stream,
        closed_promise,
        ready_promise: value::decode_handle(ready),
    });
    state.streams.writables[stream as usize].locked = true;
    register_object(state, object, ObjectKind::Writer(writer));
    object
}

fn release_writer(state: &mut NativeAgentState, writer: u32) {
    if let Some(stream) = state
        .streams
        .writers
        .get(writer as usize)
        .map(|writer| writer.stream)
        && let Some(stream) = state.streams.writables.get_mut(stream as usize)
    {
        stream.locked = false;
    }
}

fn start_write(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    chunk: i64,
) -> i64 {
    let Some((promise, promise_handle)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    if state
        .streams
        .writables
        .get(stream as usize)
        .is_none_or(|stream| stream.status != WritableStatus::Writable)
    {
        let reason = type_error(ctx, state, "WritableStream is not writable");
        super::super::promise::settle_promise(
            state,
            promise_handle,
            state.exception_value(reason).unwrap_or(reason),
            true,
        );
        return promise;
    }
    super::super::promise::enqueue_stream_task(
        state,
        StreamTask::Write {
            stream,
            chunk,
            promise: promise_handle,
        },
    );
    promise
}

pub(super) fn start_pipe_write(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    chunk: i64,
) -> i64 {
    start_write(ctx, state, stream, chunk)
}

pub(super) fn run_write(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    chunk: i64,
    promise: u32,
) -> i64 {
    let Some((controller, transform)) = state
        .streams
        .writables
        .get(stream as usize)
        .map(|stream| (stream.controller, stream.transform))
    else {
        return super::super::fail_dispatch(ctx);
    };
    let result = if let Some(transform) = transform {
        let Some((callback, this_value, readable_controller)) = state
            .streams
            .transforms
            .get(transform as usize)
            .map(|transform| {
                (
                    transform.transform,
                    transform.transformer,
                    transform.controller,
                )
            })
        else {
            return super::super::fail_dispatch(ctx);
        };
        if let Some(callback) = callback {
            let controller_object = state.streams.controllers[readable_controller as usize].object;
            state
                .invoke_callable(ctx, callback, this_value, &[chunk, controller_object])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        } else {
            super::readable::call_controller(
                ctx,
                state,
                readable_controller,
                super::ControllerMethod::Enqueue,
                &[chunk],
            )
        }
    } else {
        let controller = &state.streams.writable_controllers[controller as usize];
        if let Some(callback) = controller.write {
            let callback_this = controller.sink;
            let controller_object = controller.object;
            state
                .invoke_callable(ctx, callback, callback_this, &[chunk, controller_object])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        } else {
            value::encode_undefined()
        }
    };
    super::super::promise::resolve_into(ctx, state, promise, result);
    value::encode_undefined()
}

fn start_close(ctx: &mut NativeVmContext, state: &mut NativeAgentState, stream: u32) -> i64 {
    let Some((promise, promise_handle)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    let Some(status) = state
        .streams
        .writables
        .get(stream as usize)
        .map(|stream| stream.status)
    else {
        return super::super::fail_dispatch(ctx);
    };
    if status != WritableStatus::Writable {
        let reason = type_error(ctx, state, "WritableStream cannot be closed");
        super::super::promise::settle_promise(
            state,
            promise_handle,
            state.exception_value(reason).unwrap_or(reason),
            true,
        );
        return promise;
    }
    state.streams.writables[stream as usize].status = WritableStatus::Closing;
    super::super::promise::enqueue_stream_task(
        state,
        StreamTask::CloseWritable {
            stream,
            promise: promise_handle,
        },
    );
    promise
}

pub(super) fn start_pipe_close(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    readable: u32,
) -> i64 {
    state.streams.writables[stream as usize].pipe_source = Some(readable);
    start_close(ctx, state, stream)
}

pub(super) fn run_close(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    promise: u32,
) -> i64 {
    let Some((controller, transform)) = state
        .streams
        .writables
        .get(stream as usize)
        .map(|stream| (stream.controller, stream.transform))
    else {
        return super::super::fail_dispatch(ctx);
    };
    let result = if let Some(transform) = transform {
        let Some((callback, this_value, readable_controller)) = state
            .streams
            .transforms
            .get(transform as usize)
            .map(|transform| (transform.flush, transform.transformer, transform.controller))
        else {
            return super::super::fail_dispatch(ctx);
        };
        if let Some(callback) = callback {
            let controller_object = state.streams.controllers[readable_controller as usize].object;
            state
                .invoke_callable(ctx, callback, this_value, &[controller_object])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        } else {
            value::encode_undefined()
        }
    } else {
        let controller = &state.streams.writable_controllers[controller as usize];
        if let Some(callback) = controller.close {
            let callback_this = controller.sink;
            let controller_object = controller.object;
            state
                .invoke_callable(ctx, callback, callback_this, &[controller_object])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        } else {
            value::encode_undefined()
        }
    };
    let source = value::decode_handle(result);
    if value::is_object(result) && state.promises.contains_key(&source) {
        super::super::promise::observe(
            state,
            source,
            StreamReaction::FinishClose { stream, promise },
        );
    } else {
        let rejected = value::is_exception(result);
        let stored = if rejected {
            state.exception_value(result).unwrap_or(result)
        } else {
            result
        };
        finish_close(ctx, state, stream, promise, stored, rejected);
    }
    value::encode_undefined()
}

pub(super) fn finish_close(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    promise: u32,
    stored: i64,
    rejected: bool,
) -> i64 {
    let pipe_source = state.streams.writables[stream as usize].pipe_source.take();
    if rejected {
        error_writable(state, stream, stored);
        if let Some(readable) = pipe_source {
            super::readable::finish_pipe_write(ctx, state, readable, stored, true);
        }
        super::super::promise::settle_promise(state, promise, stored, true);
        return value::encode_undefined();
    }
    let transform = state.streams.writables[stream as usize].transform;
    state.streams.writables[stream as usize].status = WritableStatus::Closed;
    let closed_promises: Vec<_> = state
        .streams
        .writers
        .iter()
        .filter(|writer| writer.stream == stream)
        .map(|writer| writer.closed_promise)
        .collect();
    for promise in closed_promises {
        super::super::promise::settle_promise(state, promise, value::encode_undefined(), false);
    }
    if let Some(readable) = pipe_source {
        super::readable::finish_pipe_write(ctx, state, readable, stored, false);
    }
    if let Some(transform) = transform {
        let controller = state.streams.transforms[transform as usize].controller;
        let result = super::readable::call_controller(
            ctx,
            state,
            controller,
            super::ControllerMethod::Close,
            &[],
        );
        if value::is_exception(result) {
            let reason = state.exception_value(result).unwrap_or(result);
            error_writable(state, stream, reason);
            super::super::promise::settle_promise(state, promise, reason, true);
            return result;
        }
    }
    super::super::promise::settle_promise(state, promise, value::encode_undefined(), false);
    value::encode_undefined()
}

fn abort(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    args: &[i64],
) -> i64 {
    let reason = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(controller) = state
        .streams
        .writables
        .get(stream as usize)
        .map(|stream| stream.controller)
    else {
        return super::super::fail_dispatch(ctx);
    };
    let signal = state.streams.writable_controllers[controller as usize].signal;
    let _ = super::super::modules::set_named_property(
        state,
        signal,
        "aborted",
        value::encode_bool(true),
    );
    let callback = state.streams.writable_controllers[controller as usize].abort;
    let sink = state.streams.writable_controllers[controller as usize].sink;
    state.streams.writables[stream as usize].status = WritableStatus::Errored;
    if let Some(callback) = callback {
        let result = state
            .invoke_callable(ctx, callback, sink, &[reason])
            .unwrap_or_else(|| super::super::fail_dispatch(ctx));
        if value::is_exception(result) {
            return super::super::promise::rejected_promise(
                ctx,
                state,
                state.exception_value(result).unwrap_or(result),
            );
        }
    }
    resolved(ctx, state, value::encode_undefined())
}

fn error_writable(state: &mut NativeAgentState, stream: u32, reason: i64) {
    if let Some(stream) = state.streams.writables.get_mut(stream as usize) {
        stream.status = WritableStatus::Errored;
        stream.locked = false;
    }
    let promises: Vec<_> = state
        .streams
        .writers
        .iter()
        .filter(|writer| writer.stream == stream)
        .map(|writer| writer.closed_promise)
        .collect();
    for promise in promises {
        super::super::promise::settle_promise(state, promise, reason, true);
    }
}
