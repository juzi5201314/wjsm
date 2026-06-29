// ReadableStream 核心实现（WHATWG Streams Phase 1）
// 包含：构造函数、DefaultController、DefaultReader、locked getter、cancel

use super::fetch_core::{alloc_type_error_from_caller, push_native_callable};
use super::streams_transform::{call_flush_from_writable_close, call_transform_from_writable};
use crate::*;
use std::collections::VecDeque;

/// 创建 TypeError 异常值（NaN-boxed TAG_EXCEPTION）
fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    let error_obj = alloc_type_error_from_caller(caller, message);
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "TypeError".to_string(),
        message: message.to_string(),
        value: error_obj,
    });
    value::encode_exception(idx)
}

/// 标记 Response.body 对应的 Response 已被消费。
pub(crate) fn mark_response_body_used_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    response_handle: Option<u32>,
    response_obj: Option<i64>,
) {
    if let Some(handle) = response_handle {
        let mut table = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.body_used = true;
        }
    }
    if let Some(obj) = response_obj {
        let _ =
            set_host_data_property_from_caller(caller, obj, "bodyUsed", value::encode_bool(true));
    }
}
// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 构建 reader.read() 返回的 { done, value } 结果对象
pub(crate) fn build_reader_result(
    caller: &mut Caller<'_, RuntimeState>,
    done: bool,
    value: Option<i64>,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(caller, obj, "done", value::encode_bool(done));
    let val = value.unwrap_or_else(value::encode_undefined);
    let _ = define_host_data_property_from_caller(caller, obj, "value", val);
    obj
}

fn typedarray_u8_bytes(caller: &mut Caller<'_, RuntimeState>, typedarray: i64) -> Option<Vec<u8>> {
    let entry = typedarray_entry_from_value(caller, typedarray)?;
    if entry.element_size != 1 {
        return None;
    }
    let start = entry.byte_offset as usize;
    let len = entry.length as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let sab_table = shared.sab_table.lock().ok()?;
        let buffer = sab_table.get(entry.buffer_handle as usize)?;
        let data = buffer.data.read().ok()?;
        let end = start.checked_add(len)?;
        data.get(start..end).map(|bytes| bytes.to_vec())
    } else {
        let ab_table = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = ab_table.get(entry.buffer_handle as usize)?;
        let end = start.checked_add(len)?;
        buffer.data.get(start..end).map(|bytes| bytes.to_vec())
    }
}

pub(crate) fn write_u8_bytes_to_view(
    caller: &mut Caller<'_, RuntimeState>,
    view: i64,
    bytes: &[u8],
) -> Option<usize> {
    let entry = typedarray_entry_from_value(caller, view)?;
    if entry.element_size != 1 {
        return None;
    }
    let write_len = (entry.length as usize).min(bytes.len());
    let start = entry.byte_offset as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let sab_table = shared.sab_table.lock().ok()?;
        let buffer = sab_table.get(entry.buffer_handle as usize)?;
        let mut data = buffer.data.write().ok()?;
        let end = start.checked_add(write_len)?;
        data.get_mut(start..end)?
            .copy_from_slice(&bytes[..write_len]);
    } else {
        let mut ab_table = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = ab_table.get_mut(entry.buffer_handle as usize)?;
        let end = start.checked_add(write_len)?;
        buffer
            .data
            .get_mut(start..end)?
            .copy_from_slice(&bytes[..write_len]);
    }
    Some(write_len)
}

/// 构造一个与原 view 共享同一 ArrayBuffer 但长度截断为 `bytes_written` 的新
/// typed-array 视图。用于 BYOB reader.read() 返回值的 `value`，确保
/// result.value.byteLength === bytesWritten（WHATWG Streams 规范要求）。
pub(crate) fn truncate_byob_view_with_env<C: wasmtime::AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    view: i64,
    bytes_written: usize,
) -> Option<i64> {
    let entry = typedarray_entry_from_value_with_env(ctx, env, view)?;
    let new_ta = {
        let store = ctx.as_context_mut();
        let mut ta_table = store
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let h = ta_table.len() as u32;
        ta_table.push(TypedArrayEntry {
            buffer_handle: entry.buffer_handle,
            byte_offset: entry.byte_offset,
            length: bytes_written as u32,
            element_size: entry.element_size,
            element_kind: entry.element_kind,
            is_shared: entry.is_shared,
        });
        h
    };
    let obj = crate::runtime_heap::alloc_host_object(ctx, env, 4);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__typedarray_handle__",
        value::encode_f64(new_ta as f64),
    );
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__arraybuffer_handle__",
        value::encode_f64(entry.buffer_handle as f64),
    );
    let len_val = value::encode_f64(bytes_written as f64);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx, env, obj, "length", len_val,
    );
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteLength",
        len_val,
    );
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "byteOffset",
        value::encode_f64(entry.byte_offset as f64),
    );
    Some(obj)
}

