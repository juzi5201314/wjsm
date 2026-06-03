// WritableStream 核心实现（WHATWG Streams Phase 4）
// 包含：构造函数、DefaultController、DefaultWriter、locked getter、close/abort

use crate::*;
use std::collections::VecDeque;
use super::fetch_core::{push_native_callable, alloc_type_error_from_caller};

/// 创建 TypeError 异常值（NaN-boxed TAG_EXCEPTION）
fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    alloc_type_error_from_caller(caller, message)
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 创建 WritableStream JS 对象（包含 __writable_stream_handle__、locked getter、getWriter、abort、close）
fn create_writable_stream_js_object(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 5);

    // __writable_stream_handle__ = handle
    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__writable_stream_handle__", handle_val);

    // locked → accessor getter
    let locked_callable = NativeCallable::WritableStreamMethod {
        handle: stream_handle,
        kind: WritableStreamMethodKind::GetLocked,
    };
    let locked_idx = push_native_callable(caller, locked_callable);
    let locked_getter = value::encode_native_callable_idx(locked_idx);
    let undef = value::encode_undefined();
    let _ = define_host_accessor_property_with_env(caller, &env, obj, "locked", locked_getter, undef);

    // getWriter() → method
    let get_writer_callable = NativeCallable::WritableStreamMethod {
        handle: stream_handle,
        kind: WritableStreamMethodKind::GetWriter,
    };
    let get_writer_idx = push_native_callable(caller, get_writer_callable);
    let get_writer_val = value::encode_native_callable_idx(get_writer_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "getWriter", get_writer_val);

    // abort(reason?) → method
    let abort_callable = NativeCallable::WritableStreamMethod {
        handle: stream_handle,
        kind: WritableStreamMethodKind::Abort,
    };
    let abort_idx = push_native_callable(caller, abort_callable);
    let abort_val = value::encode_native_callable_idx(abort_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "abort", abort_val);

    // close() → method
    let close_callable = NativeCallable::WritableStreamMethod {
        handle: stream_handle,
        kind: WritableStreamMethodKind::Close,
    };
    let close_idx = push_native_callable(caller, close_callable);
    let close_val = value::encode_native_callable_idx(close_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "close", close_val);

    obj
}

/// 创建 WritableStreamDefaultController JS 对象（带 error 方法）
fn create_writable_controller_object(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);

    // __controller_handle__ — 内部标识
    let handle_val = value::encode_f64(controller_handle as f64);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__controller_handle__",
        handle_val,
    );

    // error(e)
    let error_callable = NativeCallable::WritableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: WritableStreamDefaultControllerMethodKind::Error,
    };
    let error_idx = push_native_callable(caller, error_callable);
    let error_val = value::encode_native_callable_idx(error_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "error", error_val);

    obj
}

/// 创建 WritableStreamDefaultWriter JS 对象
fn create_writer_js_object(
    caller: &mut Caller<'_, RuntimeState>,
    writer_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 7);

    // __writer_handle__ = handle
    let handle_val = value::encode_f64(writer_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__writer_handle__", handle_val);

    // write(chunk) → method
    let write_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::Write,
    };
    let write_idx = push_native_callable(caller, write_callable);
    let write_val = value::encode_native_callable_idx(write_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "write", write_val);

    // close() → method
    let close_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::Close,
    };
    let close_idx = push_native_callable(caller, close_callable);
    let close_val = value::encode_native_callable_idx(close_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "close", close_val);

    // abort(reason?) → method
    let abort_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::Abort,
    };
    let abort_idx = push_native_callable(caller, abort_callable);
    let abort_val = value::encode_native_callable_idx(abort_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "abort", abort_val);

    // closed → accessor getter
    let closed_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::GetClosed,
    };
    let closed_idx = push_native_callable(caller, closed_callable);
    let closed_getter = value::encode_native_callable_idx(closed_idx);
    let undef = value::encode_undefined();
    let _ = define_host_accessor_property_with_env(caller, &env, obj, "closed", closed_getter, undef);

    // ready → accessor getter
    let ready_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::GetReady,
    };
    let ready_idx = push_native_callable(caller, ready_callable);
    let ready_getter = value::encode_native_callable_idx(ready_idx);
    let _ = define_host_accessor_property_with_env(caller, &env, obj, "ready", ready_getter, undef);

    // desiredSize → accessor getter
    let desired_size_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::GetDesiredSize,
    };
    let desired_size_idx = push_native_callable(caller, desired_size_callable);
    let desired_size_getter = value::encode_native_callable_idx(desired_size_idx);
    let _ = define_host_accessor_property_with_env(caller, &env, obj, "desiredSize", desired_size_getter, undef);

    obj
}

// ── WritableStream 构造函数 ─────────────────────────────────────────────────

