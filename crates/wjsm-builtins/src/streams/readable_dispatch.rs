use wjsm_host::{
    ExecContext, NativeCallableRef, PromiseSettlement, ReadableStreamDefaultReaderMethodKind,
    ReadableStreamEntry, ReadableStreamMethodKind, ReaderEntry, ReaderKind, StreamState, Value,
};
use wjsm_ir::{constants, value};

use super::{
    create_readable_stream_object, define_accessor_property, define_data_property_with_flags,
    new_readable_controller,
};

fn create_reader_object<E: ExecContext>(
    ctx: &mut E,
    reader_handle: u32,
    closed_promise: Value,
) -> Value {
    let object = ctx.alloc_object(5);
    define_data_property_with_flags(
        ctx,
        object,
        "__reader_handle__",
        value::encode_f64(reader_handle as f64),
        constants::FLAG_PRIVATE,
    );
    for (name, kind) in [
        ("read", ReadableStreamDefaultReaderMethodKind::Read),
        (
            "releaseLock",
            ReadableStreamDefaultReaderMethodKind::ReleaseLock,
        ),
    ] {
        let callable =
            ctx.create_native_callable(NativeCallableRef::ReadableStreamDefaultReaderMethod {
                handle: reader_handle,
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
    let closed = ctx.create_native_callable(NativeCallableRef::ReadableStreamDefaultReaderMethod {
        handle: reader_handle,
        kind: ReadableStreamDefaultReaderMethodKind::GetClosed,
    });
    define_accessor_property(ctx, object, "closed", closed);
    define_data_property_with_flags(
        ctx,
        object,
        "__closed_promise__",
        closed_promise,
        constants::FLAG_PRIVATE,
    );
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_reader_object(object_handle, reader_handle);
    }
    object
}

pub fn call_readable_stream_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: ReadableStreamMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        ReadableStreamMethodKind::GetLocked => Some(value::encode_bool(
            ctx.with_readable_stream(handle, |stream| stream.locked)
                .unwrap_or(false),
        )),
        ReadableStreamMethodKind::GetReader => get_reader(ctx, handle, args),
        ReadableStreamMethodKind::Cancel => cancel(ctx, handle),
        ReadableStreamMethodKind::Tee => tee(ctx, handle),
        ReadableStreamMethodKind::AsyncIterator => async_iterator(ctx, handle),
        ReadableStreamMethodKind::PipeTo => {
            let destination = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            super::readable_pipe::readable_stream_pipe_to(ctx, handle, destination)
        }
        ReadableStreamMethodKind::PipeThrough => {
            let transform = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            super::readable_pipe::readable_stream_pipe_through(ctx, handle, transform)
        }
    }
}

