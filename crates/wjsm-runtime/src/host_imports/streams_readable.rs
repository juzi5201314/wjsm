// ReadableStream 核心实现（WHATWG Streams Phase 1）
// 包含：构造函数、DefaultController、DefaultReader、locked getter、cancel

use super::fetch_core::{alloc_type_error_from_caller, push_native_callable};
use super::streams_transform::{call_flush_from_writable_close, call_transform_from_writable};
use crate::*;
use std::collections::VecDeque;

/// 创建 TypeError 异常值（NaN-boxed TAG_EXCEPTION）
fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    let error_obj = alloc_type_error_from_caller(caller, message);
    let mut errors = caller.data().error_table.lock().expect("error table mutex");
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
            .expect("fetch_response_table mutex");
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

fn write_u8_bytes_to_view(
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
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.chunk_queue.push_front(rest);
        }
    }
    let result = build_reader_result(caller, false, Some(view));
    settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
}

/// 创建 ReadableStream JS 对象（包含 __stream_handle__、locked getter、getReader、cancel、tee、Symbol.asyncIterator）
/// 从 create_closed_readable_stream_from_bytes / construct_readable_stream / tee 共用
fn create_readable_stream_js_object(
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
    let _ =
        define_host_data_property_from_caller(caller, obj, "Symbol.asyncIterator", async_iter_val);

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

    // ByteStreamController.byobRequest：当前无活动 BYOB pull-into 请求时为 null。
    let _ = define_host_data_property_from_caller(caller, obj, "byobRequest", value::encode_null());

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
            .expect("controller mutex");
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
            .expect("controller mutex");
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
            .expect("stream mutex");
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
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.stream_handle = stream_handle;
            ctrl.started = true;
        }
    }

    // 5. 构造 ReadableStream JS 对象
    let obj = create_readable_stream_js_object(caller, stream_handle);

    obj
}

/// ReadableStream 构造函数 — 由 NativeCallable::ReadableStreamConstructor 调度
pub(crate) async fn construct_readable_stream(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    // 1. 解析 underlyingSource (args[0]) 和 strategy (args[1])
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);

    // WHATWG byte stream: underlyingSource.type === "bytes"。
    let is_byte_stream = if value::is_object(source) {
        resolve_handle(caller, source)
            .and_then(|ptr| read_object_property_by_name(caller, ptr, "type"))
            .filter(|raw| value::is_string(*raw))
            .map(|raw| get_string_value(caller, raw) == "bytes")
            .unwrap_or(false)
    } else {
        false
    };

    // 2. 解析 strategy.highWaterMark（默认 1）
    let high_water_mark = if value::is_object(strategy) {
        let strategy_ptr = resolve_handle(caller, strategy);
        if let Some(ptr) = strategy_ptr {
            let hwm_val = read_object_property_by_name(caller, ptr, "highWaterMark")
                .unwrap_or_else(value::encode_undefined);
            if value::is_f64(hwm_val) {
                let v = value::decode_f64(hwm_val);
                if v >= 0.0 && v.is_finite() { v } else { 1.0 }
            } else {
                1.0
            }
        } else {
            1.0
        }
    } else {
        1.0
    };

    // 3. 创建 StreamControllerEntry (ControllerKind::ReadableDefault)
    let controller_handle = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::ReadableDefault,
            stream_handle: 0, // 稍后回写
            chunk_queue: VecDeque::new(),
            high_water_mark,
            strategy_size: None,
            started: false,
            close_requested: false,
            byob_reader_handle: None,
            pull_requested: false,
            abort_requested: false,
            abort_reason: None,
            flush_requested: false,
        });
        handle
    };

    // 4. 创建 ReadableStreamEntry
    let stream_handle = {
        let mut table = caller
            .data()
            .readable_stream_table
            .lock()
            .expect("stream mutex");
        let handle = table.len() as u32;
        table.push(ReadableStreamEntry {
            state: StreamState::Readable,
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: None,
            response_body_handle: None,
            response_body_object: None,
            controller_handle: Some(controller_handle),
            is_byte_stream,
        });
        handle
    };

    // 5. 回写 stream_handle 到 controller
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.stream_handle = stream_handle;
        }
    }

    // 6. 如果 source.start 存在：创建 controller JS 对象，调用 start(controller)
    let controller_obj = create_controller_object(caller, controller_handle);

    if value::is_object(source) {
        let source_ptr = resolve_handle(caller, source);
        if let Some(ptr) = source_ptr {
            let start_fn = read_object_property_by_name(caller, ptr, "start")
                .unwrap_or_else(value::encode_undefined);
            if value::is_callable(start_fn) {
                // 调用 source.start(controller)
                let _ = call_wasm_callback_async(caller, start_fn, source, &[controller_obj]).await;
            }
        }
    }

    // 7. 标记 controller.started = true
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.started = true;
        }
    }

    // 8. 构造 ReadableStream JS 对象
    let obj = create_readable_stream_js_object(caller, stream_handle);

    Some(obj)
}

