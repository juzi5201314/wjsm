use wjsm_host::{
    ControllerKind, ExecContext, NativeCallableRef, PromiseSettlement, ReadableStreamEntry,
    StreamControllerEntry, StreamState, TransformStreamEntry, TransformStreamFlushParams,
    TransformStreamMethodKind, Value, WritableStreamEntry, WritableStreamState,
};
use wjsm_ir::{constants, value};

use super::{
    build_reader_result, create_controller_object, create_readable_stream_object,
    define_accessor_property, define_data_property_with_flags,
};

fn new_controller(kind: ControllerKind, high_water_mark: f64) -> StreamControllerEntry {
    StreamControllerEntry {
        kind,
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

fn strategy_high_water_mark<E: ExecContext>(
    ctx: &mut E,
    strategy: Value,
    default: f64,
) -> Result<f64, Value> {
    if !value::is_js_object(strategy) {
        return Ok(default);
    }
    let raw = ctx.read_property_by_string_key(strategy, "highWaterMark");
    let number = crate::core::to_number(ctx, raw);
    if value::is_exception(number) {
        return Err(number);
    }
    let number = value::decode_f64(number);
    Ok(if number >= 0.0 && number.is_finite() {
        number
    } else {
        default
    })
}

fn create_transform_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(3);
    define_data_property_with_flags(
        ctx,
        object,
        "__transform_stream_handle__",
        value::encode_f64(handle as f64),
        constants::FLAG_PRIVATE,
    );
    for (name, kind) in [
        ("readable", TransformStreamMethodKind::GetReadable),
        ("writable", TransformStreamMethodKind::GetWritable),
    ] {
        let getter = ctx.create_native_callable(NativeCallableRef::TransformStreamMethod {
            handle,
            kind,
        });
        define_accessor_property(ctx, object, name, getter);
    }
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_transform_stream_object(object_handle, handle);
    }
    object
}

pub async fn construct<E: ExecContext>(ctx: &mut E, args: &[Value]) -> Value {
    let transformer = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let writable_strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let readable_strategy = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let (transform, flush) = if value::is_js_object(transformer) {
        let transform = ctx.read_property_by_string_key(transformer, "transform");
        let flush = ctx.read_property_by_string_key(transformer, "flush");
        (
            ctx.is_callable(transform).then_some(transform),
            ctx.is_callable(flush).then_some(flush),
        )
    } else {
        (None, None)
    };
    let readable_hwm = match strategy_high_water_mark(ctx, readable_strategy, 0.0) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let writable_hwm = match strategy_high_water_mark(ctx, writable_strategy, 1.0) {
        Ok(value) => value,
        Err(exception) => return exception,
    };

    let readable_controller =
        ctx.alloc_stream_controller(new_controller(ControllerKind::ReadableDefault, readable_hwm));
    let readable_stream = ctx.alloc_readable_stream(ReadableStreamEntry {
        state: StreamState::Readable,
        error: None,
        disturbed: false,
        locked: false,
        http_response_handle: None,
        response_body_handle: None,
        response_body_object: None,
        controller_handle: Some(readable_controller),
        is_byte_stream: false,
        pipe_to: None,
    });
    let _ = ctx.with_stream_controller(readable_controller, |entry| {
        entry.stream_handle = readable_stream;
        entry.started = true;
    });

    let writable_controller =
        ctx.alloc_stream_controller(new_controller(ControllerKind::Writable, writable_hwm));
    let abort_signal = ctx.create_writable_abort_signal();
    let writable_stream = ctx.alloc_writable_stream(WritableStreamEntry {
        state: WritableStreamState::Writable,
        error: None,
        locked: false,
        controller_handle: Some(writable_controller),
        abort_signal: Some(abort_signal),
    });
    let _ = ctx.with_stream_controller(writable_controller, |entry| {
        entry.stream_handle = writable_stream;
        entry.started = true;
    });

    let readable_object = create_readable_stream_object(ctx, readable_stream);
    let writable_object = super::writable::create_writable_stream_object(ctx, writable_stream);
    let handle = ctx.alloc_transform_stream(TransformStreamEntry {
        readable_stream_handle: Some(readable_stream),
        writable_stream_handle: Some(writable_stream),
        transform_callback: transform,
        flush_callback: flush,
        readable_controller_handle: Some(readable_controller),
        transformer_this: value::is_js_object(transformer).then_some(transformer),
        backpressure: false,
        readable_obj: Some(readable_object),
        writable_obj: Some(writable_object),
    });
    create_transform_object(ctx, handle)
}

pub fn call_method<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: TransformStreamMethodKind,
) -> Option<Value> {
    ctx.with_transform_stream(handle, |entry| match kind {
        TransformStreamMethodKind::GetReadable => entry.readable_obj,
        TransformStreamMethodKind::GetWritable => entry.writable_obj,
    })
    .flatten()
}

pub fn call_transform_from_writable<E: ExecContext>(
    ctx: &mut E,
    writable_stream: u32,
    chunk: Value,
    write_promise: Value,
) {
    let info = ctx.with_transform_streams(|entries| {
        entries.iter().filter_map(Option::as_ref).find_map(|entry| {
            (entry.writable_stream_handle == Some(writable_stream)).then_some((
                entry.transform_callback,
                entry.readable_controller_handle,
                entry.transformer_this,
            ))
        })
    });
    let Some((transform, controller, this_value)) = info else {
        ctx.settle_promise(
            write_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
        return;
    };
    let Some(controller) = controller else {
        ctx.settle_promise(
            write_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
        return;
    };
    let controller_object = create_controller_object(ctx, controller);
    if let Some(transform) = transform {
        ctx.schedule_transform_stream_transform(
            transform,
            this_value.unwrap_or_else(value::encode_undefined),
            chunk,
            controller_object,
            write_promise,
        );
        return;
    }
    let stream = ctx.with_stream_controller(controller, |entry| entry.stream_handle);
    if let Some(stream) = stream {
        let pending = ctx.with_readers(|readers| {
            readers.iter_mut().filter_map(Option::as_mut).find_map(|reader| {
                (reader.stream_handle == stream)
                    .then(|| reader.pending_read_promise.take())
                    .flatten()
            })
        });
        if let Some(promise) = pending {
            let result = build_reader_result(ctx, false, Some(chunk));
            ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
        } else {
            let _ = ctx.with_stream_controller(controller, |entry| {
                if !entry.close_requested {
                    entry.chunk_queue.push_back(chunk);
                }
            });
        }
    }
    ctx.settle_promise(
        write_promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
}

pub fn call_flush_from_writable_close<E: ExecContext>(
    ctx: &mut E,
    writable_stream: u32,
    close_promise: Value,
) -> bool {
    let info = ctx.with_transform_streams(|entries| {
        entries.iter().filter_map(Option::as_ref).find_map(|entry| {
            (entry.writable_stream_handle == Some(writable_stream)).then_some((
                entry.flush_callback,
                entry.readable_controller_handle,
                entry.readable_stream_handle,
                entry.transformer_this,
            ))
        })
    });
    let Some((flush, Some(controller), Some(readable), this_value)) = info else {
        return false;
    };
    let controller_object = create_controller_object(ctx, controller);
    ctx.schedule_transform_stream_flush(TransformStreamFlushParams {
        callback: flush,
        this_value: this_value.unwrap_or_else(value::encode_undefined),
        controller: controller_object,
        writable_stream_handle: writable_stream,
        readable_stream_handle: readable,
        readable_controller_handle: controller,
        close_promise,
    });
    true
}
