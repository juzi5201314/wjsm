use std::collections::VecDeque;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{
    AsyncIteratorState, ByobMethod, ByobState, ControllerMethod, ControllerState, ObjectKind,
    PendingRead, PipeState, ReadableMethod, ReadableState, ReadableStatus, ReaderKind,
    ReaderMethod, ReaderState, StreamCallable, StreamProperty, StreamReaction, StreamTask,
    callable_property, new_promise, read_named, register_object, resolved, result_object,
    type_error,
};
use crate::NativeAgentState;

pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let byte_stream = if value::is_js_object(source) {
        let stream_type = read_named(ctx, state, source, "type");
        state
            .string_owned(stream_type)
            .and_then(|text| text.to_utf8())
            .is_some_and(|kind| kind == "bytes")
    } else {
        false
    };
    let high_water_mark = if value::is_js_object(strategy) {
        let raw = read_named(ctx, state, strategy, "highWaterMark");
        super::super::runtime::to_number(state, raw).unwrap_or(if byte_stream { 0.0 } else { 1.0 })
    } else if byte_stream {
        0.0
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
    let pull = value::is_js_object(source)
        .then(|| callable_property(ctx, state, source, "pull"))
        .flatten();
    let cancel = value::is_js_object(source)
        .then(|| callable_property(ctx, state, source, "cancel"))
        .flatten();
    let Some((stream, controller)) = create_stream(
        state,
        high_water_mark,
        byte_stream,
        source,
        pull,
        cancel,
        false,
    ) else {
        return super::super::fail_dispatch(ctx);
    };
    if value::is_js_object(source) {
        let start = callable_property(ctx, state, source, "start");
        if let Some(start) = start {
            let result = state
                .invoke_callable(ctx, start, source, &[controller])
                .unwrap_or_else(|| super::super::fail_dispatch(ctx));
            if value::is_exception(result) {
                error_stream(state, value::decode_handle(stream), result);
                return result;
            }
        }
    }
    stream
}

pub(super) fn from_bytes(
    _ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    bytes: &[u8],
) -> Option<(i64, u32)> {
    let (stream, _) = create_stream(
        state,
        1.0,
        true,
        value::encode_undefined(),
        None,
        None,
        true,
    )?;
    let ObjectKind::Readable(stream_handle) =
        *state.streams.objects.get(&value::decode_handle(stream))?
    else {
        return None;
    };
    let controller = state
        .streams
        .readables
        .get(stream_handle as usize)?
        .controller;
    if !bytes.is_empty() {
        let chunk = super::super::typedarray::create_uint8_array(state, bytes)?;
        state
            .streams
            .controllers
            .get_mut(controller as usize)?
            .queue
            .push_back(chunk);
    }
    Some((stream, stream_handle))
}

pub(super) fn create_stream(
    state: &mut NativeAgentState,
    high_water_mark: f64,
    byte_stream: bool,
    source: i64,
    pull: Option<i64>,
    cancel: Option<i64>,
    close_requested: bool,
) -> Option<(i64, i64)> {
    let stream_object = state.allocate_object(8, false).ok()?;
    let controller_object = state.allocate_object(6, false).ok()?;
    let stream_handle = state.streams.readables.len() as u32;
    let controller_handle = state.streams.controllers.len() as u32;
    state.streams.readables.push(ReadableState {
        object: stream_object,
        controller: controller_handle,
        status: ReadableStatus::Readable,
        error: None,
        locked: false,
        response: None,
        pipe: None,
    });
    state.streams.controllers.push(ControllerState {
        object: controller_object,
        readable: stream_handle,
        queue: VecDeque::new(),
        high_water_mark,
        close_requested,
        byte_stream,
        source,
        pull,
        cancel,
        active_byob: None,
        pulling: false,
    });
    register_object(state, stream_object, ObjectKind::Readable(stream_handle));
    register_object(
        state,
        controller_object,
        ObjectKind::Controller(controller_handle),
    );
    Some((stream_object, controller_object))
}

