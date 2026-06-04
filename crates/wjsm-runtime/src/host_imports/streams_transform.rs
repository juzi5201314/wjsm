// TransformStream 核心实现（WHATWG Streams Phase 5）
// 包含：构造函数、readable/writable getter、transform 回调调度、flush 回调调度

use super::fetch_core::{alloc_type_error_from_caller, push_native_callable};
use super::streams_readable::create_controller_object;
use super::streams_writable::{
    create_writable_abort_signal_object, create_writable_stream_js_object,
};
use crate::*;
use std::collections::VecDeque;

/// 创建 TypeError 异常值（NaN-boxed TAG_EXCEPTION）
#[allow(dead_code)]
fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    alloc_type_error_from_caller(caller, message)
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 创建 TransformStream JS 对象（包含 __transform_stream_handle__、readable getter、writable getter）
fn create_transform_stream_js_object(caller: &mut Caller<'_, RuntimeState>, ts_handle: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 3);

    // __transform_stream_handle__ — 内部标识
    let handle_val = value::encode_f64(ts_handle as f64);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__transform_stream_handle__",
        handle_val,
    );

    // readable → accessor getter
    let readable_callable = NativeCallable::TransformStreamMethod {
        handle: ts_handle,
        kind: TransformStreamMethodKind::GetReadable,
    };
    let readable_idx = push_native_callable(caller, readable_callable);
    let readable_getter = value::encode_native_callable_idx(readable_idx);
    let undef = value::encode_undefined();
    let _ = define_host_accessor_property_with_env(
        caller,
        &env,
        obj,
        "readable",
        readable_getter,
        undef,
    );

    // writable → accessor getter
    let writable_callable = NativeCallable::TransformStreamMethod {
        handle: ts_handle,
        kind: TransformStreamMethodKind::GetWritable,
    };
    let writable_idx = push_native_callable(caller, writable_callable);
    let writable_getter = value::encode_native_callable_idx(writable_idx);
    let _ = define_host_accessor_property_with_env(
        caller,
        &env,
        obj,
        "writable",
        writable_getter,
        undef,
    );

    obj
}

/// 为 TransformStream 创建 ReadableStream JS 对象
fn create_readable_stream_js_object_for_transform(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 6);

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

    obj
}

// ── TransformStream 构造函数 ─────────────────────────────────────────────────

