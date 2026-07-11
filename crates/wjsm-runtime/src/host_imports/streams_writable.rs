// WritableStream 核心实现（WHATWG Streams Phase 4）
// 包含：构造函数、DefaultController、DefaultWriter、locked getter、close/abort

use super::fetch_core::{alloc_type_error_from_caller, push_native_callable};
use crate::*;
use std::collections::VecDeque;

/// 创建 TypeError 异常值（NaN-boxed TAG_EXCEPTION）
fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    alloc_type_error_from_caller(caller, message)
}

pub(crate) fn create_writable_abort_signal_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let signal_handle = {
        let mut table = caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(AbortSignalEntry {
            aborted: false,
            reason: None,
        });
        handle
    };
    let signal_obj = alloc_host_object(caller, &env, 2);
    let handle_val = value::encode_f64(signal_handle as f64);
    let _ = define_host_data_property_from_caller(
        caller,
        signal_obj,
        "__abort_signal_handle__",
        handle_val,
    );
    let _ = define_host_data_property_from_caller(
        caller,
        signal_obj,
        "aborted",
        value::encode_bool(false),
    );
    signal_obj
}

fn mark_writable_abort_signal_aborted(
    caller: &mut Caller<'_, RuntimeState>,
    signal_obj: i64,
    reason: i64,
) {
    let signal_handle = resolve_handle(caller, signal_obj)
        .and_then(|ptr| read_object_property_by_name(caller, ptr, "__abort_signal_handle__"))
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw) as usize);
    if let Some(handle) = signal_handle {
        let mut table = caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle) {
            entry.aborted = true;
            entry.reason = Some(reason);
        }
    }
    let _ =
        set_host_data_property_from_caller(caller, signal_obj, "aborted", value::encode_bool(true));
}

fn mark_writable_stream_signal_aborted(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
    reason: i64,
) {
    let signal_obj = {
        let table = caller
            .data()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(stream_handle as usize)
            .and_then(|entry| entry.abort_signal)
    };
    if let Some(signal_obj) = signal_obj {
        mark_writable_abort_signal_aborted(caller, signal_obj, reason);
    }
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 创建 WritableStream JS 对象（包含 __writable_stream_handle__、locked getter、getWriter、abort、close）
pub(crate) fn create_writable_stream_js_object(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 5);

    // __writable_stream_handle__ = handle
    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__writable_stream_handle__",
        handle_val,
    );

    // locked → accessor getter
    let locked_callable = NativeCallable::WritableStreamMethod {
        handle: stream_handle,
        kind: WritableStreamMethodKind::GetLocked,
    };
    let locked_idx = push_native_callable(caller, locked_callable);
    let locked_getter = value::encode_native_callable_idx(locked_idx);
    let undef = value::encode_undefined();
    let _ =
        define_host_accessor_property_with_env(caller, &env, obj, "locked", locked_getter, undef);

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

    if let Some(obj_handle) = weak_target_handle_index_of(caller, obj) {
        caller
            .data()
            .writable_stream_table
            .bind_obj_handle(obj_handle, stream_handle);
    }

    obj
}

/// 创建 WritableStreamDefaultController JS 对象（带 error 方法）
fn create_writable_controller_object(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 3);

    // __controller_handle__ — 内部标识
    let handle_val = value::encode_f64(controller_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__controller_handle__", handle_val);

    // error(e)
    let error_callable = NativeCallable::WritableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: WritableStreamDefaultControllerMethodKind::Error,
    };
    let error_idx = push_native_callable(caller, error_callable);
    let error_val = value::encode_native_callable_idx(error_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "error", error_val);

    // signal → accessor getter
    let signal_callable = NativeCallable::WritableStreamDefaultControllerMethod {
        handle: controller_handle,
        kind: WritableStreamDefaultControllerMethodKind::GetSignal,
    };
    let signal_idx = push_native_callable(caller, signal_callable);
    let signal_getter = value::encode_native_callable_idx(signal_idx);
    let undef = value::encode_undefined();
    let _ =
        define_host_accessor_property_with_env(caller, &env, obj, "signal", signal_getter, undef);

    if let Some(obj_handle) = weak_target_handle_index_of(caller, obj) {
        caller
            .data()
            .stream_controller_table
            .bind_obj_handle(obj_handle, controller_handle);
    }

    obj
}

