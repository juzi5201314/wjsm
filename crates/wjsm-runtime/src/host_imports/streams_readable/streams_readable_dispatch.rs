use super::*;
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
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let (locked, is_byte_stream) = {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let entry = stream_table.get_mut(handle as usize)?;
                let locked = entry.locked;
                let is_byte_stream = entry.is_byte_stream;
                if !locked && (!wants_byob || is_byte_stream) {
                    entry.locked = true;
                }
                (locked, is_byte_stream)
            };
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
            let reader_handle = caller.data().reader_table.alloc(ReaderEntry {
                stream_handle: handle,
                kind: reader_kind,
                pending_read_promise: None,
                pending_byob_view: None,
                closed_promise: Some(closed_promise),
            });

            // 如果流已关闭，立即 resolve closed promise
            let stream_state = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
            if let Some(obj_handle) =
                crate::runtime_values::weak_target_handle_index_of(caller, obj)
            {
                caller
                    .data()
                    .reader_table
                    .bind_obj_handle(obj_handle, reader_handle);
            }

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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let entry = stream_table.get_mut(handle as usize)?;
                entry.state = StreamState::Closed;
                entry.controller_handle
            };

            // 清空 controller 队列
            if let Some(ctrl_handle) = controller_handle {
                let mut ctrl_table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let ctrl = ctrl_table.get(ctrl_handle? as usize)?;
                (
                    ctrl.chunk_queue.clone(),
                    ctrl.high_water_mark,
                    ctrl.strategy_size,
                )
            };

            // 4. 创建两个新的 StreamControllerEntry，各自持有 chunk_queue 的副本
            let controller1_handle =
                caller
                    .data()
                    .stream_controller_table
                    .alloc(StreamControllerEntry {
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
                        underlying_source: None,
                        pull_callback: None,
                        cancel_callback: None,
                        write_callback: None,
                        sink_close_callback: None,
                        active_byob_request: None,
                    });

            let controller2_handle =
                caller
                    .data()
                    .stream_controller_table
                    .alloc(StreamControllerEntry {
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
                        underlying_source: None,
                        pull_callback: None,
                        cancel_callback: None,
                        write_callback: None,
                        sink_close_callback: None,
                        active_byob_request: None,
                    });

            let stream1_handle = caller
                .data()
                .readable_stream_table
                .alloc(ReadableStreamEntry {
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

            let stream2_handle = caller
                .data()
                .readable_stream_table
                .alloc(ReadableStreamEntry {
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

            // 6. 回写 stream_handle 到各自的 controller
            {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ctrl) = table.get_mut(controller1_handle as usize) {
                    ctrl.stream_handle = stream1_handle;
                }
                if let Some(ctrl) = table.get_mut(controller2_handle as usize) {
                    ctrl.stream_handle = stream2_handle;
                }
            }

            let stream1_obj = create_readable_stream_js_object(caller, stream1_handle);
            let stream1_obj_handle = weak_target_handle_index_of(caller, stream1_obj).unwrap_or(0);
            caller
                .data()
                .readable_stream_table
                .bind_obj_handle(stream1_obj_handle, stream1_handle);
            let stream2_obj = create_readable_stream_js_object(caller, stream2_handle);
            let stream2_obj_handle = weak_target_handle_index_of(caller, stream2_obj).unwrap_or(0);
            caller
                .data()
                .readable_stream_table
                .bind_obj_handle(stream2_obj_handle, stream2_handle);

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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.locked = true;
                }
            }

            // 创建 closed_promise 和 ReaderEntry（与 GetReader 相同的模式）
            let closed_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let reader_handle = caller.data().reader_table.alloc(ReaderEntry {
                stream_handle: handle,
                kind: ReaderKind::Default,
                pending_read_promise: None,
                pending_byob_view: None,
                closed_promise: Some(closed_promise),
            });

            // 如果流已关闭，立即 resolve closed promise
            let stream_state = {
                let table = caller
                    .data()
                    .readable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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

pub(super) fn writable_stream_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    writable: i64,
) -> Option<u32> {
    resolve_handle(caller, writable)
        .and_then(|ptr| read_object_property_by_name(caller, ptr, "__writable_stream_handle__"))
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw) as u32)
}

pub(super) fn transform_parts_from_object(
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(handle)?;
        return Some((entry.readable_obj?, entry.writable_obj?));
    }
    let readable = read_object_property_by_name(caller, ptr, "readable")?;
    let writable = read_object_property_by_name(caller, ptr, "writable")?;
    Some((readable, writable))
}
