use wjsm_host::{
    ByobRequestEntry, ExecContext, NativeCallableRef, PromiseReaction, PromiseSettlement,
    ReactionType, ReadableStreamByobRequestMethodKind,
    ReadableStreamDefaultControllerMethodKind, ReadableStreamDefaultReaderMethodKind,
    ReadableStreamPipeToEntry, ReaderKind, StreamState, Value,
};
use wjsm_ir::value;

use super::{
    build_reader_result, create_byob_request_object, create_controller_object, fulfill_byob_read,
    reject_promise_with_type_error,
};

pub fn readable_stream_pipe_to<E: ExecContext>(
    ctx: &mut E,
    readable_handle: u32,
    destination: Value,
) -> Option<Value> {
    let promise = ctx.alloc_promise();
    let Some(destination) =
        super::readable_dispatch::writable_stream_handle_from_object(ctx, destination)
    else {
        reject_promise_with_type_error(
            ctx,
            promise,
            "pipeTo destination must be a WritableStream",
        );
        return Some(promise);
    };
    let can_start = ctx.with_readable_stream(readable_handle, |stream| {
        if stream.pipe_to.is_some() || stream.locked {
            return false;
        }
        stream.disturbed = true;
        stream.pipe_to = Some(ReadableStreamPipeToEntry {
            destination,
            promise,
            write_in_flight: false,
            closing: false,
        });
        true
    })?;
    if !can_start {
        reject_promise_with_type_error(ctx, promise, "ReadableStream is already locked");
        return Some(promise);
    }
    pump_readable_stream_pipe_to(ctx, readable_handle);
    Some(promise)
}

pub fn pump_readable_stream_pipe_to<E: ExecContext>(ctx: &mut E, readable_handle: u32) {
    match next_pipe_to_step(ctx, readable_handle) {
        PipeToStep::Write { destination, chunk } => {
            let write_promise = ctx.alloc_promise();
            attach_pipe_to_write_reactions(ctx, write_promise, readable_handle);
            super::writable::write_from_pipe(ctx, destination, chunk, write_promise);
        }
        PipeToStep::Close {
            destination,
            promise,
        } => {
            if !super::writable::close_from_pipe(ctx, destination, promise) {
                super::writable::finish_close(ctx, destination, promise);
                clear_pipe_to(ctx, readable_handle);
            }
        }
        PipeToStep::WaitForMore | PipeToStep::Done => {}
    }
}

enum PipeToStep {
    Write { destination: u32, chunk: Value },
    Close { destination: u32, promise: Value },
    WaitForMore,
    Done,
}

fn next_pipe_to_step<E: ExecContext>(ctx: &mut E, readable_handle: u32) -> PipeToStep {
    let Some((controller_handle, state, pipe_to)) = ctx.with_readable_stream(
        readable_handle,
        |stream| (stream.controller_handle, stream.state.clone(), stream.pipe_to),
    ) else {
        return PipeToStep::Done;
    };
    let Some(pipe_to) = pipe_to else {
        return PipeToStep::Done;
    };
    if pipe_to.write_in_flight || pipe_to.closing {
        return PipeToStep::WaitForMore;
    }
    if let Some(controller_handle) = controller_handle {
        let chunk = ctx
            .with_stream_controller(controller_handle, |controller| {
                controller.chunk_queue.pop_front()
            })
            .flatten();
        if let Some(chunk) = chunk {
            set_pipe_to_write_in_flight(ctx, readable_handle, true);
            return PipeToStep::Write {
                destination: pipe_to.destination,
                chunk,
            };
        }
    }
    let close_requested = controller_handle
        .and_then(|handle| {
            ctx.with_stream_controller(handle, |controller| controller.close_requested)
        })
        .unwrap_or(true);
    if matches!(state, StreamState::Closed) || close_requested {
        let _ = ctx.with_readable_stream(readable_handle, |stream| {
            if let Some(pipe_to) = stream.pipe_to.as_mut() {
                pipe_to.closing = true;
            }
        });
        return PipeToStep::Close {
            destination: pipe_to.destination,
            promise: pipe_to.promise,
        };
    }
    schedule_pipe_to_pull(ctx, controller_handle, readable_handle);
    PipeToStep::WaitForMore
}

