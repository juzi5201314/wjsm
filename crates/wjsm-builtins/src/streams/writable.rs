use wjsm_host::{
    ControllerKind, ExecContext, NativeCallableRef, PromiseSettlement, StreamControllerEntry,
    Value, WritableStreamDefaultControllerMethodKind, WritableStreamDefaultWriterMethodKind,
    WritableStreamEntry, WritableStreamMethodKind, WritableStreamState, WriterEntry,
};
use wjsm_ir::{constants, value};

use super::{define_accessor_property, define_data_property_with_flags};

fn new_writable_controller(high_water_mark: f64) -> StreamControllerEntry {
    StreamControllerEntry {
        kind: ControllerKind::Writable,
        stream_handle: 0,
        chunk_queue: std::collections::VecDeque::new(),
        high_water_mark,
        strategy_size: None,
        started: false,
        close_requested: false,
        byob_reader_handle: None,
        pull_requested: false,
        abort_requested: false,
        abort_reason: None,
        flush_requested: false,
        underlying_source: None,
        pull_callback: None,
        write_callback: None,
        sink_close_callback: None,
        cancel_callback: None,
        active_byob_request: None,
    }
}

pub fn create_writable_stream_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(5);
    define_data_property_with_flags(
        ctx,
        object,
        "__writable_stream_handle__",
        value::encode_f64(handle as f64),
        constants::FLAG_PRIVATE,
    );
    let locked = ctx.create_native_callable(NativeCallableRef::WritableStreamMethod {
        handle,
        kind: WritableStreamMethodKind::GetLocked,
    });
    define_accessor_property(ctx, object, "locked", locked);
    for (name, kind) in [
        ("getWriter", WritableStreamMethodKind::GetWriter),
        ("abort", WritableStreamMethodKind::Abort),
        ("close", WritableStreamMethodKind::Close),
    ] {
        let callable = ctx.create_native_callable(NativeCallableRef::WritableStreamMethod {
            handle,
            kind,
        });
        define_data_property_with_flags(
            ctx,
            object,
            name,
            callable,
            constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
        );
    }
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_writable_stream_object(object_handle, handle);
    }
    object
}

fn create_writable_controller_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(3);
    define_data_property_with_flags(
        ctx,
        object,
        "__controller_handle__",
        value::encode_f64(handle as f64),
        constants::FLAG_PRIVATE,
    );
    let error = ctx.create_native_callable(
        NativeCallableRef::WritableStreamDefaultControllerMethod {
            handle,
            kind: WritableStreamDefaultControllerMethodKind::Error,
        },
    );
    define_data_property_with_flags(
        ctx,
        object,
        "error",
        error,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    let signal = ctx.create_native_callable(
        NativeCallableRef::WritableStreamDefaultControllerMethod {
            handle,
            kind: WritableStreamDefaultControllerMethodKind::GetSignal,
        },
    );
    define_accessor_property(ctx, object, "signal", signal);
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_stream_controller_object(object_handle, handle);
    }
    object
}

fn create_writer_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(8);
    define_data_property_with_flags(
        ctx,
        object,
        "__writer_handle__",
        value::encode_f64(handle as f64),
        constants::FLAG_PRIVATE,
    );
    for (name, kind) in [
        ("write", WritableStreamDefaultWriterMethodKind::Write),
        ("close", WritableStreamDefaultWriterMethodKind::Close),
        ("abort", WritableStreamDefaultWriterMethodKind::Abort),
        (
            "releaseLock",
            WritableStreamDefaultWriterMethodKind::ReleaseLock,
        ),
    ] {
        let callable = ctx.create_native_callable(
            NativeCallableRef::WritableStreamDefaultWriterMethod { handle, kind },
        );
        define_data_property_with_flags(
            ctx,
            object,
            name,
            callable,
            constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
        );
    }
    for (name, kind) in [
        ("closed", WritableStreamDefaultWriterMethodKind::GetClosed),
        ("ready", WritableStreamDefaultWriterMethodKind::GetReady),
        (
            "desiredSize",
            WritableStreamDefaultWriterMethodKind::GetDesiredSize,
        ),
    ] {
        let getter = ctx.create_native_callable(
            NativeCallableRef::WritableStreamDefaultWriterMethod { handle, kind },
        );
        define_accessor_property(ctx, object, name, getter);
    }
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_writer_object(object_handle, handle);
    }
    object
}