/// TransformStream 构造函数 — 由 NativeCallable::TransformStreamConstructor 调度
/// 规范：https://streams.spec.whatwg.org/#ts-constructor
pub(crate) async fn construct_transform_stream(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    // 1. 解析 transformer (args[0]) 和 writableStrategy (args[1]) / readableStrategy (args[2])
    let transformer = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let writable_strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let readable_strategy = args.get(2).copied().unwrap_or_else(value::encode_undefined);

    // 2. 从 transformer 对象提取 transform/flush 方法
    let (transform_fn, flush_fn) = if value::is_object(transformer) {
        let transformer_ptr = resolve_handle(caller, transformer);
        if let Some(ptr) = transformer_ptr {
            let tf = read_object_property_by_name(caller, ptr, "transform")
                .unwrap_or_else(value::encode_undefined);
            let fl = read_object_property_by_name(caller, ptr, "flush")
                .unwrap_or_else(value::encode_undefined);
            (
                if value::is_callable(tf) {
                    Some(tf)
                } else {
                    None
                },
                if value::is_callable(fl) {
                    Some(fl)
                } else {
                    None
                },
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // 3. 解析 readable strategy 的高水位标记
    let readable_hwm = if value::is_object(readable_strategy) {
        let ptr = resolve_handle(caller, readable_strategy);
        if let Some(p) = ptr {
            let hwm_val = read_object_property_by_name(caller, p, "highWaterMark")
                .unwrap_or_else(value::encode_undefined);
            if value::is_f64(hwm_val) {
                let v = value::decode_f64(hwm_val);
                if v >= 0.0 && v.is_finite() { v } else { 0.0 }
            } else {
                0.0
            }
        } else {
            0.0
        }
    } else {
        0.0
    };

    // 4. 解析 writable strategy 的高水位标记
    let writable_hwm = if value::is_object(writable_strategy) {
        let ptr = resolve_handle(caller, writable_strategy);
        if let Some(p) = ptr {
            let hwm_val = read_object_property_by_name(caller, p, "highWaterMark")
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

    // 5. 创建 ReadableStream 的 controller (ControllerKind::ReadableDefault)
    let readable_controller_handle = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::ReadableDefault,
            stream_handle: 0,
            chunk_queue: VecDeque::new(),
            high_water_mark: readable_hwm,
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

    // 6. 创建 ReadableStreamEntry（readable 侧）
    let readable_stream_handle = {
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
            controller_handle: Some(readable_controller_handle),
            is_byte_stream: false,
        });
        handle
    };

    // 7. 回写 stream_handle 到 readable controller，标记 started
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(readable_controller_handle as usize) {
            ctrl.stream_handle = readable_stream_handle;
            ctrl.started = true;
        }
    }

    // 8. 创建 WritableStream 的 controller (ControllerKind::Writable)
    let writable_controller_handle = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::Writable,
            stream_handle: 0,
            chunk_queue: VecDeque::new(),
            high_water_mark: writable_hwm,
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

    let writable_abort_signal = create_writable_abort_signal_object(caller);

    // 9. 创建 WritableStreamEntry（writable 侧）
    let writable_stream_handle = {
        let mut table = caller
            .data()
            .writable_stream_table
            .lock()
            .expect("writable stream mutex");
        let handle = table.len() as u32;
        table.push(WritableStreamEntry {
            state: WritableStreamState::Writable,
            error: None,
            locked: false,
            controller_handle: Some(writable_controller_handle),
            abort_signal: Some(writable_abort_signal),
        });
        handle
    };

    // 10. 回写 stream_handle 到 writable controller，标记 started
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        if let Some(ctrl) = table.get_mut(writable_controller_handle as usize) {
            ctrl.stream_handle = writable_stream_handle;
            ctrl.started = true;
        }
    }

    // 11. 创建 readable/writable JS 对象
    let readable_obj =
        create_readable_stream_js_object_for_transform(caller, readable_stream_handle);
    let writable_obj = create_writable_stream_js_object(caller, writable_stream_handle);

    // 12. 创建 TransformStreamEntry
    let ts_handle = {
        let mut table = caller
            .data()
            .transform_stream_table
            .lock()
            .expect("transform stream mutex");
        let handle = table.len() as u32;
        table.push(TransformStreamEntry {
            readable_stream_handle: Some(readable_stream_handle),
            writable_stream_handle: Some(writable_stream_handle),
            transform_callback: transform_fn,
            flush_callback: flush_fn,
            readable_controller_handle: Some(readable_controller_handle),
            transformer_this: if value::is_object(transformer) {
                Some(transformer)
            } else {
                None
            },
            backpressure: false,
            readable_obj: Some(readable_obj),
            writable_obj: Some(writable_obj),
        });
        handle
    };

    // 13. 构造 TransformStream JS 对象
    let obj = create_transform_stream_js_object(caller, ts_handle);

    Some(obj)
}

// ── TransformStream 方法分发 ─────────────────────────────────────────────────

/// TransformStream 方法分发
pub(crate) fn call_transform_stream_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: TransformStreamMethodKind,
    _args: &[i64],
) -> Option<i64> {
    match kind {
        TransformStreamMethodKind::GetReadable => {
            let table = caller
                .data()
                .transform_stream_table
                .lock()
                .expect("transform stream mutex");
            table.get(handle as usize).and_then(|e| e.readable_obj)
        }
        TransformStreamMethodKind::GetWritable => {
            let table = caller
                .data()
                .transform_stream_table
                .lock()
                .expect("transform stream mutex");
            table.get(handle as usize).and_then(|e| e.writable_obj)
        }
    }
}