// ── Controller 方法实现 ──────────────────────────────────────────────────────

/// controller.enqueue(chunk)
fn controller_enqueue(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
    args: &[i64],
) -> Option<i64> {
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    // 1. 检查 close_requested → TypeError
    let (close_requested, stream_handle) = {
        let table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let ctrl = table.get(controller_handle as usize)?;
        (ctrl.close_requested, ctrl.stream_handle)
    };
    if close_requested {
        return Some(type_error_exception(
            caller,
            "Cannot enqueue to a closed stream",
        ));
    }

    // 2. 检查 stream 是否已 errored
    let stream_state = {
        let table = caller
            .data()
            .readable_stream_table
            .lock()
            .expect("stream mutex");
        table.get(stream_handle as usize).map(|e| e.state.clone())
    };
    if matches!(
        stream_state,
        Some(StreamState::Closed) | Some(StreamState::Errored)
    ) {
        return Some(type_error_exception(
            caller,
            "Cannot enqueue to a closed or errored stream",
        ));
    }

    // 3. 检查 reader 是否有 pending read promise
    let pending = {
        let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
        let mut pending_info: Option<(ReaderKind, Option<i64>, i64)> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle {
                if let Some(promise) = reader.pending_read_promise.take() {
                    pending_info = Some((reader.kind, reader.pending_byob_view.take(), promise));
                    break;
                }
            }
        }
        pending_info
    };

    if let Some((reader_kind, byob_view, promise)) = pending {
        // 有等待中的 read → 立即 settle
        if reader_kind == ReaderKind::Byob {
            if let Some(view) = byob_view {
                fulfill_byob_read(caller, controller_handle, chunk, view, promise);
            } else {
                reject_promise_with_type_error(caller, promise, "BYOB read requires a view");
            }
        } else {
            let result = build_reader_result(caller, false, Some(chunk));
            settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
        }
    } else {
        // 无等待 → 推入 chunk_queue
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.chunk_queue.push_back(chunk);
        }
    }

    Some(value::encode_undefined())
}

/// controller.close()
fn controller_close(caller: &mut Caller<'_, RuntimeState>, controller_handle: u32) -> Option<i64> {
    let (already_closed, stream_handle) = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let ctrl = table.get_mut(controller_handle as usize)?;
        if ctrl.close_requested {
            (true, ctrl.stream_handle)
        } else {
            ctrl.close_requested = true;
            (false, ctrl.stream_handle)
        }
    };

    if already_closed {
        return Some(type_error_exception(
            caller,
            "The stream has already been closed",
        ));
    }

    // 更新 stream state = Closed
    {
        let mut table = caller
            .data()
            .readable_stream_table
            .lock()
            .expect("stream mutex");
        if let Some(entry) = table.get_mut(stream_handle as usize) {
            entry.state = StreamState::Closed;
        }
    }

    // 检查 pending_read_promise → resolve {done: true, value: undefined}
    let pending = {
        let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
        let mut pending_info: Option<(Option<i64>, i64)> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle {
                if let Some(promise) = reader.pending_read_promise.take() {
                    pending_info = Some((reader.pending_byob_view.take(), promise));
                    break;
                }
            }
        }
        pending_info
    };

    if let Some((byob_view, promise)) = pending {
        let result = build_reader_result(caller, true, byob_view);
        settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
    }

    Some(value::encode_undefined())
}

