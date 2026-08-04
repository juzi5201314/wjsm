//! WHATWG Streams 后端无关语义。

pub mod readable;
pub mod readable_dispatch;
pub mod readable_pipe;
pub mod runtime;
pub mod transform;
pub mod writable;

use wjsm_host::{
    ExecContext, NativeCallableRef, PromiseSettlement, ReadableStreamByobRequestMethodKind,
    ReadableStreamDefaultControllerMethodKind, ReadableStreamMethodKind, StreamControllerEntry,
    Value,
};
use wjsm_ir::{constants, value, wk_symbol};

pub(crate) fn define_data_property_with_flags<E: ExecContext>(
    ctx: &mut E,
    object: Value,
    key: &str,
    property_value: Value,
    flags: i32,
) {
    let key_value = ctx.store_string(key);
    let Some(name_id) = ctx.property_value_to_name_id(key_value, true) else {
        return;
    };
    ctx.define_data_property_by_name_id(object, name_id, property_value, flags);
}

pub(crate) fn define_accessor_property<E: ExecContext>(
    ctx: &mut E,
    object: Value,
    key: &str,
    getter: Value,
) {
    let key_value = ctx.store_string(key);
    let Some(name_id) = ctx.property_value_to_name_id(key_value, true) else {
        return;
    };
    let Some(handle) = ctx.weak_target_handle(object) else {
        return;
    };
    ctx.define_accessor_property_with_flags(
        handle,
        name_id,
        getter,
        value::encode_undefined(),
        constants::FLAG_CONFIGURABLE as u32,
    );
}

pub fn build_reader_result<E: ExecContext>(
    ctx: &mut E,
    done: bool,
    result_value: Option<Value>,
) -> Value {
    let object = ctx.alloc_object(2);
    ctx.define_data_property(object, "done", value::encode_bool(done));
    ctx.define_data_property(
        object,
        "value",
        result_value.unwrap_or_else(value::encode_undefined),
    );
    object
}

pub(crate) fn reject_promise_with_type_error<E: ExecContext>(
    ctx: &mut E,
    promise: Value,
    message: &str,
) {
    let error = ctx.make_type_error(message);
    ctx.settle_promise(promise, PromiseSettlement::Reject(error));
}

pub(crate) fn fulfill_byob_read<E: ExecContext>(
    ctx: &mut E,
    controller_handle: u32,
    chunk: Value,
    view: Value,
    promise: Value,
) {
    let Some(bytes) = ctx.stream_typedarray_u8_bytes(chunk) else {
        reject_promise_with_type_error(
            ctx,
            promise,
            "Byte stream chunks must be Uint8Array-compatible",
        );
        return;
    };
    let Some(written) = ctx.stream_write_u8_bytes(view, &bytes) else {
        reject_promise_with_type_error(ctx, promise, "BYOB read requires a writable byte view");
        return;
    };
    if written < bytes.len() {
        let rest = ctx.stream_create_uint8array(&bytes[written..]);
        let _ = ctx.with_stream_controller(controller_handle, |controller| {
            controller.chunk_queue.push_front(rest);
        });
    }
    let result_view = ctx.stream_transfer_byob_view(view, written).unwrap_or(view);
    let result = build_reader_result(ctx, false, Some(result_view));
    ctx.settle_promise(promise, PromiseSettlement::Fulfill(result));
}

pub fn create_readable_stream_object<E: ExecContext>(ctx: &mut E, stream_handle: u32) -> Value {
    let object = ctx.alloc_object(8);
    define_data_property_with_flags(
        ctx,
        object,
        "__stream_handle__",
        value::encode_f64(stream_handle as f64),
        constants::FLAG_PRIVATE,
    );
    let locked = ctx.create_native_callable(NativeCallableRef::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::GetLocked,
    });
    define_accessor_property(ctx, object, "locked", locked);
    for (name, kind) in [
        ("getReader", ReadableStreamMethodKind::GetReader),
        ("cancel", ReadableStreamMethodKind::Cancel),
        ("tee", ReadableStreamMethodKind::Tee),
        ("pipeTo", ReadableStreamMethodKind::PipeTo),
        ("pipeThrough", ReadableStreamMethodKind::PipeThrough),
    ] {
        let callable = ctx.create_native_callable(NativeCallableRef::ReadableStreamMethod {
            handle: stream_handle,
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
    let async_iterator = ctx.create_native_callable(NativeCallableRef::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::AsyncIterator,
    });
    ctx.define_data_property_by_name_id(
        object,
        wjsm_host::encode_symbol_name_id(wk_symbol::ASYNC_ITERATOR),
        async_iterator,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_readable_stream_object(object_handle, stream_handle);
    }
    object
}

pub fn create_controller_object<E: ExecContext>(ctx: &mut E, controller_handle: u32) -> Value {
    let object = ctx.alloc_object(6);
    define_data_property_with_flags(
        ctx,
        object,
        "__controller_handle__",
        value::encode_f64(controller_handle as f64),
        constants::FLAG_PRIVATE,
    );
    for (name, kind) in [
        (
            "enqueue",
            ReadableStreamDefaultControllerMethodKind::Enqueue,
        ),
        ("close", ReadableStreamDefaultControllerMethodKind::Close),
        ("error", ReadableStreamDefaultControllerMethodKind::Error),
    ] {
        let callable =
            ctx.create_native_callable(NativeCallableRef::ReadableStreamDefaultControllerMethod {
                handle: controller_handle,
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
        let getter =
            ctx.create_native_callable(NativeCallableRef::ReadableStreamDefaultControllerMethod {
                handle: controller_handle,
                kind,
            });
        define_accessor_property(ctx, object, name, getter);
    }
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_stream_controller_object(object_handle, controller_handle);
    }
    object
}

pub fn create_byob_request_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(3);
    let getter = ctx.create_native_callable(NativeCallableRef::ReadableStreamByobRequestMethod {
        handle,
        kind: ReadableStreamByobRequestMethodKind::GetView,
    });
    define_accessor_property(ctx, object, "view", getter);
    let respond = ctx.create_native_callable(NativeCallableRef::ReadableStreamByobRequestMethod {
        handle,
        kind: ReadableStreamByobRequestMethodKind::Respond,
    });
    define_data_property_with_flags(
        ctx,
        object,
        "respond",
        respond,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    if let Some(object_handle) = ctx.weak_target_handle(object) {
        ctx.bind_byob_request_object(object_handle, handle);
    }
    object
}

pub(crate) fn new_readable_controller(
    high_water_mark: f64,
    close_requested: bool,
) -> StreamControllerEntry {
    StreamControllerEntry {
        kind: wjsm_host::ControllerKind::ReadableDefault,
        stream_handle: 0,
        chunk_queue: std::collections::VecDeque::new(),
        high_water_mark,
        strategy_size: None,
        started: false,
        close_requested,
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