/// WritableStream 构造函数 — 由 NativeCallable::WritableStreamConstructor 调度
pub(crate) async fn construct_writable_stream(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    // 1. 解析 underlyingSink (args[0]) 和 strategy (args[1])
    let sink = args.first().copied().unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);

    // 2. 解析 strategy.highWaterMark（默认 1）
    let high_water_mark = if value::is_object(strategy) {
        let strategy_ptr = resolve_handle(caller, strategy);
        if let Some(ptr) = strategy_ptr {
            let hwm_val = read_object_property_by_name(caller, ptr, "highWaterMark")
                .unwrap_or_else(value::encode_undefined);
            if value::is_f64(hwm_val) {
                let v = value::decode_f64(hwm_val);
                if v >= 0.0 && v.is_finite() {
                    v
                } else {
                    1.0
                }
            } else {
                1.0
            }
        } else {
            1.0
        }
    } else {
        1.0
    };

    // 3. 创建 StreamControllerEntry (ControllerKind::Writable)
    let controller_handle = {
        let mut table = caller
            .data()
            .stream_controller_table
            .lock()
            .expect("controller mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::Writable,
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

    // 4. 创建 WritableStreamEntry
    let stream_handle = {
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
            controller_handle: Some(controller_handle),
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

    // 6. 如果 sink.start 存在：创建 controller JS 对象，调用 start(controller)
    let controller_obj = create_writable_controller_object(caller, controller_handle);

    if value::is_object(sink) {
        let sink_ptr = resolve_handle(caller, sink);
        if let Some(ptr) = sink_ptr {
            let start_fn = read_object_property_by_name(caller, ptr, "start")
                .unwrap_or_else(value::encode_undefined);
            if value::is_callable(start_fn) {
                // 调用 sink.start(controller)
                let _ = call_wasm_callback_async(caller, start_fn, sink, &[controller_obj]).await;
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

    // 8. 构造 WritableStream JS 对象
    let obj = create_writable_stream_js_object(caller, stream_handle);

    Some(obj)
}

// ── WritableStream 方法分发 ─────────────────────────────────────────────────

/// WritableStream 方法分发
pub(crate) fn call_writable_stream_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: WritableStreamMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        WritableStreamMethodKind::GetLocked => {
            // 从 writable_stream_table 读取 locked 返回 bool
            let table = caller
                .data()
                .writable_stream_table
                .lock()
                .expect("writable stream mutex");
            let locked = table
                .get(handle as usize)
                .map(|e| e.locked)
                .unwrap_or(false);
            Some(value::encode_bool(locked))
        }
        WritableStreamMethodKind::GetWriter => {
            // 检查 locked
            let locked = {
                let mut stream_table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                let entry = stream_table.get_mut(handle as usize)?;
                if entry.locked {
                    true
                } else {
                    entry.locked = true;
                    false
                }
            };
            if locked {
                return Some(type_error_exception(
                    caller,
                    "WritableStream is already locked to a writer",
                ));
            }

            // 创建 writer + closed_promise + ready_promise
            let closed_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let ready_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            // ready promise 在 Writable 状态下立即 resolve
            {
                let table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                if let Some(entry) = table.get(handle as usize) {
                    if entry.state == WritableStreamState::Writable {
                        settle_promise(caller.data(), ready_promise, PromiseSettlement::Fulfill(value::encode_undefined()));
                    }
                }
            }

            let writer_handle = {
                let mut table = caller.data().writer_table.lock().expect("writer mutex");
                let wh = table.len() as u32;
                table.push(WriterEntry {
                    writable_stream_handle: handle,
                    closed_promise: Some(closed_promise),
                    ready_promise: Some(ready_promise),
                });
                wh
            };

            // 如果流已关闭，立即 resolve closed promise
            {
                let table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                if let Some(entry) = table.get(handle as usize) {
                    if entry.state == WritableStreamState::Closed {
                        settle_promise(caller.data(), closed_promise, PromiseSettlement::Fulfill(value::encode_undefined()));
                    } else if entry.state == WritableStreamState::Errored {
                        let err = entry.error.unwrap_or_else(value::encode_undefined);
                        settle_promise(caller.data(), closed_promise, PromiseSettlement::Reject(err));
                        settle_promise(caller.data(), ready_promise, PromiseSettlement::Reject(err));
                    }
                }
            }

            // 构造 writer JS 对象
            let obj = create_writer_js_object(caller, writer_handle);

            Some(obj)
        }
        WritableStreamMethodKind::Abort => {
            let reason = args.first().copied().unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            // 设置状态为 Errored
            {
                let mut table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.state = WritableStreamState::Errored;
                    entry.error = Some(reason);
                }
            }

            // 如果有 writer，reject 其 closed 和 ready promise
            {
                let writer_table = caller.data().writer_table.lock().expect("writer mutex");
                for writer_entry in writer_table.iter() {
                    if writer_entry.writable_stream_handle == handle {
                        if let Some(cp) = writer_entry.closed_promise {
                            settle_promise(caller.data(), cp, PromiseSettlement::Reject(reason));
                        }
                        if let Some(rp) = writer_entry.ready_promise {
                            settle_promise(caller.data(), rp, PromiseSettlement::Reject(reason));
                        }
                    }
                }
            }

            // 调用 sink.abort(reason) 如果存在
            let ctrl_handle = {
                let table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                table.get(handle as usize).and_then(|e| e.controller_handle)
            };
            if let Some(_ch) = ctrl_handle {
                // 通过 controller 的 stream_handle 获取 sink
                // sink 存储在构造时的 source 对象中，这里简化处理：直接 resolve promise
            }

            settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
            Some(promise)
        }
        WritableStreamMethodKind::Close => {
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            // 获取 controller handle 和当前状态
            let (ctrl_handle, current_state) = {
                let table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                table.get(handle as usize).map(|e| (e.controller_handle, e.state)).unwrap_or((None, WritableStreamState::Closed))
            };

            if current_state != WritableStreamState::Writable {
                // 非可写状态直接 reject
                let err = type_error_exception(caller, "WritableStream is not in writable state");
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                return Some(promise);
            }

            // 设置状态为 Closing
            {
                let mut table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.state = WritableStreamState::Closing;
                }
            }

            // 标记 controller close_requested
            if let Some(ch) = ctrl_handle {
                let mut ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                if let Some(ctrl) = ctrl_table.get_mut(ch as usize) {
                    ctrl.close_requested = true;
                }
            }

            // 设置状态为 Closed，resolve close promise
            {
                let mut table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.state = WritableStreamState::Closed;
                }
            }

            // resolve writer 的 closed_promise
            {
                let writer_table = caller.data().writer_table.lock().expect("writer mutex");
                for writer_entry in writer_table.iter() {
                    if writer_entry.writable_stream_handle == handle {
                        if let Some(cp) = writer_entry.closed_promise {
                            settle_promise(caller.data(), cp, PromiseSettlement::Fulfill(value::encode_undefined()));
                        }
                    }
                }
            }

            settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
            Some(promise)
        }
    }
}