/// controller.error(e)
fn controller_error(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
    args: &[i64],
) -> Option<i64> {
    let error_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let stream_handle = {
        let table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let ctrl = table.get(controller_handle as usize)?;
        ctrl.stream_handle
    };

    // 设置 stream state = Errored
    {
        let mut table = caller
            .data()
            .readable_stream_table
            .lock()
            .expect("stream mutex");
        if let Some(entry) = table.get_mut(stream_handle as usize) {
            entry.state = StreamState::Errored;
            // 尝试存储错误消息
            if value::is_string(error_val) {
                entry.error = Some(format!("stream error"));
            }
        }
    }

    // 检查 pending_read_promise → reject
    let pending = {
        let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
        let mut pending_promise: Option<i64> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle {
                if let Some(promise) = reader.pending_read_promise.take() {
                    pending_promise = Some(promise);
                    break;
                }
            }
        }
        pending_promise
    };

    if let Some(promise) = pending {
        settle_promise(caller.data(), promise, PromiseSettlement::Reject(error_val));
    }

    Some(value::encode_undefined())
}

// ── ReadableStream 方法分发 ─────────────────────────────────────────────────

/// ReadableStream 方法分发
pub(crate) fn call_readable_stream_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: ReadableStreamMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        ReadableStreamMethodKind::GetLocked => {
            // 从 readable_stream_table 读取 locked 返回 bool
            let table = caller
                .data()
                .readable_stream_table
                .lock()
                .expect("stream mutex");
            let locked = table
                .get(handle as usize)
                .map(|e| e.locked)
                .unwrap_or(false);
            Some(value::encode_bool(locked))
        }
        ReadableStreamMethodKind::GetReader => {
            let wants_byob = args
                .first()
                .copied()
                .filter(|options| value::is_object(*options))
                .and_then(|options| resolve_handle(caller, options))
                .and_then(|ptr| read_object_property_by_name(caller, ptr, "mode"))
                .filter(|mode| value::is_string(*mode))
                .map(|mode| get_string_value(caller, mode) == "byob")
                .unwrap_or(false);

            // 检查 locked；BYOB reader 只能用于 byte stream。
            let (locked, is_byte_stream, response_body) = {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let entry = stream_table.get_mut(handle as usize)?;
                let locked = entry.locked;
                let is_byte_stream = entry.is_byte_stream;
                let response_body = (entry.response_body_handle, entry.response_body_object);
                if !locked && (!wants_byob || is_byte_stream) {
                    entry.locked = true;
                    entry.disturbed = true;
                }
                (locked, is_byte_stream, response_body)
            };
            if !locked && (!wants_byob || is_byte_stream) {
                mark_response_body_used_from_caller(caller, response_body.0, response_body.1);
            }
            if locked {
                return Some(type_error_exception(
                    caller,
                    "ReadableStream is already locked to a reader",
                ));
            }
            if wants_byob && !is_byte_stream {
                return Some(type_error_exception(
                    caller,
                    "ReadableStreamBYOBReader requires a byte stream",
                ));
            }

            let reader_kind = if wants_byob {
                ReaderKind::Byob
            } else {
                ReaderKind::Default
            };

            // 创建 reader + closed_promise
            let closed_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reader_handle = {
                let mut table = caller.data().reader_table.lock().expect("reader mutex");
                let rh = table.len() as u32;
                table.push(ReaderEntry {
                    stream_handle: handle,
                    kind: reader_kind,
                    pending_read_promise: None,
                    pending_byob_view: None,
                    closed_promise: Some(closed_promise),
                });
                rh
            };

            // 如果流已关闭，立即 resolve closed promise
            let stream_state = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                table.get(handle as usize).map(|e| e.state.clone())
            };
            if matches!(stream_state, Some(StreamState::Closed)) {
                settle_promise(
                    caller.data(),
                    closed_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
            }

            // 构造 reader JS 对象
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let obj = alloc_host_object(caller, &env, 5);

            // __reader_handle__
            let rh_val = value::encode_f64(reader_handle as f64);
            let _ = define_host_data_property_from_caller(caller, obj, "__reader_handle__", rh_val);

            // read() → method
            let read_callable = NativeCallable::ReadableStreamDefaultReaderMethod {
                handle: reader_handle,
                kind: ReadableStreamDefaultReaderMethodKind::Read,
            };
            let read_idx = push_native_callable(caller, read_callable);
            let read_val = value::encode_native_callable_idx(read_idx);
            let _ = define_host_data_property_from_caller(caller, obj, "read", read_val);

            // releaseLock() → method
            let release_callable = NativeCallable::ReadableStreamDefaultReaderMethod {
                handle: reader_handle,
                kind: ReadableStreamDefaultReaderMethodKind::ReleaseLock,
            };
            let release_idx = push_native_callable(caller, release_callable);
            let release_val = value::encode_native_callable_idx(release_idx);
            let _ = define_host_data_property_from_caller(caller, obj, "releaseLock", release_val);

            // closed → accessor getter
            let closed_callable = NativeCallable::ReadableStreamDefaultReaderMethod {
                handle: reader_handle,
                kind: ReadableStreamDefaultReaderMethodKind::GetClosed,
            };
            let closed_idx = push_native_callable(caller, closed_callable);
            let closed_getter = value::encode_native_callable_idx(closed_idx);
            let undef = value::encode_undefined();
            let _ = define_host_accessor_property_with_env(
                caller,
                &env,
                obj,
                "closed",
                closed_getter,
                undef,
            );

            // closedPromise — 内部属性，reader 实现可能需要
            let _ = define_host_data_property_from_caller(
                caller,
                obj,
                "__closed_promise__",
                closed_promise,
            );

            Some(obj)
        }
        ReadableStreamMethodKind::Cancel => {
            // 设置 state = Closed
            let controller_handle = {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let entry = stream_table.get_mut(handle as usize)?;
                entry.state = StreamState::Closed;
                entry.controller_handle
            };

            // 清空 controller 队列
            if let Some(ctrl_handle) = controller_handle {
                let mut ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                if let Some(ctrl) = ctrl_table.get_mut(ctrl_handle as usize) {
                    ctrl.chunk_queue.clear();
                    ctrl.close_requested = true;
                }
            }

            Some(value::encode_undefined())
        }
        ReadableStreamMethodKind::Tee => {
            // WHATWG Streams Standard: ReadableStream.tee()
            // 将当前流拆分为两个独立分支，返回 [stream1, stream2]

            // 1. 检查流是否已 locked
            let is_locked = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                table
                    .get(handle as usize)
                    .map(|e| e.locked)
                    .unwrap_or(false)
            };
            if is_locked {
                return Some(type_error_exception(
                    caller,
                    "ReadableStream is already locked to a reader",
                ));
            }

            // 2. 标记原始流 disturbed = true, locked = true，获取 state 和 controller_handle
            let (original_state, ctrl_handle, original_is_byte_stream) = {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let entry = stream_table.get_mut(handle as usize)?;
                entry.disturbed = true;
                entry.locked = true;
                (
                    entry.state.clone(),
                    entry.controller_handle,
                    entry.is_byte_stream,
                )
            };

            // 3. 从原始 controller 获取 chunk_queue 和配置（stream_table 锁已释放）
            let (chunk_queue_clone, controller_hwm, controller_strategy_size) = {
                let ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                let ctrl = ctrl_table.get(ctrl_handle? as usize)?;
                (
                    ctrl.chunk_queue.clone(),
                    ctrl.high_water_mark,
                    ctrl.strategy_size,
                )
            };

            // 4. 创建两个新的 StreamControllerEntry，各自持有 chunk_queue 的副本
            let controller1_handle = {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                let h = table.len() as u32;
                table.push(StreamControllerEntry {
                    kind: ControllerKind::ReadableDefault,
                    stream_handle: 0, // 稍后回写
                    chunk_queue: chunk_queue_clone.clone(),
                    high_water_mark: controller_hwm,
                    strategy_size: controller_strategy_size,
                    started: true, // tee 产生的流不需要 start 回调
                    close_requested: matches!(original_state, StreamState::Closed),
                    byob_reader_handle: None,
                    pull_requested: false,
                    abort_requested: false,
                    abort_reason: None,
                    flush_requested: false,
                });
                h
            };

            let controller2_handle = {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                let h = table.len() as u32;
                table.push(StreamControllerEntry {
                    kind: ControllerKind::ReadableDefault,
                    stream_handle: 0, // 稍后回写
                    chunk_queue: chunk_queue_clone,
                    high_water_mark: controller_hwm,
                    strategy_size: controller_strategy_size,
                    started: true,
                    close_requested: matches!(original_state, StreamState::Closed),
                    byob_reader_handle: None,
                    pull_requested: false,
                    abort_requested: false,
                    abort_reason: None,
                    flush_requested: false,
                });
                h
            };

            // 5. 创建两个新的 ReadableStreamEntry
            let stream1_handle = {
                let mut table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let h = table.len() as u32;
                table.push(ReadableStreamEntry {
                    state: StreamState::Readable,
                    error: None,
                    disturbed: false,
                    locked: false,
                    http_response_handle: None,
                    response_body_handle: None,
                    response_body_object: None,
                    controller_handle: Some(controller1_handle),
                    is_byte_stream: original_is_byte_stream,
                });
                h
            };

            let stream2_handle = {
                let mut table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let h = table.len() as u32;
                table.push(ReadableStreamEntry {
                    state: StreamState::Readable,
                    error: None,
                    disturbed: false,
                    locked: false,
                    http_response_handle: None,
                    response_body_handle: None,
                    response_body_object: None,
                    controller_handle: Some(controller2_handle),
                    is_byte_stream: original_is_byte_stream,
                });
                h
            };

            // 6. 回写 stream_handle 到各自的 controller
            {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                if let Some(ctrl) = table.get_mut(controller1_handle as usize) {
                    ctrl.stream_handle = stream1_handle;
                }
                if let Some(ctrl) = table.get_mut(controller2_handle as usize) {
                    ctrl.stream_handle = stream2_handle;
                }
            }

            // 7. 构造两个 ReadableStream JS 对象
            let stream1_obj = create_readable_stream_js_object(caller, stream1_handle);
            let stream2_obj = create_readable_stream_js_object(caller, stream2_handle);

            // 8. 创建 JS 数组 [stream1_obj, stream2_obj]
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let arr = alloc_host_object(caller, &env, 2);
            let _ = define_host_data_property_from_caller(
                caller,
                arr,
                "length",
                value::encode_f64(2.0),
            );
            let _ = define_host_data_property_from_caller(caller, arr, "0", stream1_obj);
            let _ = define_host_data_property_from_caller(caller, arr, "1", stream2_obj);

            Some(arr)
        }
        ReadableStreamMethodKind::AsyncIterator => {
            // 检查流是否已锁定
            let locked = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                table
                    .get(handle as usize)
                    .map(|e| e.locked)
                    .unwrap_or(false)
            };
            if locked {
                return Some(type_error_exception(
                    caller,
                    "ReadableStream is already locked to a reader",
                ));
            }

            // 锁定流
            {
                let mut table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.locked = true;
                }
            }

            // 创建 closed_promise 和 ReaderEntry（与 GetReader 相同的模式）
            let closed_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reader_handle = {
                let mut table = caller.data().reader_table.lock().expect("reader mutex");
                let rh = table.len() as u32;
                table.push(ReaderEntry {
                    stream_handle: handle,
                    kind: ReaderKind::Default,
                    pending_read_promise: None,
                    pending_byob_view: None,
                    closed_promise: Some(closed_promise),
                });
                rh
            };

            // 如果流已关闭，立即 resolve closed promise
            let stream_state = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                table.get(handle as usize).map(|e| e.state.clone())
            };
            if matches!(stream_state, Some(StreamState::Closed)) {
                settle_promise(
                    caller.data(),
                    closed_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
            }

            // 创建迭代器对象
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let iter_obj = alloc_host_object(caller, &env, 2);

            // next() 方法：委托给 reader.read()
            let next_callable = NativeCallable::ReadableStreamAsyncIteratorNext { reader_handle };
            let next_idx = push_native_callable(caller, next_callable);
            let next_val = value::encode_native_callable_idx(next_idx);
            let _ = define_host_data_property_from_caller(caller, iter_obj, "next", next_val);

            // return() 方法：释放锁定并返回 {done: true, value: undefined}
            let ret_callable = NativeCallable::ReadableStreamAsyncIteratorReturn { reader_handle };
            let ret_idx = push_native_callable(caller, ret_callable);
            let ret_val = value::encode_native_callable_idx(ret_idx);
            let _ = define_host_data_property_from_caller(caller, iter_obj, "return", ret_val);

            Some(iter_obj)
        }
        ReadableStreamMethodKind::PipeTo => {
            let destination = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            readable_stream_pipe_to(caller, handle, destination)
        }
        ReadableStreamMethodKind::PipeThrough => {
            let transform = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            readable_stream_pipe_through(caller, handle, transform)
        }
    }
}