pub(super) fn readable_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let stream = state.streams.readables.get(handle as usize)?;
    let method = match key {
        "cancel" => ReadableMethod::Cancel,
        "getReader" => ReadableMethod::GetReader,
        "pipeThrough" => ReadableMethod::PipeThrough,
        "pipeTo" => ReadableMethod::PipeTo,
        "locked" => {
            return Some(StreamProperty::Value(value::encode_bool(stream.locked)));
        }
        _ => return None,
    };
    Some(StreamProperty::Callable(StreamCallable::Readable(
        handle, method,
    )))
}

pub(super) fn reader_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let reader = state.streams.readers.get(handle as usize)?;
    let method = match key {
        "closed" => {
            return Some(StreamProperty::Value(value::encode_object_handle(
                reader.closed_promise,
            )));
        }
        "read" => ReaderMethod::Read,
        "releaseLock" => ReaderMethod::ReleaseLock,
        _ => return None,
    };
    Some(StreamProperty::Callable(StreamCallable::Reader(
        handle, method,
    )))
}

pub(super) fn controller_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let controller = state.streams.controllers.get(handle as usize)?;
    match key {
        "byobRequest" => Some(StreamProperty::Value(
            controller
                .active_byob
                .and_then(|request| state.streams.byob_requests.get(request as usize))
                .map_or_else(value::encode_null, |request| request.object),
        )),
        "close" => Some(StreamProperty::Callable(StreamCallable::Controller(
            handle,
            ControllerMethod::Close,
        ))),
        "desiredSize" => Some(StreamProperty::Value(value::encode_f64(
            controller.high_water_mark - controller.queue.len() as f64,
        ))),
        "enqueue" => Some(StreamProperty::Callable(StreamCallable::Controller(
            handle,
            ControllerMethod::Enqueue,
        ))),
        "error" => Some(StreamProperty::Callable(StreamCallable::Controller(
            handle,
            ControllerMethod::Error,
        ))),
        _ => None,
    }
}

pub(super) fn byob_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<StreamProperty> {
    let request = state.streams.byob_requests.get(handle as usize)?;
    match key {
        "respond" => Some(StreamProperty::Callable(StreamCallable::Byob(
            handle,
            ByobMethod::Respond,
        ))),
        "view" => Some(StreamProperty::Value(request.view)),
        _ => None,
    }
}

pub(super) fn async_iterator_property(handle: u32, key: &str) -> Option<StreamProperty> {
    let callable = match key {
        "next" => StreamCallable::AsyncIteratorNext(handle),
        "return" => StreamCallable::AsyncIteratorReturn(handle),
        _ => return None,
    };
    Some(StreamProperty::Callable(callable))
}

pub(super) fn call_readable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: ReadableMethod,
    args: &[i64],
) -> i64 {
    match method {
        ReadableMethod::Cancel => cancel(ctx, state, handle, args),
        ReadableMethod::GetReader => get_reader(ctx, state, handle, args),
        ReadableMethod::PipeThrough => pipe_through(ctx, state, handle, args),
        ReadableMethod::PipeTo => pipe_to(ctx, state, handle, args),
    }
}

pub(super) fn call_reader(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: ReaderMethod,
    args: &[i64],
) -> i64 {
    match method {
        ReaderMethod::Read => read(ctx, state, handle, args.first().copied()),
        ReaderMethod::ReleaseLock => release_reader(state, handle),
    }
}

pub(super) fn call_controller(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: ControllerMethod,
    args: &[i64],
) -> i64 {
    match method {
        ControllerMethod::Close => close(ctx, state, handle),
        ControllerMethod::Enqueue => enqueue(ctx, state, handle, args),
        ControllerMethod::Error => {
            let error = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let stream = state
                .streams
                .controllers
                .get(handle as usize)
                .map(|controller| controller.readable);
            if let Some(stream) = stream {
                error_stream(state, stream, error);
                value::encode_undefined()
            } else {
                super::super::fail_dispatch(ctx)
            }
        }
    }
}