fn reject_promise_with_type_error(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    message: &str,
) {
    let err = type_error_exception(caller, message);
    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
}

fn fulfill_byob_read(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
    chunk: i64,
    view: i64,
    promise: i64,
) {
    let Some(bytes) = typedarray_u8_bytes(caller, chunk) else {
        reject_promise_with_type_error(
            caller,
            promise,
            "Byte stream chunks must be Uint8Array-compatible",
        );
        return;
    };
    let Some(written) = write_u8_bytes_to_view(caller, view, &bytes) else {
        reject_promise_with_type_error(caller, promise, "BYOB read requires a writable byte view");
        return;
    };
    if written < bytes.len() {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        let rest = create_uint8array_with_env(caller, &env, &bytes[written..]);
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.chunk_queue.push_front(rest);
        }
    }
    // 构造截断视图：result.value.byteLength === bytesWritten（规范语义）
    let result_view = {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        truncate_byob_view_with_env(caller, &env, view, written).unwrap_or(view)
    };
    let result = build_reader_result(caller, false, Some(result_view));
    settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
}

/// 创建 ReadableStream JS 对象（包含 __stream_handle__、locked getter、getReader、cancel、tee、Symbol.asyncIterator）
/// 从 create_closed_readable_stream_from_bytes / construct_readable_stream / tee 共用
pub(crate) fn create_readable_stream_js_object(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);

    // __stream_handle__ = handle
    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__stream_handle__", handle_val);

    // locked → accessor getter
    let locked_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::GetLocked,
    };
    let locked_idx = push_native_callable(caller, locked_callable);
    let locked_getter = value::encode_native_callable_idx(locked_idx);
    let undef = value::encode_undefined();
    let _ =
        define_host_accessor_property_with_env(caller, &env, obj, "locked", locked_getter, undef);

    // getReader() → method
    let get_reader_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::GetReader,
    };
    let get_reader_idx = push_native_callable(caller, get_reader_callable);
    let get_reader_val = value::encode_native_callable_idx(get_reader_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "getReader", get_reader_val);

    // cancel() → method
    let cancel_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::Cancel,
    };
    let cancel_idx = push_native_callable(caller, cancel_callable);
    let cancel_val = value::encode_native_callable_idx(cancel_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "cancel", cancel_val);

    // tee() → method
    let tee_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::Tee,
    };
    let tee_idx = push_native_callable(caller, tee_callable);
    let tee_val = value::encode_native_callable_idx(tee_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "tee", tee_val);

    // [Symbol.asyncIterator] → method
    let async_iter_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::AsyncIterator,
    };
    let async_iter_idx = push_native_callable(caller, async_iter_callable);
    let async_iter_val = value::encode_native_callable_idx(async_iter_idx);
    let _ = define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        encode_symbol_name_id(3),
        async_iter_val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );

    // pipeTo(destination) → method
    let pipe_to_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::PipeTo,
    };
    let pipe_to_idx = push_native_callable(caller, pipe_to_callable);
    let pipe_to_val = value::encode_native_callable_idx(pipe_to_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "pipeTo", pipe_to_val);

    // pipeThrough(transform) → method
    let pipe_through_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::PipeThrough,
    };
    let pipe_through_idx = push_native_callable(caller, pipe_through_callable);
    let pipe_through_val = value::encode_native_callable_idx(pipe_through_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "pipeThrough", pipe_through_val);

    obj
}