/// 创建 WritableStreamDefaultWriter JS 对象
fn create_writer_js_object(caller: &mut Caller<'_, RuntimeState>, writer_handle: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);

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

    // releaseLock() → method
    let release_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::ReleaseLock,
    };
    let release_idx = push_native_callable(caller, release_callable);
    let release_val = value::encode_native_callable_idx(release_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "releaseLock", release_val);

    // closed → accessor getter
    let closed_callable = NativeCallable::WritableStreamDefaultWriterMethod {
        handle: writer_handle,
        kind: WritableStreamDefaultWriterMethodKind::GetClosed,
    };
    let closed_idx = push_native_callable(caller, closed_callable);
    let closed_getter = value::encode_native_callable_idx(closed_idx);
    let undef = value::encode_undefined();
    let _ =
        define_host_accessor_property_with_env(caller, &env, obj, "closed", closed_getter, undef);

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
    let _ = define_host_accessor_property_with_env(
        caller,
        &env,
        obj,
        "desiredSize",
        desired_size_getter,
        undef,
    );
    if let Some(obj_handle) = weak_target_handle_index_of(caller, obj) {
        caller
            .data()
            .writer_table
            .bind_obj_handle(obj_handle, writer_handle);
    }

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
    let sink = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let strategy = args.get(1).copied().unwrap_or_else(value::encode_undefined);

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

    // 3. 创建 StreamControllerEntry (ControllerKind::Writable)
    let controller_handle = caller
        .data()
        .stream_controller_table
        .alloc(StreamControllerEntry {
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
            underlying_source: None,
            pull_callback: None,
            cancel_callback: None,
            write_callback: None,
            sink_close_callback: None,
            active_byob_request: None,
        });

    let abort_signal = create_writable_abort_signal_object(caller);

    // 4. 创建 WritableStreamEntry
    let stream_handle = caller
        .data()
        .writable_stream_table
        .alloc(WritableStreamEntry {
            state: WritableStreamState::Writable,
            error: None,
            locked: false,
            controller_handle: Some(controller_handle),
            abort_signal: Some(abort_signal),
        });

    // 5. 回写 stream_handle 到 controller
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    // 6.5 保存 underlyingSink 与 write/close/abort 回调
    if value::is_object(sink)
        && let Some(ptr) = resolve_handle(caller, sink) {
            let write_fn = read_object_property_by_name(caller, ptr, "write")
                .unwrap_or_else(value::encode_undefined);
            let close_fn = read_object_property_by_name(caller, ptr, "close")
                .unwrap_or_else(value::encode_undefined);
            let abort_fn = read_object_property_by_name(caller, ptr, "abort")
                .unwrap_or_else(value::encode_undefined);
            let mut table = caller
                .data()
                .stream_controller_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ctrl) = table.get_mut(controller_handle as usize) {
                ctrl.underlying_source = Some(sink);
                if value::is_callable(write_fn) {
                    ctrl.write_callback = Some(write_fn);
                }
                if value::is_callable(close_fn) {
                    ctrl.sink_close_callback = Some(close_fn);
                }
                if value::is_callable(abort_fn) {
                    ctrl.cancel_callback = Some(abort_fn);
                }
            }
        }

    // 7. 标记 controller.started = true
    {
        let mut table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.started = true;
        }
    }

    // 8. 构造 WritableStream JS 对象
    let obj = create_writable_stream_js_object(caller, stream_handle);

    Some(obj)
}