pub(super) fn call_byob(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: ByobMethod,
    args: &[i64],
) -> i64 {
    match method {
        ByobMethod::Respond => respond(ctx, state, handle, args),
    }
}

fn get_reader(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    args: &[i64],
) -> i64 {
    let wants_byob = args.first().copied().is_some_and(|options| {
        if !value::is_js_object(options) {
            return false;
        }
        let mode = read_named(ctx, state, options, "mode");
        state
            .string_owned(mode)
            .and_then(|text| text.to_utf8())
            .is_some_and(|mode| mode == "byob")
    });
    let Some((locked, byte_stream, status)) =
        state.streams.readables.get(stream as usize).map(|entry| {
            (
                entry.locked,
                state.streams.controllers[entry.controller as usize].byte_stream,
                entry.status,
            )
        })
    else {
        return super::super::fail_dispatch(ctx);
    };
    if locked {
        return type_error(ctx, state, "ReadableStream is already locked");
    }
    if wants_byob && !byte_stream {
        return type_error(ctx, state, "BYOB reader requires a byte stream");
    }
    let Some((_, closed_promise)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    if status == ReadableStatus::Closed {
        super::super::promise::settle_promise(
            state,
            closed_promise,
            value::encode_undefined(),
            false,
        );
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 5, false) else {
        return super::super::fail_dispatch(ctx);
    };
    let reader = state.streams.readers.len() as u32;
    state.streams.readers.push(ReaderState {
        stream,
        kind: if wants_byob {
            ReaderKind::Byob
        } else {
            ReaderKind::Default
        },
        closed_promise,
        pending: VecDeque::new(),
    });
    state.streams.readables[stream as usize].locked = true;
    register_object(state, object, ObjectKind::Reader(reader));
    object
}

fn read(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    reader: u32,
    view: Option<i64>,
) -> i64 {
    let Some((stream, kind)) = state
        .streams
        .readers
        .get(reader as usize)
        .map(|reader| (reader.stream, reader.kind))
    else {
        return super::super::fail_dispatch(ctx);
    };
    if let Some(response) = state.streams.readables[stream as usize].response {
        super::super::fetch::mark_response_used(state, response);
    }
    if kind == ReaderKind::Byob
        && !view.is_some_and(|view| super::super::typedarray::byte_length(state, view).is_some())
    {
        return type_error(ctx, state, "BYOB read requires a typed array view");
    }
    let Some((promise, promise_handle)) = new_promise(ctx, state) else {
        return super::super::fail_dispatch(ctx);
    };
    let controller = state.streams.readables[stream as usize].controller;
    if let Some(chunk) = state.streams.controllers[controller as usize]
        .queue
        .pop_front()
    {
        let stored = if let Some(view) = view {
            copy_chunk_to_view(state, controller, chunk, view).unwrap_or(view)
        } else {
            chunk
        };
        let result = result_object(ctx, state, false, stored);
        super::super::promise::settle_promise(state, promise_handle, result, false);
        close_if_drained(state, stream);
        if state.streams.readables[stream as usize].status == ReadableStatus::Closed
            && let Some(response) = state.streams.readables[stream as usize].response
        {
            super::super::fetch::complete_response_body(ctx, state, response);
        }
        return promise;
    }
    match state.streams.readables[stream as usize].status {
        ReadableStatus::Closed => {
            let result = result_object(ctx, state, true, value::encode_undefined());
            super::super::promise::settle_promise(state, promise_handle, result, false);
            if let Some(response) = state.streams.readables[stream as usize].response {
                super::super::fetch::complete_response_body(ctx, state, response);
            }
        }
        ReadableStatus::Errored => {
            let reason = state.streams.readables[stream as usize]
                .error
                .unwrap_or_else(value::encode_undefined);
            super::super::promise::settle_promise(state, promise_handle, reason, true);
            if let Some(response) = state.streams.readables[stream as usize].response {
                super::super::fetch::complete_response_body(ctx, state, response);
            }
        }
        ReadableStatus::Readable => {
            state.streams.readers[reader as usize]
                .pending
                .push_back(PendingRead {
                    promise: promise_handle,
                    view,
                });
            if let Some(view) = view {
                create_byob_request(ctx, state, controller, reader, view, promise_handle);
            }
            schedule_pull(state, controller);
        }
    }
    promise
}

fn create_byob_request(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    controller: u32,
    reader: u32,
    view: i64,
    promise: u32,
) {
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 3, false) else {
        let reason = super::super::fail_dispatch(ctx);
        super::super::promise::settle_promise(state, promise, reason, true);
        return;
    };
    let handle = state.streams.byob_requests.len() as u32;
    state.streams.byob_requests.push(ByobState {
        object,
        controller,
        reader,
        view,
        promise,
        responded: false,
    });
    state.streams.controllers[controller as usize].active_byob = Some(handle);
    register_object(state, object, ObjectKind::Byob(handle));
}