pub async fn construct<E: ExecContext>(ctx: &mut E, args: &[Value]) -> Value {
    let sink = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let high_water_mark = if value::is_js_object(strategy) {
        let raw = ctx.read_property_by_string_key(strategy, "highWaterMark");
        let number = crate::core::to_number(ctx, raw);
        if value::is_exception(number) {
            return number;
        }
        let number = value::decode_f64(number);
        if number >= 0.0 && number.is_finite() {
            number
        } else {
            1.0
        }
    } else {
        1.0
    };
    let controller = ctx.alloc_stream_controller(new_writable_controller(high_water_mark));
    let signal = ctx.create_writable_abort_signal();
    let stream = ctx.alloc_writable_stream(WritableStreamEntry {
        state: WritableStreamState::Writable,
        error: None,
        locked: false,
        controller_handle: Some(controller),
        abort_signal: Some(signal),
    });
    let _ = ctx.with_stream_controller(controller, |entry| entry.stream_handle = stream);
    let controller_object = create_writable_controller_object(ctx, controller);
    if value::is_js_object(sink) {
        let start = ctx.read_property_by_string_key(sink, "start");
        if ctx.is_callable(start) {
            let arguments = [controller_object];
            let _ = ctx.call_js_async(start, sink, &arguments).await;
        }
        let write = ctx.read_property_by_string_key(sink, "write");
        let close = ctx.read_property_by_string_key(sink, "close");
        let abort = ctx.read_property_by_string_key(sink, "abort");
        let write = ctx.is_callable(write).then_some(write);
        let close = ctx.is_callable(close).then_some(close);
        let abort = ctx.is_callable(abort).then_some(abort);
        let _ = ctx.with_stream_controller(controller, |entry| {
            entry.underlying_source = Some(sink);
            entry.write_callback = write;
            entry.sink_close_callback = close;
            entry.cancel_callback = abort;
        });
    }
    let _ = ctx.with_stream_controller(controller, |entry| entry.started = true);
    create_writable_stream_object(ctx, stream)
}