fn writable_stream_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    writable: i64,
) -> Option<u32> {
    resolve_handle(caller, writable)
        .and_then(|ptr| read_object_property_by_name(caller, ptr, "__writable_stream_handle__"))
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw) as u32)
}

fn transform_parts_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    transform: i64,
) -> Option<(i64, i64)> {
    let ptr = resolve_handle(caller, transform)?;
    let transform_handle = read_object_property_by_name(caller, ptr, "__transform_stream_handle__")
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw) as usize);
    if let Some(handle) = transform_handle {
        let table = caller
            .data()
            .transform_stream_table
            .lock()
            .expect("transform stream mutex");
        let entry = table.get(handle)?;
        return Some((entry.readable_obj?, entry.writable_obj?));
    }
    let readable = read_object_property_by_name(caller, ptr, "readable")?;
    let writable = read_object_property_by_name(caller, ptr, "writable")?;
    Some((readable, writable))
}

fn readable_stream_pipe_to(
    caller: &mut Caller<'_, RuntimeState>,
    readable_handle: u32,
    destination: i64,
) -> Option<i64> {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(writable_handle) = writable_stream_handle_from_object(caller, destination) else {
        reject_promise_with_type_error(
            caller,
            promise,
            "pipeTo destination must be a WritableStream",
        );
        return Some(promise);
    };
    let (controller_handle, stream_state) = {
        let table = caller
            .data()
            .readable_stream_table
            .lock()
            .expect("stream mutex");
        let entry = table.get(readable_handle as usize)?;
        (entry.controller_handle, entry.state.clone())
    };
    let chunks = if let Some(ctrl_handle) = controller_handle {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(ctrl_handle as usize) {
            ctrl.chunk_queue.drain(..).collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    for chunk in chunks {
        let write_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        call_transform_from_writable(caller, writable_handle, chunk, write_promise);
    }
    if matches!(stream_state, StreamState::Closed) {
        let close_deferred = call_flush_from_writable_close(caller, writable_handle, promise);
        if !close_deferred {
            settle_promise(
                caller.data(),
                promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
        }
    } else {
        settle_promise(
            caller.data(),
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
    Some(promise)
}

fn readable_stream_pipe_through(
    caller: &mut Caller<'_, RuntimeState>,
    readable_handle: u32,
    transform: i64,
) -> Option<i64> {
    let Some((readable, writable)) = transform_parts_from_object(caller, transform) else {
        return Some(type_error_exception(
            caller,
            "pipeThrough transform must contain readable and writable",
        ));
    };
    let _ = readable_stream_pipe_to(caller, readable_handle, writable);
    Some(readable)
}

// ── ReadableStreamDefaultReader 方法分发 ────────────────────────────────────

/// ReadableStreamDefaultReader 方法分发
pub(crate) fn call_default_reader_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: ReadableStreamDefaultReaderMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        ReadableStreamDefaultReaderMethodKind::Read => {
            // 1. 从 reader_table 获取 stream_handle / reader kind
            let (stream_handle, reader_kind) = {
                let reader_table = caller.data().reader_table.lock().expect("reader mutex");
                let reader = reader_table.get(handle as usize)?;
                (reader.stream_handle, reader.kind)
            };
            let byob_view = if reader_kind == ReaderKind::Byob {
                Some(
                    args.first()
                        .copied()
                        .unwrap_or_else(value::encode_undefined),
                )
            } else {
                None
            };

            // 2. 获取 controller handle 和 stream 状态
            let (controller_handle, http_response_handle, stream_state) = {
                let stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                let entry = stream_table.get(stream_handle as usize)?;
                (
                    entry.controller_handle,
                    entry.http_response_handle,
                    entry.state.clone(),
                )
            };

            // 3. 自定义流路径：检查 controller chunk_queue
            if let Some(ctrl_handle) = controller_handle {
                let chunk = {
                    let mut ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .lock()
                        .expect("controller mutex");
                    ctrl_table
                        .get_mut(ctrl_handle as usize)
                        .and_then(|ctrl| ctrl.chunk_queue.pop_front())
                };

                if let Some(chunk_val) = chunk {
                    // 有 chunk → 立即返回 Promise(value, done:false)
                    let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
                    if reader_kind == ReaderKind::Byob {
                        if let Some(view) = byob_view {
                            fulfill_byob_read(caller, ctrl_handle, chunk_val, view, p);
                        } else {
                            reject_promise_with_type_error(caller, p, "BYOB read requires a view");
                        }
                    } else {
                        let result = build_reader_result(caller, false, Some(chunk_val));
                        settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
                    }
                    return Some(p);
                }

                // 检查 close_requested 或 stream state
                let close_requested = {
                    let ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .lock()
                        .expect("controller mutex");
                    ctrl_table
                        .get(ctrl_handle as usize)
                        .map(|c| c.close_requested)
                        .unwrap_or(false)
                };

                if close_requested || matches!(stream_state, StreamState::Closed) {
                    let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
                    let result = build_reader_result(caller, true, byob_view);
                    settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
                    return Some(p);
                }

                if matches!(stream_state, StreamState::Errored) {
                    let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
                    let err = alloc_type_error_from_caller(caller, "Stream errored");
                    settle_promise(caller.data(), p, PromiseSettlement::Reject(err));
                    return Some(p);
                }

                // pending：存储 pending_read_promise
                let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
                {
                    let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
                    if let Some(reader) = reader_table.get_mut(handle as usize) {
                        reader.pending_read_promise = Some(p);
                        reader.pending_byob_view = byob_view;
                    }
                }
                return Some(p);
            }

            // 4. HTTP 路径：检查 http_response_handle
            if let Some(http_handle) = http_response_handle {
                // 转发到 fetch_core.rs 的现有 HTTP 逻辑
                return call_reader_http_read(caller, handle, http_handle);
            }

            // 5. 无 controller 且无 HTTP → closed
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let result = build_reader_result(caller, true, byob_view);
            settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
            Some(p)
        }
        ReadableStreamDefaultReaderMethodKind::ReleaseLock => {
            // 设 stream.locked = false
            let stream_handle = {
                let reader_table = caller.data().reader_table.lock().expect("reader mutex");
                reader_table.get(handle as usize)?.stream_handle
            };
            let mut stream_table = caller
                .data()
                .readable_stream_table
                .lock()
                .expect("stream mutex");
            if let Some(entry) = stream_table.get_mut(stream_handle as usize) {
                entry.locked = false;
            }
            Some(value::encode_undefined())
        }
        ReadableStreamDefaultReaderMethodKind::GetClosed => {
            // 返回 closed_promise
            let reader_table = caller.data().reader_table.lock().expect("reader mutex");
            let reader = reader_table.get(handle as usize)?;
            let promise = reader
                .closed_promise
                .unwrap_or_else(value::encode_undefined);
            Some(promise)
        }
    }
}

/// HTTP 路径的 reader.read() — 转发到 fetch_core.rs 的现有逻辑
fn call_reader_http_read(
    caller: &mut Caller<'_, RuntimeState>,
    _reader_handle: u32,
    http_handle: u32,
) -> Option<i64> {
    let response = {
        let mut http_table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        http_table
            .get_mut(http_handle as usize)
            .and_then(|e| e.response.take())
    };
    if response.is_none() {
        let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = build_reader_result(caller, true, None);
        settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
        return Some(p);
    }
    let mut response = response.unwrap();
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let tx = caller.data().host_completion_tx.clone()?;
    let promise_clone = promise;
    tokio::spawn(async move {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
                    promise: promise_clone,
                    materialize: Box::new(move |store, env| {
                        let arr = create_uint8array_with_env(store, env, &chunk);
                        let result = build_reader_result_with_env(store, env, false, Some(arr));
                        PromiseSettlement::Fulfill(result)
                    }),
                });
            }
            Ok(None) => {
                let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
                    promise: promise_clone,
                    materialize: Box::new(move |store, env| {
                        let result = build_reader_result_with_env(store, env, true, None);
                        PromiseSettlement::Fulfill(result)
                    }),
                });
            }
            Err(e) => {
                let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
                    promise: promise_clone,
                    materialize: Box::new(move |store, env| {
                        let err = crate::runtime_heap::alloc_type_error_with_env(
                            store,
                            env,
                            e.to_string(),
                        );
                        PromiseSettlement::Reject(err)
                    }),
                });
            }
        }
    });
    Some(promise)
}