fn schedule_pipe_to_pull<E: ExecContext>(
    ctx: &mut E,
    controller_handle: Option<u32>,
    readable_handle: u32,
) {
    let Some(controller_handle) = controller_handle else {
        return;
    };
    let pull = ctx.with_stream_controller(controller_handle, |controller| {
        controller
            .pull_callback
            .map(|callback| (callback, controller.underlying_source))
    });
    if let Some(Some((callback, this_value))) = pull {
        let controller = create_controller_object(ctx, controller_handle);
        ctx.schedule_readable_pull(
            callback,
            this_value.unwrap_or_else(value::encode_undefined),
            controller,
        );
        ctx.schedule_readable_pipe_pump(readable_handle);
    }
}

fn attach_pipe_to_write_reactions<E: ExecContext>(
    ctx: &mut E,
    write_promise: Value,
    readable_handle: u32,
) {
    let fulfill = ctx.create_native_callable(
        NativeCallableRef::ReadableStreamPipeToWriteFulfilled { readable_handle },
    );
    let reject = ctx.create_native_callable(
        NativeCallableRef::ReadableStreamPipeToWriteRejected { readable_handle },
    );
    ctx.mark_promise_handled(write_promise);
    ctx.push_promise_reaction(
        write_promise,
        PromiseReaction::new(fulfill, write_promise, ReactionType::Fulfill),
        true,
    );
    ctx.push_promise_reaction(
        write_promise,
        PromiseReaction::new(reject, write_promise, ReactionType::Reject),
        false,
    );
}

pub fn finish_pipe_to_write<E: ExecContext>(
    ctx: &mut E,
    readable_handle: u32,
    error: Option<Value>,
) -> Value {
    if let Some(error) = error {
        if let Some(promise) = pipe_to_promise(ctx, readable_handle) {
            ctx.settle_promise(promise, PromiseSettlement::Reject(error));
        }
        clear_pipe_to(ctx, readable_handle);
    } else {
        set_pipe_to_write_in_flight(ctx, readable_handle, false);
        pump_readable_stream_pipe_to(ctx, readable_handle);
    }
    value::encode_undefined()
}

fn pipe_to_promise<E: ExecContext>(ctx: &mut E, readable_handle: u32) -> Option<Value> {
    ctx.with_readable_stream(readable_handle, |stream| {
        stream.pipe_to.map(|pipe_to| pipe_to.promise)
    })
    .flatten()
}

pub fn clear_pipe_to<E: ExecContext>(ctx: &mut E, readable_handle: u32) {
    let _ = ctx.with_readable_stream(readable_handle, |stream| {
        stream.pipe_to = None;
    });
}

fn set_pipe_to_write_in_flight<E: ExecContext>(
    ctx: &mut E,
    readable_handle: u32,
    write_in_flight: bool,
) {
    let _ = ctx.with_readable_stream(readable_handle, |stream| {
        if let Some(pipe_to) = stream.pipe_to.as_mut() {
            pipe_to.write_in_flight = write_in_flight;
        }
    });
}

pub fn readable_stream_pipe_through<E: ExecContext>(
    ctx: &mut E,
    readable_handle: u32,
    transform: Value,
) -> Option<Value> {
    let Some((readable, writable)) =
        super::readable_dispatch::transform_parts_from_object(ctx, transform)
    else {
        return Some(ctx.make_type_error(
            "pipeThrough transform must contain readable and writable",
        ));
    };
    let _ = readable_stream_pipe_to(ctx, readable_handle, writable);
    Some(readable)
}