/// 创建 controller JS 对象（带 enqueue/close/error + desiredSize getter）
pub(crate) fn create_controller_object(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 6);

    // __controller_handle__ — 内部标识
    let handle_val = value::encode_f64(controller_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__controller_handle__", handle_val);

    // enqueue(chunk)
    let enqueue_callable = NativeCallable::ReadableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: ReadableStreamDefaultControllerMethodKind::Enqueue,
    };
    let enqueue_idx = push_native_callable(caller, enqueue_callable);
    let enqueue_val = value::encode_native_callable_idx(enqueue_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "enqueue", enqueue_val);

    // close()
    let close_callable = NativeCallable::ReadableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: ReadableStreamDefaultControllerMethodKind::Close,
    };
    let close_idx = push_native_callable(caller, close_callable);
    let close_val = value::encode_native_callable_idx(close_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "close", close_val);

    // error(e)
    let error_callable = NativeCallable::ReadableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: ReadableStreamDefaultControllerMethodKind::Error,
    };
    let error_idx = push_native_callable(caller, error_callable);
    let error_val = value::encode_native_callable_idx(error_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "error", error_val);

    // desiredSize → accessor getter
    let desired_size_callable = NativeCallable::ReadableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: ReadableStreamDefaultControllerMethodKind::GetDesiredSize,
    };
    let desired_size_idx = push_native_callable(caller, desired_size_callable);
    let desired_size_getter = value::encode_native_callable_idx(desired_size_idx);
    let undef = value::encode_undefined();
    let _ = define_host_accessor_property_with_env(
        caller,
        &env,
        obj,
        "desiredSize",
        desired_size_getter,
        undef,
    );

    // ByteStreamController.byobRequest → accessor getter
    // 返回当前活动的 ReadableStreamBYOBRequest 对象，或 null。
    let byob_request_callable = NativeCallable::ReadableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: ReadableStreamDefaultControllerMethodKind::GetByobRequest,
    };
    let byob_request_idx = push_native_callable(caller, byob_request_callable);
    let byob_request_getter = value::encode_native_callable_idx(byob_request_idx);
    let _ = define_host_accessor_property_with_env(
        caller,
        &env,
        obj,
        "byobRequest",
        byob_request_getter,
        undef,
    );

    obj
}

// ── ReadableStream 构造函数 ─────────────────────────────────────────────────

/// 创建已关闭的 ReadableStream，body bytes 作为 Uint8Array chunk 预入队
/// 用于 data: URL 和构造 Response 时的 body → ReadableStream 转换
pub(crate) fn create_closed_readable_stream_from_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8],
    response_body_handle: Option<u32>,
    response_body_object: Option<i64>,
) -> i64 {
    // 1. 创建 StreamControllerEntry (ControllerKind::ReadableDefault)
    let controller_handle = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::ReadableDefault,
            stream_handle: 0, // 稍后回写
            chunk_queue: VecDeque::new(),
            high_water_mark: 1.0,
            strategy_size: None,
            started: false,
            close_requested: true, // 已关闭
            byob_reader_handle: None,
            pull_requested: false,
            abort_requested: false,
            abort_reason: None,
            flush_requested: false,
            underlying_source: None,
            pull_callback: None,
            cancel_callback: None,
            active_byob_request: None,
        });
        handle
    };

    // 2. 将 body bytes 作为 Uint8Array 推入 controller 的 chunk_queue
    if !bytes.is_empty() {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        let uint8array_obj = create_uint8array_with_env(caller, &env, bytes);
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.chunk_queue.push_back(uint8array_obj);
        }
    }

    // 3. 创建 ReadableStreamEntry (controller_handle: Some(ctrl), http_response_handle: None)
    let stream_handle = {
        let mut table = caller
            .data()
            .readable_stream_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ReadableStreamEntry {
            state: StreamState::Closed, // 已关闭
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: None,
            response_body_handle,
            response_body_object,
            controller_handle: Some(controller_handle),
            is_byte_stream: true,
        });
        handle
    };

    // 4. 回写 stream_handle 到 controller
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.stream_handle = stream_handle;
            ctrl.started = true;
        }
    }

    // 5. 构造 ReadableStream JS 对象
    create_readable_stream_js_object(caller, stream_handle)
}

/// ReadableStream 构造函数 — 由 NativeCallable::ReadableStreamConstructor 调度
mod streams_readable_ctrl;
mod streams_readable_dispatch;
mod streams_readable_pipe;

pub(crate) use streams_readable_ctrl::*;
pub(crate) use streams_readable_dispatch::*;
pub(crate) use streams_readable_pipe::*;