// ── WritableStreamDefaultWriter 方法分发 ────────────────────────────────────

/// WritableStreamDefaultWriter 方法分发
pub(crate) fn call_default_writer_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: WritableStreamDefaultWriterMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        WritableStreamDefaultWriterMethodKind::Write => {
            // writer.write(chunk)
            let _chunk = args.first().copied().unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            // 获取 stream handle
            let stream_handle = {
                let table = caller.data().writer_table.lock().expect("writer mutex");
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            let stream_handle = match stream_handle {
                Some(sh) => sh,
                None => {
                    let err = type_error_exception(caller, "writer is not attached to a stream");
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                    return Some(promise);
                }
            };

            // 检查流状态
            let state = {
                let table = caller
                    .data()
                    .writable_stream_table
                    .lock()
                    .expect("writable stream mutex");
                table.get(stream_handle as usize).map(|e| e.state)
            };

            match state {
                Some(WritableStreamState::Errored) => {
                    let err = {
                        let table = caller
                            .data()
                            .writable_stream_table
                            .lock()
                            .expect("writable stream mutex");
                        table.get(stream_handle as usize).and_then(|e| e.error)
                    };
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(
                        err.unwrap_or_else(value::encode_undefined),
                    ));
                }
                Some(WritableStreamState::Closed) | Some(WritableStreamState::Closing) => {
                    let err = type_error_exception(caller, "Cannot write to a closing/closed stream");
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                }
                Some(WritableStreamState::Writable) => {
                    // 简化实现：直接 resolve write promise
                    // 在完整实现中应调用 sink.write(chunk, controller)
                    settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
                }
                None => {
                    let err = type_error_exception(caller, "stream not found");
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                }
            }

            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::Close => {
            // writer.close() — 释放锁并关闭流
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            let stream_handle = {
                let table = caller.data().writer_table.lock().expect("reader mutex");
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 调用 stream.close 逻辑
                let (ctrl_handle, current_state) = {
                    let table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    table.get(sh as usize).map(|e| (e.controller_handle, e.state)).unwrap_or((None, WritableStreamState::Closed))
                };

                if current_state == WritableStreamState::Writable {
                    // 设置 Closing
                    {
                        let mut table = caller
                            .data()
                            .writable_stream_table
                            .lock()
                            .expect("writable stream mutex");
                        if let Some(entry) = table.get_mut(sh as usize) {
                            entry.state = WritableStreamState::Closing;
                        }
                    }

                    // 标记 close_requested
                    if let Some(ch) = ctrl_handle {
                        let mut ctrl_table = caller
                            .data()
                            .stream_controller_table
                            .lock()
                            .expect("controller mutex");
                        if let Some(ctrl) = ctrl_table.get_mut(ch as usize) {
                            ctrl.close_requested = true;
                        }
                    }

                    // 设置 Closed
                    {
                        let mut table = caller
                            .data()
                            .writable_stream_table
                            .lock()
                            .expect("writable stream mutex");
                        if let Some(entry) = table.get_mut(sh as usize) {
                            entry.state = WritableStreamState::Closed;
                        }
                    }
                }

                // 释放锁
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.locked = false;
                    }
                }

                // resolve writer closed_promise
                {
                    let writer_table = caller.data().writer_table.lock().expect("writer mutex");
                    for writer_entry in writer_table.iter() {
                        if writer_entry.writable_stream_handle == sh {
                            if let Some(cp) = writer_entry.closed_promise {
                                settle_promise(caller.data(), cp, PromiseSettlement::Fulfill(value::encode_undefined()));
                            }
                        }
                    }
                }

                settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
            } else {
                let err = type_error_exception(caller, "writer is not attached to a stream");
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
            }

            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::Abort => {
            // writer.abort(reason) — 释放锁并中止流
            let reason = args.first().copied().unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            let stream_handle = {
                let table = caller.data().writer_table.lock().expect("writer mutex");
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 设置 Errored
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.state = WritableStreamState::Errored;
                        entry.error = Some(reason);
                    }
                }

                // 释放锁
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.locked = false;
                    }
                }

                // reject writer closed 和 ready promise
                {
                    let writer_table = caller.data().writer_table.lock().expect("writer mutex");
                    for writer_entry in writer_table.iter() {
                        if writer_entry.writable_stream_handle == sh {
                            if let Some(cp) = writer_entry.closed_promise {
                                settle_promise(caller.data(), cp, PromiseSettlement::Reject(reason));
                            }
                            if let Some(rp) = writer_entry.ready_promise {
                                settle_promise(caller.data(), rp, PromiseSettlement::Reject(reason));
                            }
                        }
                    }
                }

                settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
            } else {
                let err = type_error_exception(caller, "writer is not attached to a stream");
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
            }

            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::GetClosed => {
            // 返回 closed promise
            let table = caller.data().writer_table.lock().expect("writer mutex");
            table.get(handle as usize).and_then(|e| e.closed_promise)
        }
        WritableStreamDefaultWriterMethodKind::GetReady => {
            // 返回 ready promise
            let table = caller.data().writer_table.lock().expect("writer mutex");
            table.get(handle as usize).and_then(|e| e.ready_promise)
        }
        WritableStreamDefaultWriterMethodKind::GetDesiredSize => {
            // 返回 controller.desiredSize 值
            let stream_handle = {
                let table = caller.data().writer_table.lock().expect("writer mutex");
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                let ctrl_handle = {
                    let table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    table.get(sh as usize).and_then(|e| e.controller_handle)
                };

                if let Some(ch) = ctrl_handle {
                    let ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .lock()
                        .expect("controller mutex");
                    if let Some(ctrl) = ctrl_table.get(ch as usize) {
                        let desired = ctrl.high_water_mark - ctrl.chunk_queue.len() as f64;
                        return Some(value::encode_f64(desired));
                    }
                }
            }
            Some(value::encode_null())
        }
    }
}