pub fn call_default_reader_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: ReadableStreamDefaultReaderMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        ReadableStreamDefaultReaderMethodKind::Read => reader_read(ctx, handle, args),
        ReadableStreamDefaultReaderMethodKind::ReleaseLock => {
            let stream_handle = ctx.with_reader(handle, |reader| reader.stream_handle)?;
            let _ = ctx.with_readable_stream(stream_handle, |stream| stream.locked = false);
            Some(value::encode_undefined())
        }
        ReadableStreamDefaultReaderMethodKind::GetClosed => ctx
            .with_reader(handle, |reader| {
                reader
                    .closed_promise
                    .unwrap_or_else(value::encode_undefined)
            }),
    }
}

fn reader_read<E: ExecContext>(ctx: &mut E, handle: u32, args: &[Value]) -> Option<Value> {
    let (stream_handle, reader_kind) =
        ctx.with_reader(handle, |reader| (reader.stream_handle, reader.kind))?;
    let byob_view = (reader_kind == ReaderKind::Byob).then(|| {
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined)
    });
    let (controller, http, state, response) =
        ctx.with_readable_stream(stream_handle, |stream| {
            stream.disturbed = true;
            (
                stream.controller_handle,
                stream.http_response_handle,
                stream.state.clone(),
                (stream.response_body_handle, stream.response_body_object),
            )
        })?;
    ctx.mark_response_body_used(response.0, response.1);
    if let Some(controller) = controller {
        let chunk = ctx
            .with_stream_controller(controller, |entry| entry.chunk_queue.pop_front())
            .flatten();
        if let Some(chunk) = chunk {
            let promise = ctx.alloc_promise();
            if reader_kind == ReaderKind::Byob {
                if let Some(view) = byob_view {
                    fulfill_byob_read(ctx, controller, chunk, view, promise);
                } else {
                    reject_promise_with_type_error(ctx, promise, "BYOB read requires a view");
                }
            } else {
                let result = build_reader_result(ctx, false, Some(chunk));
                ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
            }
            return Some(promise);
        }
        let close_requested = ctx
            .with_stream_controller(controller, |entry| entry.close_requested)
            .unwrap_or(false);
        if close_requested || matches!(state, StreamState::Closed) {
            let promise = ctx.alloc_promise();
            let result = build_reader_result(ctx, true, byob_view);
            ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
            return Some(promise);
        }
        if matches!(state, StreamState::Errored) {
            let promise = ctx.alloc_promise();
            let error = ctx.make_type_error("Stream errored");
            ctx.settle_promise(promise, PromiseSettlement::Reject(error));
            return Some(promise);
        }
        let promise = ctx.alloc_promise();
        let _ = ctx.with_reader(handle, |reader| {
            reader.pending_read_promise = Some(promise);
            reader.pending_byob_view = byob_view;
        });
        if reader_kind == ReaderKind::Byob
            && let Some(view) = byob_view
        {
            let byob = ctx.alloc_byob_request(ByobRequestEntry {
                controller_handle: controller,
                reader_handle: handle,
                view,
                promise,
                responded: false,
            });
            let _ = ctx.with_stream_controller(controller, |entry| {
                entry.active_byob_request = Some(byob);
            });
        }
        let pull = ctx.with_stream_controller(controller, |entry| {
            entry
                .pull_callback
                .map(|callback| (callback, entry.underlying_source))
        });
        if let Some(Some((callback, this_value))) = pull {
            let controller_object = create_controller_object(ctx, controller);
            ctx.schedule_readable_pull(
                callback,
                this_value.unwrap_or_else(value::encode_undefined),
                controller_object,
            );
        }
        return Some(promise);
    }
    if let Some(http) = http {
        return ctx.fetch_body_reader_read(handle, http, byob_view);
    }
    let promise = ctx.alloc_promise();
    let result = build_reader_result(ctx, true, byob_view);
    ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
    Some(promise)
}