fn respond(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    request: u32,
    args: &[i64],
) -> i64 {
    let count = args
        .first()
        .and_then(|count| super::super::runtime::to_number(state, *count))
        .filter(|count| count.is_finite() && *count >= 0.0 && count.fract() == 0.0)
        .map(|count| count as usize);
    let Some(count) = count else {
        return type_error(
            ctx,
            state,
            "BYOB respond count must be a non-negative integer",
        );
    };
    let Some((controller, reader, view, promise, responded)) = state
        .streams
        .byob_requests
        .get(request as usize)
        .map(|entry| {
            (
                entry.controller,
                entry.reader,
                entry.view,
                entry.promise,
                entry.responded,
            )
        })
    else {
        return super::super::fail_dispatch(ctx);
    };
    let Some(capacity) = super::super::typedarray::byte_length(state, view) else {
        return type_error(ctx, state, "BYOB request view is invalid");
    };
    if responded || count > capacity {
        return type_error(ctx, state, "BYOB response exceeds the supplied view");
    }
    let Some(result_view) = super::super::typedarray::prefix_view(state, view, count) else {
        return super::super::fail_dispatch(ctx);
    };
    state.streams.byob_requests[request as usize].responded = true;
    state.streams.controllers[controller as usize].active_byob = None;
    if let Some(position) = state.streams.readers[reader as usize]
        .pending
        .iter()
        .position(|pending| pending.promise == promise)
    {
        state.streams.readers[reader as usize]
            .pending
            .remove(position);
    }
    let result = result_object(ctx, state, false, result_view);
    super::super::promise::settle_promise(state, promise, result, false);
    let stream = state.streams.controllers[controller as usize].readable;
    close_if_drained(state, stream);
    value::encode_undefined()
}

fn enqueue(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    controller: u32,
    args: &[i64],
) -> i64 {
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(stream) = state
        .streams
        .controllers
        .get(controller as usize)
        .map(|controller| controller.readable)
    else {
        return super::super::fail_dispatch(ctx);
    };
    if state.streams.controllers[controller as usize].close_requested
        || state.streams.readables[stream as usize].status != ReadableStatus::Readable
    {
        return type_error(ctx, state, "Cannot enqueue into a closed stream");
    }
    let pending_reader = state
        .streams
        .readers
        .iter()
        .enumerate()
        .find(|(_, reader)| reader.stream == stream && !reader.pending.is_empty())
        .map(|(index, _)| index);
    if let Some(reader) = pending_reader {
        let pending = state.streams.readers[reader]
            .pending
            .pop_front()
            .expect("pending reader was selected");
        let stored = if let Some(view) = pending.view {
            copy_chunk_to_view(state, controller, chunk, view).unwrap_or(view)
        } else {
            chunk
        };
        state.streams.controllers[controller as usize].active_byob = None;
        let result = result_object(ctx, state, false, stored);
        super::super::promise::settle_promise(state, pending.promise, result, false);
    } else {
        state.streams.controllers[controller as usize]
            .queue
            .push_back(chunk);
    }
    if state.streams.readables[stream as usize].pipe.is_some() {
        super::super::promise::enqueue_stream_task(state, StreamTask::Pump { readable: stream });
    }
    value::encode_undefined()
}