/// TransformStream 的 transform 调度：当 writable 侧的 writer.write(chunk) 被调用时，
/// 从 transform_stream_table 查找关联的 TransformStreamEntry，调用 transform(chunk, controller)。
pub(crate) fn call_transform_from_writable(
    caller: &mut Caller<'_, RuntimeState>,
    writable_stream_handle: u32,
    chunk: i64,
    write_promise: i64,
) {
    // 1. 查找关联的 TransformStreamEntry
    let (transform_fn, readable_ctrl_handle, transformer_this) = {
        let table = caller
            .data()
            .transform_stream_table
            .lock()
            .expect("transform stream mutex");
        let entry = match table
            .iter()
            .find(|e| e.writable_stream_handle == Some(writable_stream_handle))
        {
            Some(e) => e,
            None => {
                // 不是 TransformStream 的 writable 侧 → 直接 resolve
                settle_promise(
                    caller.data(),
                    write_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
                return;
            }
        };
        (
            entry.transform_callback,
            entry.readable_controller_handle,
            entry.transformer_this,
        )
    };

    // 2. 创建 readable controller JS 对象（传递给 transform 回调）
    if let Some(ctrl_handle) = readable_ctrl_handle {
        let controller_obj = create_controller_object(caller, ctrl_handle);

        if let Some(tf) = transform_fn {
            // transform 回调必须先运行，writer.write() 的 promise 才能完成；
            // 否则后续 write/close 可能跑到 enqueue 之前，破坏 readable 侧顺序。
            let this_val = transformer_this.unwrap_or_else(value::encode_undefined);
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::TransformStreamTransform {
                callback: tf,
                this_val,
                chunk,
                controller: controller_obj,
                write_promise,
            });
        } else {
            // 默认 Identity Transform：直接将 chunk 推入 readable 侧
            // 检查是否有 pending read promise
            let stream_handle = {
                let ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                ctrl_table
                    .get(ctrl_handle as usize)
                    .map(|c| c.stream_handle)
            };

            if let Some(sh) = stream_handle {
                let pending = {
                    let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
                    let mut pending_promise: Option<i64> = None;
                    for reader in reader_table.iter_mut() {
                        if reader.stream_handle == sh {
                            if let Some(promise) = reader.pending_read_promise.take() {
                                pending_promise = Some(promise);
                                break;
                            }
                        }
                    }
                    pending_promise
                };

                if let Some(promise) = pending {
                    // 有等待中的 read → 立即 settle
                    let result = build_reader_result(caller, false, Some(chunk));
                    settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
                } else {
                    // 无等待 → 推入 chunk_queue
                    let mut ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .lock()
                        .expect("controller mutex");
                    if let Some(ctrl) = ctrl_table.get_mut(ctrl_handle as usize) {
                        if !ctrl.close_requested {
                            ctrl.chunk_queue.push_back(chunk);
                        }
                    }
                }
            }
            settle_promise(
                caller.data(),
                write_promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
        }
    } else {
        // 无 controller → 直接 resolve
        settle_promise(
            caller.data(),
            write_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
}

/// TransformStream 的 flush 调度：当 writable 侧关闭时，
/// 调用 flush(controller)，然后关闭 readable 侧。
pub(crate) fn call_flush_from_writable_close(
    caller: &mut Caller<'_, RuntimeState>,
    writable_stream_handle: u32,
    close_promise: i64,
) -> bool {
    // 1. 查找关联的 TransformStreamEntry
    let (flush_fn, readable_ctrl_handle, readable_stream_handle, transformer_this) = {
        let table = caller
            .data()
            .transform_stream_table
            .lock()
            .expect("transform stream mutex");
        let entry = match table
            .iter()
            .find(|e| e.writable_stream_handle == Some(writable_stream_handle))
        {
            Some(e) => e,
            None => return false,
        };
        (
            entry.flush_callback,
            entry.readable_controller_handle,
            entry.readable_stream_handle,
            entry.transformer_this,
        )
    };

    // 2. 将 flush + close 作为同一个微任务排队；排在已入队 transform 之后，
    // 避免 close_requested 提前阻止 transform(controller.enqueue)。
    match (readable_stream_handle, readable_ctrl_handle) {
        (Some(rs_handle), Some(ctrl_handle)) => {
            let controller_obj = create_controller_object(caller, ctrl_handle);
            let this_val = transformer_this.unwrap_or_else(value::encode_undefined);
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::TransformStreamFlush {
                callback: flush_fn,
                this_val,
                controller: controller_obj,
                readable_stream_handle: rs_handle,
                readable_controller_handle: ctrl_handle,
                close_promise,
            });
        }
        _ => {
            settle_promise(
                caller.data(),
                close_promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
        }
    }

    true
}