pub fn call_default_controller_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: ReadableStreamDefaultControllerMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        ReadableStreamDefaultControllerMethodKind::Enqueue => {
            super::readable::controller_enqueue(ctx, handle, args)
        }
        ReadableStreamDefaultControllerMethodKind::Close => {
            super::readable::controller_close(ctx, handle)
        }
        ReadableStreamDefaultControllerMethodKind::Error => {
            super::readable::controller_error(ctx, handle, args)
        }
        ReadableStreamDefaultControllerMethodKind::GetDesiredSize => Some(
            ctx.with_stream_controller(handle, |controller| {
                value::encode_f64(
                    controller.high_water_mark - controller.chunk_queue.len() as f64,
                )
            })
            .unwrap_or_else(value::encode_null),
        ),
        ReadableStreamDefaultControllerMethodKind::GetByobRequest => {
            Some(controller_get_byob_request(ctx, handle))
        }
    }
}

fn controller_get_byob_request<E: ExecContext>(ctx: &mut E, controller: u32) -> Value {
    let active = ctx
        .with_stream_controller(controller, |entry| entry.active_byob_request)
        .flatten();
    let Some(handle) = active else {
        return value::encode_null();
    };
    let request = ctx.with_byob_request(handle, |entry| (entry.view, entry.responded));
    let Some((_view, false)) = request else {
        return value::encode_null();
    };
    create_byob_request_object(ctx, handle)
}

pub fn call_byob_request_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: ReadableStreamByobRequestMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        ReadableStreamByobRequestMethodKind::GetView => ctx.with_byob_request(handle, |entry| {
            if entry.responded {
                value::encode_null()
            } else {
                entry.view
            }
        }),
        ReadableStreamByobRequestMethodKind::Respond => byob_respond(ctx, handle, args),
    }
}

fn byob_respond<E: ExecContext>(ctx: &mut E, handle: u32, args: &[Value]) -> Option<Value> {
    let bytes_written = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if !value::is_f64(bytes_written) {
        return Some(ctx.make_type_error("respond(bytesWritten) requires a number"));
    }
    let bytes_written = value::decode_f64(bytes_written);
    if !bytes_written.is_finite() || bytes_written.fract() != 0.0 || bytes_written < 0.0 {
        return Some(ctx.make_type_error("bytesWritten must be a non-negative integer"));
    }
    let bytes_written = bytes_written as usize;
    let request = ctx.with_byob_request(handle, |entry| {
        (
            entry.controller_handle,
            entry.reader_handle,
            entry.view,
            entry.promise,
            entry.responded,
        )
    });
    let Some((controller, reader, view, promise, responded)) = request else {
        return Some(ctx.make_type_error("invalid BYOB request"));
    };
    if responded {
        return Some(ctx.make_type_error("BYOB request already responded"));
    }
    let view_length = ctx
        .stream_typedarray_u8_bytes(view)
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    if bytes_written > view_length {
        return Some(ctx.make_type_error("bytesWritten exceeds view.byteLength"));
    }
    let result_view = ctx
        .stream_transfer_byob_view(view, bytes_written)
        .unwrap_or(view);
    let _ = ctx.with_byob_request(handle, |entry| entry.responded = true);
    let _ = ctx.with_stream_controller(controller, |entry| {
        if entry.active_byob_request == Some(handle) {
            entry.active_byob_request = None;
        }
    });
    let _ = ctx.with_reader(reader, |entry| {
        entry.pending_read_promise = None;
        entry.pending_byob_view = None;
    });
    let result = build_reader_result(ctx, false, Some(result_view));
    ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
    Some(value::encode_undefined())
}

pub fn async_iterator_return<E: ExecContext>(ctx: &mut E, reader_handle: u32) -> Value {
    if let Some(stream_handle) = ctx.with_reader(reader_handle, |reader| reader.stream_handle) {
        let _ = ctx.with_readable_stream(stream_handle, |stream| stream.locked = false);
    }
    let promise = ctx.alloc_promise();
    let result = build_reader_result(ctx, true, None);
    ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
    promise
}