fn close(ctx: &mut NativeVmContext, state: &mut NativeAgentState, controller: u32) -> i64 {
    let Some(stream) = state
        .streams
        .controllers
        .get(controller as usize)
        .map(|controller| controller.readable)
    else {
        return super::super::fail_dispatch(ctx);
    };
    if state.streams.controllers[controller as usize].close_requested {
        return type_error(ctx, state, "ReadableStream is already closing");
    }
    state.streams.controllers[controller as usize].close_requested = true;
    close_if_drained(state, stream);
    if state.streams.readables[stream as usize].pipe.is_some() {
        super::super::promise::enqueue_stream_task(state, StreamTask::Pump { readable: stream });
    }
    value::encode_undefined()
}

fn close_if_drained(state: &mut NativeAgentState, stream: u32) {
    let controller = state.streams.readables[stream as usize].controller;
    if !state.streams.controllers[controller as usize].close_requested
        || !state.streams.controllers[controller as usize]
            .queue
            .is_empty()
        || state.streams.controllers[controller as usize]
            .active_byob
            .is_some()
    {
        return;
    }
    state.streams.readables[stream as usize].status = ReadableStatus::Closed;
    let readers: Vec<_> = state
        .streams
        .readers
        .iter()
        .enumerate()
        .filter(|(_, reader)| reader.stream == stream)
        .map(|(index, _)| index)
        .collect();
    for reader in readers {
        let closed = state.streams.readers[reader].closed_promise;
        let pending: Vec<_> = state.streams.readers[reader]
            .pending
            .drain(..)
            .map(|pending| pending.promise)
            .collect();
        super::super::promise::settle_promise(state, closed, value::encode_undefined(), false);
        for promise in pending {
            let result = closed_result(state);
            super::super::promise::settle_promise(state, promise, result, false);
        }
    }
}

fn closed_result(state: &mut NativeAgentState) -> i64 {
    let object = state.allocate_object(2, false).ok();
    let Some(object) = object else {
        return value::encode_undefined();
    };
    let _ =
        super::super::modules::set_named_property(state, object, "done", value::encode_bool(true));
    let _ = super::super::modules::set_named_property(
        state,
        object,
        "value",
        value::encode_undefined(),
    );
    object
}

fn error_stream(state: &mut NativeAgentState, stream: u32, reason: i64) {
    if let Some(stream) = state.streams.readables.get_mut(stream as usize) {
        stream.status = ReadableStatus::Errored;
        stream.error = Some(reason);
    }
    let reader_handles: Vec<_> = state
        .streams
        .readers
        .iter()
        .enumerate()
        .filter(|(_, reader)| reader.stream == stream)
        .map(|(index, _)| index)
        .collect();
    for reader in reader_handles {
        let closed = state.streams.readers[reader].closed_promise;
        let pending: Vec<_> = state.streams.readers[reader]
            .pending
            .drain(..)
            .map(|pending| pending.promise)
            .collect();
        super::super::promise::settle_promise(state, closed, reason, true);
        for promise in pending {
            super::super::promise::settle_promise(state, promise, reason, true);
        }
    }
}