fn get_reader<E: ExecContext>(ctx: &mut E, handle: u32, args: &[Value]) -> Option<Value> {
    let options = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let wants_byob = if value::is_js_object(options) {
        let mode = ctx.read_property_by_string_key(options, "mode");
        value::is_string(mode) && ctx.read_string_utf8_lossy(mode) == "byob"
    } else {
        false
    };
    let (locked, is_byte_stream) = ctx.with_readable_stream(handle, |stream| {
        let locked = stream.locked;
        if !locked && (!wants_byob || stream.is_byte_stream) {
            stream.locked = true;
        }
        (locked, stream.is_byte_stream)
    })?;
    if locked {
        return Some(ctx.make_type_error("ReadableStream is already locked to a reader"));
    }
    if wants_byob && !is_byte_stream {
        return Some(ctx.make_type_error("ReadableStreamBYOBReader requires a byte stream"));
    }
    let closed_promise = ctx.alloc_promise();
    let reader_handle = ctx.alloc_reader(ReaderEntry {
        stream_handle: handle,
        kind: if wants_byob {
            ReaderKind::Byob
        } else {
            ReaderKind::Default
        },
        pending_read_promise: None,
        pending_byob_view: None,
        closed_promise: Some(closed_promise),
    });
    if matches!(
        ctx.with_readable_stream(handle, |stream| stream.state.clone()),
        Some(StreamState::Closed)
    ) {
        ctx.settle_promise(
            closed_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
    Some(create_reader_object(ctx, reader_handle, closed_promise))
}

fn cancel<E: ExecContext>(ctx: &mut E, handle: u32) -> Option<Value> {
    let (controller, http, response) = ctx.with_readable_stream(handle, |stream| {
        stream.state = StreamState::Closed;
        stream.disturbed = true;
        (
            stream.controller_handle,
            stream.http_response_handle,
            (stream.response_body_handle, stream.response_body_object),
        )
    })?;
    ctx.mark_response_body_used(response.0, response.1);
    if let Some(http) = http {
        ctx.cancel_http_response(http);
    }
    if let Some(controller) = controller {
        let _ = ctx.with_stream_controller(controller, |controller| {
            controller.chunk_queue.clear();
            controller.close_requested = true;
        });
    }
    let promise = ctx.alloc_promise();
    ctx.settle_promise(
        promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
    Some(promise)
}

fn tee<E: ExecContext>(ctx: &mut E, handle: u32) -> Option<Value> {
    let (state, controller_handle, is_byte_stream) =
        ctx.with_readable_stream(handle, |stream| {
            if stream.locked {
                return None;
            }
            stream.disturbed = true;
            stream.locked = true;
            Some((
                stream.state.clone(),
                stream.controller_handle,
                stream.is_byte_stream,
            ))
        })??;
    let controller_handle = controller_handle?;
    let (queue, high_water_mark, strategy_size) =
        ctx.with_stream_controller(controller_handle, |controller| {
            (
                controller.chunk_queue.clone(),
                controller.high_water_mark,
                controller.strategy_size,
            )
        })?;
    let mut first_controller =
        new_readable_controller(high_water_mark, matches!(state, StreamState::Closed));
    first_controller.chunk_queue = queue.clone();
    first_controller.strategy_size = strategy_size;
    first_controller.started = true;
    let first_controller_handle = ctx.alloc_stream_controller(first_controller);
    let mut second_controller =
        new_readable_controller(high_water_mark, matches!(state, StreamState::Closed));
    second_controller.chunk_queue = queue;
    second_controller.strategy_size = strategy_size;
    second_controller.started = true;
    let second_controller_handle = ctx.alloc_stream_controller(second_controller);

    let create_branch = |controller_handle| ReadableStreamEntry {
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
    };
    let first_handle = ctx.alloc_readable_stream(create_branch(first_controller_handle));
    let second_handle = ctx.alloc_readable_stream(create_branch(second_controller_handle));
    let _ = ctx.with_stream_controller(first_controller_handle, |controller| {
        controller.stream_handle = first_handle;
    });
    let _ = ctx.with_stream_controller(second_controller_handle, |controller| {
        controller.stream_handle = second_handle;
    });
    let first = create_readable_stream_object(ctx, first_handle);
    let second = create_readable_stream_object(ctx, second_handle);
    let result = ctx.alloc_array(2);
    ctx.array_write_elem(result, 0, first);
    ctx.array_write_elem(result, 1, second);
    Some(result)
}

fn async_iterator<E: ExecContext>(ctx: &mut E, handle: u32) -> Option<Value> {
    let state = ctx.with_readable_stream(handle, |stream| {
        if stream.locked {
            return None;
        }
        stream.locked = true;
        Some(stream.state.clone())
    })??;
    let closed_promise = ctx.alloc_promise();
    let reader_handle = ctx.alloc_reader(ReaderEntry {
        stream_handle: handle,
        kind: ReaderKind::Default,
        pending_read_promise: None,
        pending_byob_view: None,
        closed_promise: Some(closed_promise),
    });
    if matches!(state, StreamState::Closed) {
        ctx.settle_promise(
            closed_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
    let iterator = ctx.alloc_object(2);
    for (name, callable) in [
        (
            "next",
            NativeCallableRef::ReadableStreamAsyncIteratorNext { reader_handle },
        ),
        (
            "return",
            NativeCallableRef::ReadableStreamAsyncIteratorReturn { reader_handle },
        ),
    ] {
        let callable = ctx.create_native_callable(callable);
        define_data_property_with_flags(
            ctx,
            iterator,
            name,
            callable,
            constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
        );
    }
    Some(iterator)
}

pub(crate) fn writable_stream_handle_from_object<E: ExecContext>(
    ctx: &mut E,
    writable: Value,
) -> Option<u32> {
    let raw = ctx.read_property_by_string_key(writable, "__writable_stream_handle__");
    value::is_f64(raw).then(|| value::decode_f64(raw) as u32)
}

pub(crate) fn transform_parts_from_object<E: ExecContext>(
    ctx: &mut E,
    transform: Value,
) -> Option<(Value, Value)> {
    let raw = ctx.read_property_by_string_key(transform, "__transform_stream_handle__");
    if value::is_f64(raw) {
        let handle = value::decode_f64(raw) as u32;
        return ctx
            .with_transform_stream(handle, |entry| entry.readable_obj.zip(entry.writable_obj))
            .flatten();
    }
    let readable = ctx.read_property_by_string_key(transform, "readable");
    let writable = ctx.read_property_by_string_key(transform, "writable");
    Some((readable, writable))
}
