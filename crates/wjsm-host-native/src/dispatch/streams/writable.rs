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
    if value::is_js_object(sink) {
        // start 的属性读取与调用可再入 JS 触发 GC，此刻流与 controller
        // 包装对象仅由局部值持有，须钉扎到构造结束。
        let initial_temp_roots = state.temporary_roots.len();
        state.temporary_roots.push(stream);
        state.temporary_roots.push(controller);
        let start = callable_property(ctx, state, sink, "start");
        if let Some(start) = start {
            let result = state
                .invoke_callable(ctx, start, sink, &[controller])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx));
            if value::is_exception(result) {
                state.temporary_roots.truncate(initial_temp_roots);
                return result;
            }
        }
        state.temporary_roots.truncate(initial_temp_roots);
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
    let controller = state.streams.readables[readable_handle].controller;
    // GC 重试分配与 start 回调可触发 GC，此刻 readable/controller/writable/
    // transform 包装对象仅由局部值持有，须钉扎到构造结束。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(readable);
    state.temporary_roots.push(controller_object);
    let Some((writable, _)) = create_writable(state, transformer, None, None, None, None) else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    state.temporary_roots.push(writable);
    let Some(ObjectKind::Writable(writable_handle)) = state
        .streams
        .objects
        .get(&value::decode_handle(writable))
        .copied()
    else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 3, false) else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    if state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::TransformStreamConstructor)
        .is_err()
    {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    }
    let Some(transform_handle) = state.streams.transforms.insert(TransformState {
        object,
        readable: readable_handle,
        writable: writable_handle,
        controller,
        transformer,
        transform,
        flush,
    }) else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    state.streams.writables[writable_handle].transform = Some(transform_handle);
    register_object(state, object, ObjectKind::Transform(transform_handle));
    state.temporary_roots.push(object);
    if value::is_js_object(transformer)
        && let Some(start) = callable_property(ctx, state, transformer, "start")
    {
        let result = state
            .invoke_callable(ctx, start, transformer, &[controller_object])
            .unwrap_or_else(|| super::super::fail_dispatch(ctx));
        if value::is_exception(result) {
            state.temporary_roots.truncate(initial_temp_roots);
            return result;
        }
    }
    state.temporary_roots.truncate(initial_temp_roots);
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
    // peek 与两次 insert 之间没有其他插入/清扫（普通分配不触发同步 GC），
    // 交叉下标由此保持一致。
    let controller = state.streams.writable_controllers.peek_handle()?;
    let stream = state.streams.writables.insert(WritableState {
        object,
        controller,
        status: WritableStatus::Writable,
        locked: false,
        transform,
        pipe_source: None,
    })?;
    let inserted = state
        .streams
        .writable_controllers
        .insert(WritableControllerState {
            object: controller_object,
            stream,
            sink,
            write,
            close,
            abort,
            signal,
        })?;
    debug_assert_eq!(inserted, controller);
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
    let stream = state.streams.writables.get(handle)?;
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
    let writer = state.streams.writers.get(handle)?;
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
    let controller = state.streams.writable_controllers.get(handle)?;
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
        .get(writer)
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
        .get(controller)
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
        .get(stream)
        .map(|stream| stream.locked)
    else {
        return super::super::fail_dispatch(ctx);
    };
    if locked {
        return type_error(ctx, state, "WritableStream is already locked");
    }
    // 第二个 promise 与 writer 对象的分配可触发 GC，closed/ready promise
    // 在挂入侧表前仅由局部值持有，须钉扎。
    let initial_temp_roots = state.temporary_roots.len();
    let Some((closed, closed_promise)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    state.temporary_roots.push(closed);
    let Some((ready, ready_promise)) = new_promise(ctx, state) else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    state.temporary_roots.push(ready);
    super::super::promise::settle_promise(state, ready_promise, value::encode_undefined(), false);
    let object = state.allocate_object_with_gc_retry(ctx, 8, false);
    state.temporary_roots.truncate(initial_temp_roots);
    let Ok(object) = object else {
        return super::super::fail_dispatch(ctx);
    };
    let Some(writer) = state.streams.writers.insert(WriterState {
        object,
        stream,
        closed_promise,
        ready_promise,
    }) else {
        return super::super::fail_dispatch(ctx);
    };
    state.streams.writables[stream].locked = true;
    register_object(state, object, ObjectKind::Writer(writer));
    object
}