fn cancel(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
    args: &[i64],
) -> i64 {
    let reason = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some((controller, response)) = state
        .streams
        .readables
        .get(stream as usize)
        .map(|stream| (stream.controller, stream.response))
    else {
        return super::super::fail_dispatch(ctx);
    };
    state.streams.controllers[controller as usize].queue.clear();
    state.streams.controllers[controller as usize].close_requested = true;
    close_if_drained(state, stream);
    if let Some(response) = response {
        super::super::fetch::mark_response_used(state, response);
        super::super::fetch::complete_response_body(ctx, state, response);
    }
    let callback = state.streams.controllers[controller as usize].cancel;
    let source = state.streams.controllers[controller as usize].source;
    if let Some(callback) = callback {
        let result = state
            .invoke_callable(ctx, callback, source, &[reason])
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

fn release_reader(state: &mut NativeAgentState, reader: u32) -> i64 {
    if let Some(stream) = state
        .streams
        .readers
        .get(reader as usize)
        .map(|reader| reader.stream)
        && let Some(stream) = state.streams.readables.get_mut(stream as usize)
    {
        stream.locked = false;
    }
    value::encode_undefined()
}

pub(super) fn create_async_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stream: u32,
) -> i64 {
    let reader_object = get_reader(ctx, state, stream, &[]);
    if value::is_exception(reader_object) {
        return reader_object;
    }
    let Some(ObjectKind::Reader(reader)) = state
        .streams
        .objects
        .get(&value::decode_handle(reader_object))
        .copied()
    else {
        return super::super::fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 3, false) else {
        return super::super::fail_dispatch(ctx);
    };
    let iterator = state.streams.async_iterators.len() as u32;
    state
        .streams
        .async_iterators
        .push(AsyncIteratorState { object, reader });
    register_object(state, object, ObjectKind::AsyncIterator(iterator));
    object
}

pub(super) fn async_iterator_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: u32,
) -> i64 {
    let Some(reader) = state
        .streams
        .async_iterators
        .get(iterator as usize)
        .map(|iterator| iterator.reader)
    else {
        return super::super::fail_dispatch(ctx);
    };
    read(ctx, state, reader, None)
}

pub(super) fn async_iterator_return(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: u32,
) -> i64 {
    let Some(reader) = state
        .streams
        .async_iterators
        .get(iterator as usize)
        .map(|iterator| iterator.reader)
    else {
        return super::super::fail_dispatch(ctx);
    };
    release_reader(state, reader);
    let result = result_object(ctx, state, true, value::encode_undefined());
    resolved(ctx, state, result)
}

fn pipe_through(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    readable: u32,
    args: &[i64],
) -> i64 {
    let Some(pair) = args
        .first()
        .copied()
        .filter(|pair| value::is_js_object(*pair))
    else {
        return type_error(ctx, state, "pipeThrough requires a readable/writable pair");
    };
    let readable_object = read_named(ctx, state, pair, "readable");
    let writable_object = read_named(ctx, state, pair, "writable");
    if pipe_to_object(ctx, state, readable, writable_object).is_none() {
        return type_error(ctx, state, "pipeThrough writable is invalid");
    }
    readable_object
}

fn pipe_to(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    readable: u32,
    args: &[i64],
) -> i64 {
    let destination = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    pipe_to_object(ctx, state, readable, destination)
        .unwrap_or_else(|| type_error(ctx, state, "pipeTo destination must be a WritableStream"))
}

fn pipe_to_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    readable: u32,
    destination: i64,
) -> Option<i64> {
    let ObjectKind::Writable(destination) = *state
        .streams
        .objects
        .get(&value::decode_handle(destination))?
    else {
        return None;
    };
    if state
        .streams
        .readables
        .get(readable as usize)?
        .pipe
        .is_some()
        || state.streams.readables[readable as usize].locked
    {
        return None;
    }
    let (promise, promise_handle) = new_promise(ctx, state)?;
    state.streams.readables[readable as usize].locked = true;
    state.streams.readables[readable as usize].pipe = Some(PipeState {
        destination,
        promise: promise_handle,
        writing: false,
        closing: false,
    });
    super::super::promise::enqueue_stream_task(state, StreamTask::Pump { readable });
    Some(promise)
}