/// 普通 WritableStream：排队调用 underlyingSink.write(chunk, controller)。
fn call_sink_write_from_writable(
    caller: &mut Caller<'_, RuntimeState>,
    writable_stream_handle: u32,
    chunk: i64,
    write_promise: i64,
) {
    let (write_fn, ctrl_handle, sink_this) = {
        let ws_table = caller
            .data()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctrl_handle = match ws_table.get(writable_stream_handle as usize) {
            Some(e) => e.controller_handle,
            None => {
                settle_promise(
                    caller.data(),
                    write_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
                return;
            }
        };
        let Some(ch) = ctrl_handle else {
            settle_promise(
                caller.data(),
                write_promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
            return;
        };
        let ctrl_table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctrl = match ctrl_table.get(ch as usize) {
            Some(c) => c,
            None => {
                settle_promise(
                    caller.data(),
                    write_promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
                return;
            }
        };
        (ctrl.write_callback, ch, ctrl.underlying_source)
    };

    let controller_obj = create_writable_controller_object(caller, ctrl_handle);
    if let Some(callback) = write_fn {
        let this_val = sink_this.unwrap_or_else(value::encode_undefined);
        let mut queue = caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        queue.push_back(Microtask::WritableStreamSinkWrite {
            callback,
            this_val,
            chunk,
            controller: controller_obj,
            write_promise,
        });
    } else {
        settle_promise(
            caller.data(),
            write_promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }
}

/// 普通 WritableStream 关闭：排队 underlyingSink.close(controller)；返回是否延后 resolve close_promise。
fn call_sink_close_from_writable_close(
    caller: &mut Caller<'_, RuntimeState>,
    writable_stream_handle: u32,
    close_promise: i64,
) -> bool {
    let (close_fn, ctrl_handle, sink_this) = {
        let ws_table = caller
            .data()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctrl_handle = match ws_table.get(writable_stream_handle as usize) {
            Some(e) => e.controller_handle,
            None => return false,
        };
        let Some(ch) = ctrl_handle else {
            return false;
        };
        let ctrl_table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctrl = match ctrl_table.get(ch as usize) {
            Some(c) => c,
            None => return false,
        };
        (ctrl.sink_close_callback, ch, ctrl.underlying_source)
    };

    let controller_obj = create_writable_controller_object(caller, ctrl_handle);
    let this_val = sink_this.unwrap_or_else(value::encode_undefined);
    let mut queue = caller
        .data()
        .microtask_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    queue.push_back(Microtask::WritableStreamSinkClose {
        callback: close_fn,
        this_val,
        controller: controller_obj,
        writable_stream_handle,
        close_promise,
    });
    true
}

pub(crate) fn finish_writable_stream_close<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    writable_stream_handle: u32,
    close_promise: i64,
) {
    {
        let mut table = ctx
            .state_mut()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(writable_stream_handle as usize) {
            entry.state = WritableStreamState::Closed;
        }
    }

    let closed_promises = {
        let writer_table = ctx
            .state_mut()
            .writer_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        writer_table
            .iter()
            .filter(|writer_entry| writer_entry.writable_stream_handle == writable_stream_handle)
            .filter_map(|writer_entry| writer_entry.closed_promise)
            .collect::<Vec<_>>()
    };
    for promise in closed_promises {
        settle_promise(
            ctx.state_mut(),
            promise,
            PromiseSettlement::Fulfill(value::encode_undefined()),
        );
    }

    settle_promise(
        ctx.state_mut(),
        close_promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
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
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get(handle as usize)
                    && entry.state == WritableStreamState::Writable
                {
                    settle_promise(
                        caller.data(),
                        ready_promise,
                        PromiseSettlement::Fulfill(value::encode_undefined()),
                    );
                }
            }

            let writer_handle = caller.data().writer_table.alloc(WriterEntry {
                writable_stream_handle: handle,
                closed_promise: Some(closed_promise),
                ready_promise: Some(ready_promise),
            });

            // 如果流已关闭，立即 resolve closed promise
            {
                let table = caller
                    .data()
                    .writable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get(handle as usize) {
                    if entry.state == WritableStreamState::Closed {
                        settle_promise(
                            caller.data(),
                            closed_promise,
                            PromiseSettlement::Fulfill(value::encode_undefined()),
                        );
                    } else if entry.state == WritableStreamState::Errored {
                        let err = entry.error.unwrap_or_else(value::encode_undefined);
                        settle_promise(
                            caller.data(),
                            closed_promise,
                            PromiseSettlement::Reject(err),
                        );
                        settle_promise(
                            caller.data(),
                            ready_promise,
                            PromiseSettlement::Reject(err),
                        );
                    }
                }
            }

            // 构造 writer JS 对象
            let obj = create_writer_js_object(caller, writer_handle);

            Some(obj)
        }
        WritableStreamMethodKind::Abort => {
            let reason = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            // 设置状态为 Errored
            {
                let mut table = caller
                    .data()
                    .writable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.state = WritableStreamState::Errored;
                    entry.error = Some(reason);
                }
            }
            mark_writable_stream_signal_aborted(caller, handle, reason);

            // 如果有 writer，reject 其 closed 和 ready promise
            {
                let writer_table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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

            settle_promise(
                caller.data(),
                promise,
                PromiseSettlement::Fulfill(value::encode_undefined()),
            );
            Some(promise)
        }
        WritableStreamMethodKind::Close => {
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            // 获取 controller handle 和当前状态
            let (ctrl_handle, current_state) = {
                let table = caller
                    .data()
                    .writable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table
                    .get(handle as usize)
                    .map(|e| (e.controller_handle, e.state))
                    .unwrap_or((None, WritableStreamState::Closed))
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.state = WritableStreamState::Closing;
                }
            }

            // 标记 controller close_requested
            if let Some(ch) = ctrl_handle {
                let mut ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ctrl) = ctrl_table.get_mut(ch as usize) {
                    ctrl.close_requested = true;
                }
            }

            let close_deferred = call_flush_from_writable_close(caller, handle, promise);
            if !close_deferred {
                let sink_deferred = call_sink_close_from_writable_close(caller, handle, promise);
                if !sink_deferred {
                    finish_writable_stream_close(caller, handle, promise);
                }
            }

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
            let chunk = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            // 获取 stream handle
            let stream_handle = {
                let table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(stream_handle as usize).map(|e| e.state)
            };
            match state {
                Some(WritableStreamState::Errored) => {
                    let err = {
                        let table = caller
                            .data()
                            .writable_stream_table
                            .inner
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        table.get(stream_handle as usize).and_then(|e| e.error)
                    };
                    settle_promise(
                        caller.data(),
                        promise,
                        PromiseSettlement::Reject(err.unwrap_or_else(value::encode_undefined)),
                    );
                }
                Some(WritableStreamState::Closed) | Some(WritableStreamState::Closing) => {
                    let err =
                        type_error_exception(caller, "Cannot write to a closing/closed stream");
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                }
                Some(WritableStreamState::Writable) => {
                    // 检查是否属于 TransformStream 的 writable 侧
                    let is_transform = {
                        let ts_table = caller
                            .data()
                            .transform_stream_table
                            .inner
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        ts_table
                            .iter()
                            .any(|e| e.writable_stream_handle == Some(stream_handle))
                    };
                    if is_transform {
                        // TransformStream 路径：调用 transform(chunk, controller)
                        call_transform_from_writable(caller, stream_handle, chunk, promise);
                    } else {
                        // 普通 WritableStream：调用 underlyingSink.write
                        call_sink_write_from_writable(caller, stream_handle, chunk, promise);
                    }
                }
                None => {
                    let err = type_error_exception(caller, "stream not found");
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
                }
            }
            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::Close => {
            // writer.close() — 关闭流（不释放锁；须 writer.releaseLock()）
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            let stream_handle = {
                let table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 调用 stream.close 逻辑
                let (ctrl_handle, current_state) = {
                    let table = caller
                        .data()
                        .writable_stream_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    table
                        .get(sh as usize)
                        .map(|e| (e.controller_handle, e.state))
                        .unwrap_or((None, WritableStreamState::Closed))
                };
                let mut close_deferred;

                if current_state == WritableStreamState::Writable {
                    // 设置 Closing
                    {
                        let mut table = caller
                            .data()
                            .writable_stream_table
                            .inner
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if let Some(entry) = table.get_mut(sh as usize) {
                            entry.state = WritableStreamState::Closing;
                        }
                    }

                    // 标记 close_requested
                    if let Some(ch) = ctrl_handle {
                        let mut ctrl_table = caller
                            .data()
                            .stream_controller_table
                            .inner
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if let Some(ctrl) = ctrl_table.get_mut(ch as usize) {
                            ctrl.close_requested = true;
                        }
                    }

                    // TransformStream 路径：flush + readable close 在已排队 transform 之后执行。
                    close_deferred = call_flush_from_writable_close(caller, sh, promise);
                    if !close_deferred {
                        close_deferred = call_sink_close_from_writable_close(caller, sh, promise);
                    }
                    if !close_deferred {
                        finish_writable_stream_close(caller, sh, promise);
                    }
                }
            } else {
                let err = type_error_exception(caller, "writer is not attached to a stream");
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
            }

            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::Abort => {
            // writer.abort(reason) — 中止流（不释放锁；须 writer.releaseLock()）
            let reason = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());

            let stream_handle = {
                let table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 设置 Errored
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.state = WritableStreamState::Errored;
                        entry.error = Some(reason);
                    }
                }
                mark_writable_stream_signal_aborted(caller, sh, reason);

                // reject writer closed 和 ready promise
                {
                    let writer_table = caller
                        .data()
                        .writer_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for writer_entry in writer_table.iter() {
                        if writer_entry.writable_stream_handle == sh {
                            if let Some(cp) = writer_entry.closed_promise {
                                settle_promise(
                                    caller.data(),
                                    cp,
                                    PromiseSettlement::Reject(reason),
                                );
                            }
                            if let Some(rp) = writer_entry.ready_promise {
                                settle_promise(
                                    caller.data(),
                                    rp,
                                    PromiseSettlement::Reject(reason),
                                );
                            }
                        }
                    }
                }

                settle_promise(
                    caller.data(),
                    promise,
                    PromiseSettlement::Fulfill(value::encode_undefined()),
                );
            } else {
                let err = type_error_exception(caller, "writer is not attached to a stream");
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(err));
            }

            Some(promise)
        }
        WritableStreamDefaultWriterMethodKind::ReleaseLock => {
            let stream_handle = {
                let table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };
            if let Some(sh) = stream_handle {
                let mut table = caller
                    .data()
                    .writable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(sh as usize) {
                    entry.locked = false;
                }
            }
            Some(value::encode_undefined())
        }
        WritableStreamDefaultWriterMethodKind::GetClosed => {
            // 返回 closed promise
            let table = caller
                .data()
                .writer_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle as usize).and_then(|e| e.closed_promise)
        }
        WritableStreamDefaultWriterMethodKind::GetReady => {
            // 返回 ready promise
            let table = caller
                .data()
                .writer_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle as usize).and_then(|e| e.ready_promise)
        }
        WritableStreamDefaultWriterMethodKind::GetDesiredSize => {
            // 返回 controller.desiredSize 值
            let stream_handle = {
                let table = caller
                    .data()
                    .writer_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.writable_stream_handle)
            };

            if let Some(sh) = stream_handle {
                let ctrl_handle = {
                    let table = caller
                        .data()
                        .writable_stream_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    table.get(sh as usize).and_then(|e| e.controller_handle)
                };

                if let Some(ch) = ctrl_handle {
                    let ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
            let error_val = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);

            // 获取关联的 stream handle
            let stream_handle = {
                let table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.stream_handle)
            };

            if let Some(sh) = stream_handle {
                // 设置 WritableStream 为 Errored
                {
                    let mut table = caller
                        .data()
                        .writable_stream_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(entry) = table.get_mut(sh as usize) {
                        entry.state = WritableStreamState::Errored;
                        entry.error = Some(error_val);
                    }
                }

                // reject writer 的 closed 和 ready promise
                {
                    let writer_table = caller
                        .data()
                        .writer_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for writer_entry in writer_table.iter() {
                        if writer_entry.writable_stream_handle == sh {
                            if let Some(cp) = writer_entry.closed_promise {
                                settle_promise(
                                    caller.data(),
                                    cp,
                                    PromiseSettlement::Reject(error_val),
                                );
                            }
                            if let Some(rp) = writer_entry.ready_promise {
                                settle_promise(
                                    caller.data(),
                                    rp,
                                    PromiseSettlement::Reject(error_val),
                                );
                            }
                        }
                    }
                }
            }

            Some(value::encode_undefined())
        }
        WritableStreamDefaultControllerMethodKind::GetSignal => {
            let stream_handle = {
                let table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| e.stream_handle)
            };
            let signal_obj = stream_handle.and_then(|sh| {
                let table = caller
                    .data()
                    .writable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(sh as usize).and_then(|entry| entry.abort_signal)
            });
            Some(signal_obj.unwrap_or_else(value::encode_undefined))
        }
    }
}