fn release_writer(state: &mut NativeAgentState, writer: u32) {
    if let Some(stream) = state
        .streams
        .writers
        .get(writer)
        .map(|writer| writer.stream)
        && let Some(stream) = state.streams.writables.get_mut(stream)
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
        .get(stream)
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
        .get(stream)
        .map(|stream| (stream.controller, stream.transform))
    else {
        return super::super::fail_dispatch(ctx);
    };
    let result = if let Some(transform) = transform {
        let Some((callback, this_value, readable_controller)) =
            state.streams.transforms.get(transform).map(|transform| {
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
            let controller_object = state.streams.controllers[readable_controller].object;
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
        let controller = &state.streams.writable_controllers[controller];
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
        .get(stream)
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
    state.streams.writables[stream].status = WritableStatus::Closing;
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
    state.streams.writables[stream].pipe_source = Some(readable);
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
        .get(stream)
        .map(|stream| (stream.controller, stream.transform))
    else {
        return super::super::fail_dispatch(ctx);
    };
    let result = if let Some(transform) = transform {
        let Some((callback, this_value, readable_controller)) = state
            .streams
            .transforms
            .get(transform)
            .map(|transform| (transform.flush, transform.transformer, transform.controller))
        else {
            return super::super::fail_dispatch(ctx);
        };
        if let Some(callback) = callback {
            let controller_object = state.streams.controllers[readable_controller].object;
            state
                .invoke_callable(ctx, callback, this_value, &[controller_object])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        } else {
            value::encode_undefined()
        }
    } else {
        let controller = &state.streams.writable_controllers[controller];
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
    let pipe_source = state.streams.writables[stream].pipe_source.take();
    if rejected {
        error_writable(state, stream, stored);
        if let Some(readable) = pipe_source {
            super::readable::finish_pipe_write(ctx, state, readable, stored, true);
        }
        super::super::promise::settle_promise(state, promise, stored, true);
        return value::encode_undefined();
    }
    let transform = state.streams.writables[stream].transform;
    state.streams.writables[stream].status = WritableStatus::Closed;
    let closed_promises: Vec<_> = state
        .streams
        .writers
        .iter()
        .filter(|(_, writer)| writer.stream == stream)
        .map(|(_, writer)| writer.closed_promise)
        .collect();
    for promise in closed_promises {
        super::super::promise::settle_promise(state, promise, value::encode_undefined(), false);
    }
    if let Some(readable) = pipe_source {
        super::readable::finish_pipe_write(ctx, state, readable, stored, false);
    }
    if let Some(transform) = transform {
        let controller = state.streams.transforms[transform].controller;
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
        .get(stream)
        .map(|stream| stream.controller)
    else {
        return super::super::fail_dispatch(ctx);
    };
    let signal = state.streams.writable_controllers[controller].signal;
    let _ = super::super::modules::set_named_property(
        state,
        signal,
        "aborted",
        value::encode_bool(true),
    );
    let callback = state.streams.writable_controllers[controller].abort;
    let sink = state.streams.writable_controllers[controller].sink;
    state.streams.writables[stream].status = WritableStatus::Errored;
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
    if let Some(stream) = state.streams.writables.get_mut(stream) {
        stream.status = WritableStatus::Errored;
        stream.locked = false;
    }
    let promises: Vec<_> = state
        .streams
        .writers
        .iter()
        .filter(|(_, writer)| writer.stream == stream)
        .map(|(_, writer)| writer.closed_promise)
        .collect();
    for promise in promises {
        super::super::promise::settle_promise(state, promise, reason, true);
    }
}