fn call_sink_write<E: ExecContext>(ctx: &mut E, stream: u32, chunk: Value, promise: Value) {
    let controller = ctx
        .with_writable_stream(stream, |entry| entry.controller_handle)
        .flatten();
    let Some(controller) = controller else {
        ctx.settle_promise(
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
        return;
    };
    let info = ctx.with_stream_controller(controller, |entry| {
        (entry.write_callback, entry.underlying_source)
    });
    let Some((callback, this_value)) = info else {
        ctx.settle_promise(
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
        return;
    };
    if let Some(callback) = callback {
        let controller_object = create_writable_controller_object(ctx, controller);
        ctx.schedule_writable_sink_write(
            callback,
            this_value.unwrap_or_else(value::encode_undefined),
            chunk,
            controller_object,
            promise,
        );
    } else {
        ctx.settle_promise(
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
}

fn call_sink_close<E: ExecContext>(ctx: &mut E, stream: u32, promise: Value) -> bool {
    let controller = ctx
        .with_writable_stream(stream, |entry| entry.controller_handle)
        .flatten();
    let Some(controller) = controller else {
        return false;
    };
    let Some((callback, this_value)) = ctx.with_stream_controller(controller, |entry| {
        (entry.sink_close_callback, entry.underlying_source)
    }) else {
        return false;
    };
    let controller_object = create_writable_controller_object(ctx, controller);
    ctx.schedule_writable_sink_close(
        callback,
        this_value.unwrap_or_else(value::encode_undefined),
        controller_object,
        stream,
        promise,
    );
    true
}

pub fn write_from_pipe<E: ExecContext>(ctx: &mut E, stream: u32, chunk: Value, promise: Value) {
    let is_transform = ctx.with_transform_streams(|entries| {
        entries.iter().filter_map(Option::as_ref).any(|entry| {
            entry.writable_stream_handle == Some(stream)
        })
    });
    if is_transform {
        super::transform::call_transform_from_writable(ctx, stream, chunk, promise);
    } else {
        call_sink_write(ctx, stream, chunk, promise);
    }
}

pub fn close_from_pipe<E: ExecContext>(ctx: &mut E, stream: u32, promise: Value) -> bool {
    if super::transform::call_flush_from_writable_close(ctx, stream, promise) {
        true
    } else {
        call_sink_close(ctx, stream, promise)
    }
}

pub fn finish_close<E: ExecContext>(ctx: &mut E, stream: u32, promise: Value) {
    let _ = ctx.with_writable_stream(stream, |entry| {
        entry.state = WritableStreamState::Closed;
    });
    let closed = ctx.with_writers(|writers| {
        writers
            .iter()
            .filter_map(Option::as_ref)
            .filter(|writer| writer.writable_stream_handle == stream)
            .filter_map(|writer| writer.closed_promise)
            .collect::<Vec<_>>()
    });
    for closed in closed {
        ctx.settle_promise(
            closed,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
    ctx.settle_promise(
        promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
}

fn reject_writer_promises<E: ExecContext>(ctx: &mut E, stream: u32, reason: Value) {
    let promises = ctx.with_writers(|writers| {
        writers
            .iter()
            .filter_map(Option::as_ref)
            .filter(|writer| writer.writable_stream_handle == stream)
            .flat_map(|writer| [writer.closed_promise, writer.ready_promise])
            .flatten()
            .collect::<Vec<_>>()
    });
    for promise in promises {
        ctx.settle_promise(promise, PromiseSettlement::Reject(reason));
    }
}

fn abort_stream<E: ExecContext>(ctx: &mut E, stream: u32, reason: Value) {
    let _ = ctx.with_writable_stream(stream, |entry| {
        entry.state = WritableStreamState::Errored;
        entry.error = Some(reason);
    });
    ctx.mark_writable_stream_signal_aborted(stream, reason);
    reject_writer_promises(ctx, stream, reason);
}

fn close_stream<E: ExecContext>(ctx: &mut E, stream: u32, promise: Value) {
    let current = ctx.with_writable_stream(stream, |entry| {
        (entry.controller_handle, entry.state)
    });
    let Some((controller, WritableStreamState::Writable)) = current else {
        let error = ctx.make_type_error("WritableStream is not in writable state");
        ctx.settle_promise(promise, PromiseSettlement::Reject(error));
        return;
    };
    let _ = ctx.with_writable_stream(stream, |entry| {
        entry.state = WritableStreamState::Closing;
    });
    if let Some(controller) = controller {
        let _ = ctx.with_stream_controller(controller, |entry| {
            entry.close_requested = true;
        });
    }
    if !close_from_pipe(ctx, stream, promise) {
        finish_close(ctx, stream, promise);
    }
}

pub fn call_writable_stream_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: WritableStreamMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        WritableStreamMethodKind::GetLocked => Some(value::encode_bool(
            ctx.with_writable_stream(handle, |entry| entry.locked)
                .unwrap_or(false),
        )),
        WritableStreamMethodKind::GetWriter => get_writer(ctx, handle),
        WritableStreamMethodKind::Abort => {
            let reason = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let promise = ctx.alloc_promise();
            abort_stream(ctx, handle, reason);
            ctx.settle_promise(
                promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
            Some(promise)
        }
        WritableStreamMethodKind::Close => {
            let promise = ctx.alloc_promise();
            close_stream(ctx, handle, promise);
            Some(promise)
        }
    }
}

fn get_writer<E: ExecContext>(ctx: &mut E, stream: u32) -> Option<Value> {
    let state = ctx.with_writable_stream(stream, |entry| {
        if entry.locked {
            return None;
        }
        entry.locked = true;
        Some((entry.state, entry.error))
    })??;
    let closed = ctx.alloc_promise();
    let ready = ctx.alloc_promise();
    match state {
        (WritableStreamState::Writable, _) => ctx.settle_promise(
            ready,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        ),
        (WritableStreamState::Closed, _) => ctx.settle_promise(
            closed,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        ),
        (WritableStreamState::Errored, error) => {
            let error = error.unwrap_or_else(value::encode_undefined);
            ctx.settle_promise(closed, PromiseSettlement::Reject(error));
            ctx.settle_promise(ready, PromiseSettlement::Reject(error));
        }
        (WritableStreamState::Closing, _) => {}
    }
    let writer = ctx.alloc_writer(WriterEntry {
        writable_stream_handle: stream,
        closed_promise: Some(closed),
        ready_promise: Some(ready),
    });
    Some(create_writer_object(ctx, writer))
}

pub fn call_default_writer_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: WritableStreamDefaultWriterMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        WritableStreamDefaultWriterMethodKind::Write => writer_write(ctx, handle, args),
        WritableStreamDefaultWriterMethodKind::Close => writer_close(ctx, handle),
        WritableStreamDefaultWriterMethodKind::Abort => writer_abort(ctx, handle, args),
        WritableStreamDefaultWriterMethodKind::ReleaseLock => {
            if let Some(stream) = ctx.with_writer(handle, |entry| entry.writable_stream_handle) {
                let _ = ctx.with_writable_stream(stream, |entry| entry.locked = false);
            }
            Some(value::encode_undefined())
        }
        WritableStreamDefaultWriterMethodKind::GetClosed => {
            ctx.with_writer(handle, |entry| entry.closed_promise).flatten()
        }
        WritableStreamDefaultWriterMethodKind::GetReady => {
            ctx.with_writer(handle, |entry| entry.ready_promise).flatten()
        }
        WritableStreamDefaultWriterMethodKind::GetDesiredSize => {
            let stream = ctx.with_writer(handle, |entry| entry.writable_stream_handle)?;
            let controller = ctx
                .with_writable_stream(stream, |entry| entry.controller_handle)
                .flatten();
            Some(
                controller
                    .and_then(|controller| {
                        ctx.with_stream_controller(controller, |entry| {
                            value::encode_f64(
                                entry.high_water_mark - entry.chunk_queue.len() as f64,
                            )
                        })
                    })
                    .unwrap_or_else(value::encode_null),
            )
        }
    }
}

fn writer_write<E: ExecContext>(ctx: &mut E, writer: u32, args: &[Value]) -> Option<Value> {
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let promise = ctx.alloc_promise();
    let Some(stream) = ctx.with_writer(writer, |entry| entry.writable_stream_handle) else {
        let error = ctx.make_type_error("writer is not attached to a stream");
        ctx.settle_promise(promise, PromiseSettlement::Reject(error));
        return Some(promise);
    };
    let state = ctx.with_writable_stream(stream, |entry| (entry.state, entry.error));
    match state {
        Some((WritableStreamState::Writable, _)) => {
            write_from_pipe(ctx, stream, chunk, promise)
        }
        Some((WritableStreamState::Errored, error)) => ctx.settle_promise(
            promise,
            PromiseSettlement::Reject(error.unwrap_or_else(value::encode_undefined)),
        ),
        Some((WritableStreamState::Closing | WritableStreamState::Closed, _)) => {
            let error = ctx.make_type_error("Cannot write to a closing/closed stream");
            ctx.settle_promise(promise, PromiseSettlement::Reject(error));
        }
        None => {
            let error = ctx.make_type_error("stream not found");
            ctx.settle_promise(promise, PromiseSettlement::Reject(error));
        }
    }
    Some(promise)
}

fn writer_close<E: ExecContext>(ctx: &mut E, writer: u32) -> Option<Value> {
    let promise = ctx.alloc_promise();
    if let Some(stream) = ctx.with_writer(writer, |entry| entry.writable_stream_handle) {
        close_stream(ctx, stream, promise);
    } else {
        let error = ctx.make_type_error("writer is not attached to a stream");
        ctx.settle_promise(promise, PromiseSettlement::Reject(error));
    }
    Some(promise)
}

fn writer_abort<E: ExecContext>(ctx: &mut E, writer: u32, args: &[Value]) -> Option<Value> {
    let reason = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let promise = ctx.alloc_promise();
    if let Some(stream) = ctx.with_writer(writer, |entry| entry.writable_stream_handle) {
        abort_stream(ctx, stream, reason);
        ctx.settle_promise(
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    } else {
        let error = ctx.make_type_error("writer is not attached to a stream");
        ctx.settle_promise(promise, PromiseSettlement::Reject(error));
    }
    Some(promise)
}

pub fn call_controller_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: WritableStreamDefaultControllerMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        WritableStreamDefaultControllerMethodKind::Error => {
            let error = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if let Some(stream) =
                ctx.with_stream_controller(handle, |entry| entry.stream_handle)
            {
                let _ = ctx.with_writable_stream(stream, |entry| {
                    entry.state = WritableStreamState::Errored;
                    entry.error = Some(error);
                });
                reject_writer_promises(ctx, stream, error);
            }
            Some(value::encode_undefined())
        }
        WritableStreamDefaultControllerMethodKind::GetSignal => {
            let signal = ctx
                .with_stream_controller(handle, |entry| entry.stream_handle)
                .and_then(|stream| {
                    ctx.with_writable_stream(stream, |entry| entry.abort_signal)
                        .flatten()
                });
            Some(signal.unwrap_or_else(value::encode_undefined))
        }
    }
}