/// 辅助：使用 env 构建 reader result（用于 Materialize 回调）
pub(crate) fn build_reader_result_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    done: bool,
    value: Option<i64>,
) -> i64 {
    let obj = crate::runtime_heap::alloc_host_object(ctx, env, 2);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "done",
        wjsm_ir::value::encode_bool(done),
    );
    let val = value.unwrap_or_else(wjsm_ir::value::encode_undefined);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx, env, obj, "value", val,
    );
    obj
}

/// 辅助：使用 env 创建 Uint8Array（用于 HTTP chunk 回调）
fn create_uint8array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: &[u8],
) -> i64 {
    let ab_handle = {
        let mut store = ctx.as_context_mut();
        let mut ab_table = store.data_mut().arraybuffer_table.lock().expect("mutex");
        let handle = ab_table.len() as u32;
        ab_table.push(ArrayBufferEntry {
            data: bytes.to_vec(),
        });
        handle
    };
    let ta_handle = {
        let mut store = ctx.as_context_mut();
        let mut ta_table = store.data_mut().typedarray_table.lock().expect("mutex");
        let handle = ta_table.len() as u32;
        ta_table.push(TypedArrayEntry {
            buffer_handle: ab_handle,
            byte_offset: 0,
            length: bytes.len() as u32,
            element_size: 1,
            element_kind: 1,
            is_shared: false,
        });
        handle
    };
    let obj = crate::runtime_heap::alloc_host_object(ctx, env, 4);
    let ta_handle_val = wjsm_ir::value::encode_f64(ta_handle as f64);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__typedarray_handle__",
        ta_handle_val,
    );
    let ab_handle_val = wjsm_ir::value::encode_f64(ab_handle as f64);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        obj,
        "__arraybuffer_handle__",
        ab_handle_val,
    );
    let len_val = wjsm_ir::value::encode_f64(bytes.len() as f64);
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
        wjsm_ir::value::encode_f64(0.0),
    );
    obj
}

// ── ReadableStreamDefaultController 方法分发 ────────────────────────────────

/// ReadableStreamDefaultController 方法分发
pub(crate) fn call_default_controller_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: ReadableStreamDefaultControllerMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        ReadableStreamDefaultControllerMethodKind::Enqueue => {
            controller_enqueue(caller, handle, args)
        }
        ReadableStreamDefaultControllerMethodKind::Close => controller_close(caller, handle),
        ReadableStreamDefaultControllerMethodKind::Error => controller_error(caller, handle, args),
        ReadableStreamDefaultControllerMethodKind::GetDesiredSize => {
            // 计算 high_water_mark - queue.len() 返回 number
            let table = caller
                .data()
                .stream_controller_table
                .lock()
                .expect("controller mutex");
            if let Some(ctrl) = table.get(handle as usize) {
                let desired = ctrl.high_water_mark - ctrl.chunk_queue.len() as f64;
                Some(value::encode_f64(desired))
            } else {
                Some(value::encode_null())
            }
        }
    }
}