pub(super) fn pump(ctx: &mut NativeVmContext, state: &mut NativeAgentState, readable: u32) -> i64 {
    let Some((controller, destination, writing, closing, pipe_promise)) = state
        .streams
        .readables
        .get(readable as usize)
        .and_then(|stream| {
            stream.pipe.as_ref().map(|pipe| {
                (
                    stream.controller,
                    pipe.destination,
                    pipe.writing,
                    pipe.closing,
                    pipe.promise,
                )
            })
        })
    else {
        return value::encode_undefined();
    };
    if writing {
        return value::encode_undefined();
    }
    if let Some(chunk) = state.streams.controllers[controller as usize]
        .queue
        .pop_front()
    {
        state.streams.readables[readable as usize]
            .pipe
            .as_mut()
            .expect("pipe exists")
            .writing = true;
        let write = super::writable::start_pipe_write(ctx, state, destination, chunk);
        super::super::promise::observe(
            state,
            value::decode_handle(write),
            StreamReaction::Pump { readable },
        );
        return value::encode_undefined();
    }
    if state.streams.readables[readable as usize].status == ReadableStatus::Errored {
        let reason = state.streams.readables[readable as usize]
            .error
            .unwrap_or_else(value::encode_undefined);
        super::super::promise::settle_promise(state, pipe_promise, reason, true);
        state.streams.readables[readable as usize].pipe = None;
        return value::encode_undefined();
    }
    let source_closed = state.streams.controllers[controller as usize].close_requested;
    if source_closed && !closing {
        if let Some(pipe) = state.streams.readables[readable as usize].pipe.as_mut() {
            pipe.writing = true;
            pipe.closing = true;
        }
        super::writable::start_pipe_close(ctx, state, destination, readable);
    }
    value::encode_undefined()
}

pub(super) fn finish_pipe_write(
    _ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    readable: u32,
    stored: i64,
    rejected: bool,
) -> i64 {
    let Some((promise, closing)) = state
        .streams
        .readables
        .get_mut(readable as usize)
        .and_then(|stream| stream.pipe.as_mut())
        .map(|pipe| {
            pipe.writing = false;
            (pipe.promise, pipe.closing)
        })
    else {
        return value::encode_undefined();
    };
    if rejected {
        super::super::promise::settle_promise(state, promise, stored, true);
        state.streams.readables[readable as usize].pipe = None;
    } else if closing {
        super::super::promise::settle_promise(state, promise, value::encode_undefined(), false);
        state.streams.readables[readable as usize].pipe = None;
    } else {
        super::super::promise::enqueue_stream_task(state, StreamTask::Pump { readable });
    }
    value::encode_undefined()
}

pub(super) fn run_pull(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    controller: u32,
) -> i64 {
    let Some((callback, source, controller_object)) = state
        .streams
        .controllers
        .get_mut(controller as usize)
        .and_then(|entry| {
            entry.pulling = false;
            entry
                .pull
                .map(|callback| (callback, entry.source, entry.object))
        })
    else {
        return value::encode_undefined();
    };
    let result = state
        .invoke_callable(ctx, callback, source, &[controller_object])
        .unwrap_or_else(|| super::super::fail_dispatch(ctx));
    if value::is_exception(result) {
        let stream = state.streams.controllers[controller as usize].readable;
        error_stream(state, stream, result);
    }
    result
}

fn schedule_pull(state: &mut NativeAgentState, controller: u32) {
    let Some(entry) = state.streams.controllers.get_mut(controller as usize) else {
        return;
    };
    if entry.pull.is_none() || entry.pulling || entry.close_requested {
        return;
    }
    entry.pulling = true;
    super::super::promise::enqueue_stream_task(state, StreamTask::Pull { controller });
}

fn copy_chunk_to_view(
    state: &mut NativeAgentState,
    controller: u32,
    chunk: i64,
    view: i64,
) -> Option<i64> {
    let chunk_length = super::super::typedarray::byte_length(state, chunk)?;
    let view_length = super::super::typedarray::byte_length(state, view)?;
    let written = chunk_length.min(view_length);
    for index in 0..written {
        let stored = super::super::typedarray::get_element(state, chunk, index)?;
        super::super::typedarray::set_element(state, view, index, stored)?;
    }
    if written < chunk_length {
        let mut rest = Vec::with_capacity(chunk_length - written);
        for index in written..chunk_length {
            let stored = super::super::typedarray::get_element(state, chunk, index)?;
            rest.push(value::decode_f64(stored) as u8);
        }
        let rest = super::super::typedarray::create_uint8_array(state, &rest)?;
        state.streams.controllers[controller as usize]
            .queue
            .push_front(rest);
    }
    super::super::typedarray::prefix_view(state, view, written)
}