// ── WritableStreamDefaultController 方法分发 ────────────────────────────────

/// WritableStreamDefaultController 方法分发
pub(crate) fn call_writable_controller_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: WritableStreamDefaultControllerMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        WritableStreamDefaultControllerMethodKind::Error => {
            // controller.error(e)
            let error_val = args.first().copied().unwrap_or_else(value::encode_undefined);

            // 获取关联的 stream handle
            let stream_handle = {
                let table = caller
                    .data()
                    .stream_controller_table
                    .lock()
                    .expect("controller mutex");
                table.get(handle as usize).map(|e| e.stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 设置 WritableStream 为 Errored
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .lock()
                        .expect("writable stream mutex");
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.state = WritableStreamState::Errored;
                        entry.error = Some(error_val);
                    }
                }

                // reject writer 的 closed 和 ready promise
                {
                    let writer_table = caller.data().writer_table.lock().expect("writer mutex");
                    for writer_entry in writer_table.iter() {
                        if writer_entry.writable_stream_handle == sh {
                            if let Some(cp) = writer_entry.closed_promise {
                                settle_promise(caller.data(), cp, PromiseSettlement::Reject(error_val));
                            }
                            if let Some(rp) = writer_entry.ready_promise {
                                settle_promise(caller.data(), rp, PromiseSettlement::Reject(error_val));
                            }
                        }
                    }
                }
            }

            Some(value::encode_undefined())
        }
    }
}
