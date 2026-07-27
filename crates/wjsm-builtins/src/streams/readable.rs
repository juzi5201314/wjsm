use wjsm_host::{
    ExecContext, PromiseSettlement, ReadableStreamEntry, ReaderKind, StreamState, Value,
};
use wjsm_ir::value;

use super::{
    build_reader_result, create_controller_object, create_readable_stream_object,
    fulfill_byob_read, new_readable_controller, reject_promise_with_type_error,
};

pub async fn construct<E: ExecContext>(ctx: &mut E, args: &[Value]) -> Value {
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let is_byte_stream = if value::is_js_object(source) {
        let stream_type = ctx.read_property_by_string_key(source, "type");
        value::is_string(stream_type) && ctx.read_string_utf8_lossy(stream_type) == "bytes"
    } else {
        false
    };
    let high_water_mark = if value::is_js_object(strategy) {
        let value = ctx.read_property_by_string_key(strategy, "highWaterMark");
        let number = crate::core::to_number(ctx, value);
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

    let controller_handle = ctx.alloc_stream_controller(new_readable_controller(
        high_water_mark,
        false,
    ));
    let stream_handle = ctx.alloc_readable_stream(ReadableStreamEntry {
        state: StreamState::Readable,
        error: None,
        disturbed: false,
        locked: false,
        http_response_handle: None,
        response_body_handle: None,
        response_body_object: None,
        controller_handle: Some(controller_handle),
        is_byte_stream,
        pipe_to: None,
    });
    let _ = ctx.with_stream_controller(controller_handle, |controller| {
        controller.stream_handle = stream_handle;
    });
    let controller_object = create_controller_object(ctx, controller_handle);

    if value::is_js_object(source) {
        let pull = ctx.read_property_by_string_key(source, "pull");
        let cancel = ctx.read_property_by_string_key(source, "cancel");
        let pull = ctx.is_callable(pull).then_some(pull);
        let cancel = ctx.is_callable(cancel).then_some(cancel);
        let _ = ctx.with_stream_controller(controller_handle, |controller| {
            controller.underlying_source = Some(source);
            controller.pull_callback = pull;
            controller.cancel_callback = cancel;
        });
        let start = ctx.read_property_by_string_key(source, "start");
        if ctx.is_callable(start) {
            let arguments = [controller_object];
            let _ = ctx.call_js_async(start, source, &arguments).await;
        }
    }
    let _ = ctx.with_stream_controller(controller_handle, |controller| {
        controller.started = true;
    });
    create_readable_stream_object(ctx, stream_handle)
}

pub fn create_closed_from_bytes<E: ExecContext>(
    ctx: &mut E,
    bytes: &[u8],
    response_body_handle: Option<u32>,
    response_body_object: Option<Value>,
) -> (Value, u32) {
    let controller_handle =
        ctx.alloc_stream_controller(new_readable_controller(1.0, true));
    if !bytes.is_empty() {
        let chunk = ctx.stream_create_uint8array(bytes);
        let _ = ctx.with_stream_controller(controller_handle, |controller| {
            controller.chunk_queue.push_back(chunk);
        });
    }
    let stream_handle = ctx.alloc_readable_stream(ReadableStreamEntry {
        state: StreamState::Closed,
        error: None,
        disturbed: false,
        locked: false,
        http_response_handle: None,
        response_body_handle,
        response_body_object,
        controller_handle: Some(controller_handle),
        is_byte_stream: true,
        pipe_to: None,
    });
    let _ = ctx.with_stream_controller(controller_handle, |controller| {
        controller.stream_handle = stream_handle;
        controller.started = true;
    });
    (create_readable_stream_object(ctx, stream_handle), stream_handle)
}

pub fn controller_enqueue<E: ExecContext>(
    ctx: &mut E,
    controller_handle: u32,
    args: &[Value],
) -> Option<Value> {
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let (close_requested, stream_handle) = ctx.with_stream_controller(
        controller_handle,
        |controller| (controller.close_requested, controller.stream_handle),
    )?;
    if close_requested {
        return Some(ctx.make_type_error("Cannot enqueue to a closed stream"));
    }
    let state = ctx.with_readable_stream(stream_handle, |stream| stream.state.clone());
    if matches!(state, Some(StreamState::Closed | StreamState::Errored)) {
        return Some(ctx.make_type_error(
            "Cannot enqueue to a closed or errored stream",
        ));
    }
    let pending = ctx.with_readers(|readers| {
        readers.iter_mut().filter_map(Option::as_mut).find_map(|reader| {
            if reader.stream_handle != stream_handle {
                return None;
            }
            reader.pending_read_promise.take().map(|promise| {
                (reader.kind, reader.pending_byob_view.take(), promise)
            })
        })
    });
    if let Some((kind, view, promise)) = pending {
        if kind == ReaderKind::Byob {
            if let Some(view) = view {
                fulfill_byob_read(ctx, controller_handle, chunk, view, promise);
            } else {
                reject_promise_with_type_error(ctx, promise, "BYOB read requires a view");
            }
        } else {
            let result = build_reader_result(ctx, false, Some(chunk));
            ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
        }
    } else {
        let _ = ctx.with_stream_controller(controller_handle, |controller| {
            controller.chunk_queue.push_back(chunk);
        });
    }
    super::readable_pipe::pump_readable_stream_pipe_to(ctx, stream_handle);
    Some(value::encode_undefined())
}

pub fn controller_close<E: ExecContext>(
    ctx: &mut E,
    controller_handle: u32,
) -> Option<Value> {
    let (already_closed, stream_handle) = ctx.with_stream_controller(
        controller_handle,
        |controller| {
            let already_closed = controller.close_requested;
            controller.close_requested = true;
            (already_closed, controller.stream_handle)
        },
    )?;
    if already_closed {
        return Some(ctx.make_type_error("The stream has already been closed"));
    }
    let _ = ctx.with_readable_stream(stream_handle, |stream| {
        stream.state = StreamState::Closed;
    });
    let pending = ctx.with_readers(|readers| {
        readers.iter_mut().filter_map(Option::as_mut).find_map(|reader| {
            if reader.stream_handle != stream_handle {
                return None;
            }
            reader
                .pending_read_promise
                .take()
                .map(|promise| (reader.pending_byob_view.take(), promise))
        })
    });
    if let Some((view, promise)) = pending {
        let result = build_reader_result(ctx, true, view);
        ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
    }
    super::readable_pipe::pump_readable_stream_pipe_to(ctx, stream_handle);
    Some(value::encode_undefined())
}

pub fn controller_error<E: ExecContext>(
    ctx: &mut E,
    controller_handle: u32,
    args: &[Value],
) -> Option<Value> {
    let error = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let stream_handle = ctx.with_stream_controller(controller_handle, |controller| {
        controller.stream_handle
    })?;
    let _ = ctx.with_readable_stream(stream_handle, |stream| {
        stream.state = StreamState::Errored;
        if value::is_string(error) {
            stream.error = Some("stream error".to_string());
        }
    });
    let pending = ctx.with_readers(|readers| {
        readers.iter_mut().filter_map(Option::as_mut).find_map(|reader| {
            (reader.stream_handle == stream_handle)
                .then(|| reader.pending_read_promise.take())
                .flatten()
        })
    });
    if let Some(promise) = pending {
        ctx.settle_promise(promise, PromiseSettlement::Reject(error));
    }
    Some(value::encode_undefined())
}

