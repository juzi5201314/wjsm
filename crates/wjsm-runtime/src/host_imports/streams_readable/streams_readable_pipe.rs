use super::*;
pub(super) fn readable_stream_pipe_to(
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get(readable_handle as usize)?;
        (entry.controller_handle, entry.state.clone())
    };
    let chunks = if let Some(ctrl_handle) = controller_handle {
        let mut table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

pub(super) fn readable_stream_pipe_through(
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
                let reader_table = caller
                    .data()
                    .reader_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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

            // 2. 获取 controller handle、stream 状态和 response_body 信息
            let (controller_handle, http_response_handle, stream_state, response_body) = {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let entry = stream_table.get_mut(stream_handle as usize)?;
                // read() 触发 disturbed → bodyUsed = true（WHATWG Fetch §6.4.4）
                entry.disturbed = true;
                (
                    entry.controller_handle,
                    entry.http_response_handle,
                    entry.state.clone(),
                    (entry.response_body_handle, entry.response_body_object),
                )
            };
            // 标记 Response.bodyUsed = true（流被实际读取）
            mark_response_body_used_from_caller(caller, response_body.0, response_body.1);

            // 3. 自定义流路径：检查 controller chunk_queue
            if let Some(ctrl_handle) = controller_handle {
                let chunk = {
                    let mut ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                    let mut reader_table = caller
                        .data()
                        .reader_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(reader) = reader_table.get_mut(handle as usize) {
                        reader.pending_read_promise = Some(p);
                        reader.pending_byob_view = byob_view;
                    }
                }

                // BYOB path：create ByobRequestEntry, set controller.active_byob_request
                if reader_kind == ReaderKind::Byob
                    && let Some(view) = byob_view
                {
                    let byob_handle = caller.data().byob_request_table.alloc(ByobRequestEntry {
                        controller_handle: ctrl_handle,
                        reader_handle: handle,
                        view,
                        promise: p,
                        responded: false,
                    });
                    let mut ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(ctrl) = ctrl_table.get_mut(ctrl_handle as usize) {
                        ctrl.active_byob_request = Some(byob_handle);
                    }
                }

                // Schedule pull microtask if pull_callback is set
                let pull_info = {
                    let ctrl_table = caller
                        .data()
                        .stream_controller_table
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    ctrl_table
                        .get(ctrl_handle as usize)
                        .and_then(|ctrl| ctrl.pull_callback.map(|cb| (cb, ctrl.underlying_source)))
                };
                if let Some((pull_callback, this_val)) = pull_info {
                    let controller_obj = create_controller_object(caller, ctrl_handle);
                    let mut queue = caller
                        .data()
                        .microtask_queue
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    queue.push_back(Microtask::ReadableStreamPull {
                        callback: pull_callback,
                        this_val: this_val.unwrap_or_else(value::encode_undefined),
                        controller: controller_obj,
                    });
                }

                return Some(p);
            }

            // 4. HTTP 路径：检查 http_response_handle
            if let Some(http_handle) = http_response_handle {
                // 转发到 fetch-backed body bridge。
                return super::super::streams_fetch_body::call_fetch_body_reader_read(
                    caller,
                    handle,
                    http_handle,
                    byob_view,
                );
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
                let reader_table = caller
                    .data()
                    .reader_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                reader_table.get(handle as usize)?.stream_handle
            };
            let mut stream_table = caller
                .data()
                .readable_stream_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = stream_table.get_mut(stream_handle as usize) {
                entry.locked = false;
            }
            Some(value::encode_undefined())
        }
        ReadableStreamDefaultReaderMethodKind::GetClosed => {
            // 返回 closed_promise
            let reader_table = caller
                .data()
                .reader_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let reader = reader_table.get(handle as usize)?;
            let promise = reader
                .closed_promise
                .unwrap_or_else(value::encode_undefined);
            Some(promise)
        }
    }
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
pub(crate) fn create_uint8array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: &[u8],
) -> i64 {
    let ab_handle = {
        let mut store = ctx.as_context_mut();
        let mut ab_table = store
            .data_mut()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = ab_table.len() as u32;
        ab_table.push(ArrayBufferEntry {
            data: bytes.to_vec(),
        });
        handle
    };
    let ta_handle = {
        let mut store = ctx.as_context_mut();
        let mut ta_table = store
            .data_mut()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ctrl) = table.get(handle as usize) {
                let desired = ctrl.high_water_mark - ctrl.chunk_queue.len() as f64;
                Some(value::encode_f64(desired))
            } else {
                Some(value::encode_null())
            }
        }
        ReadableStreamDefaultControllerMethodKind::GetByobRequest => {
            Some(controller_get_byob_request(caller, handle))
        }
    }
}

fn controller_get_byob_request(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
) -> i64 {
    // 1. 读取 controller.active_byob_request 与 controller_handle 校验
    let active = {
        let table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(controller_handle as usize)
            .and_then(|c| c.active_byob_request)
    };
    let Some(byob_handle) = active else {
        return value::encode_null();
    };

    // 2. 如果已 respond，则返回 null
    let (view, responded) = {
        let table = caller
            .data()
            .byob_request_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.get(byob_handle as usize) else {
            return value::encode_null();
        };
        (entry.view, entry.responded)
    };
    if responded {
        return value::encode_null();
    }

    // 3. 构造 { view, respond(n) } JS 对象
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__byob_request_handle__",
        value::encode_f64(byob_handle as f64),
    );
    let _ = define_host_data_property_from_caller(caller, obj, "view", view);

    let respond_callable = NativeCallable::ReadableStreamByobRequestMethod {
        handle: byob_handle,
        kind: ReadableStreamByobRequestMethodKind::Respond,
    };
    let respond_idx = push_native_callable(caller, respond_callable);
    let respond_val = value::encode_native_callable_idx(respond_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "respond", respond_val);

    if let Some(obj_handle) = weak_target_handle_index_of(caller, obj) {
        caller
            .data()
            .byob_request_table
            .bind_obj_handle(obj_handle, byob_handle);
    }

    obj
}

fn typedarray_entry_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    view: i64,
) -> Option<TypedArrayEntry> {
    if !value::is_object(view) {
        return None;
    }
    let ptr = resolve_handle(caller, view)?;
    let handle_raw = read_object_property_by_name(caller, ptr, "__typedarray_handle__")?;
    let handle = value::decode_f64(handle_raw) as usize;
    let table = caller
        .data()
        .typedarray_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table.get(handle).cloned()
}

fn typedarray_length_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    view: i64,
) -> Option<usize> {
    typedarray_entry_from_object(caller, view).map(|e| e.length as usize)
}

pub(crate) fn call_byob_request_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    handle: u32,
    kind: ReadableStreamByobRequestMethodKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        ReadableStreamByobRequestMethodKind::GetView => {
            // 直接返回 view（data property 已经设置；此分支作为防御保留）
            let table = caller
                .data()
                .byob_request_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = table.get(handle as usize)?;
            if entry.responded {
                Some(value::encode_null())
            } else {
                Some(entry.view)
            }
        }
        ReadableStreamByobRequestMethodKind::Respond => {
            let bytes_written_arg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if !value::is_f64(bytes_written_arg) {
                return Some(type_error_exception(
                    caller,
                    "respond(bytesWritten) requires a number",
                ));
            }
            let bytes_written_f = value::decode_f64(bytes_written_arg);
            if !bytes_written_f.is_finite()
                || bytes_written_f.fract() != 0.0
                || bytes_written_f < 0.0
            {
                return Some(type_error_exception(
                    caller,
                    "bytesWritten must be a non-negative integer",
                ));
            }
            let bytes_written = bytes_written_f as usize;

            // 取出 entry 信息（释放 byob_request_table 锁后再调用需要 &mut caller 的 helper）
            let entry_info = {
                let table = caller
                    .data()
                    .byob_request_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(handle as usize).map(|e| {
                    (
                        e.controller_handle,
                        e.reader_handle,
                        e.view,
                        e.promise,
                        e.responded,
                    )
                })
            };
            let Some((controller_handle, reader_handle, view, promise, already_responded)) =
                entry_info
            else {
                return Some(type_error_exception(caller, "invalid BYOB request"));
            };
            let view_len = typedarray_length_from_object(caller, view).unwrap_or(0);

            if already_responded {
                return Some(type_error_exception(
                    caller,
                    "BYOB request already responded",
                ));
            }
            if bytes_written > view_len {
                return Some(type_error_exception(
                    caller,
                    "bytesWritten exceeds view.byteLength",
                ));
            }

            // 构造转移后的结果 view：result.value.byteLength === bytesWritten，原 view detached。
            let result_view = {
                let env = WasmEnv::from_caller(caller).expect("WasmEnv");
                transfer_byob_view_with_env(caller, &env, view, bytes_written).unwrap_or(view)
            };

            // 标记 responded + 清理 controller 上的 active_byob_request
            {
                let mut table = caller
                    .data()
                    .byob_request_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.responded = true;
                }
            }
            {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ctrl) = table.get_mut(controller_handle as usize)
                    && ctrl.active_byob_request == Some(handle)
                {
                    ctrl.active_byob_request = None;
                }
            }
            // 清理 reader 的 pending 状态
            {
                let mut reader_table = caller
                    .data()
                    .reader_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(reader) = reader_table.get_mut(reader_handle as usize) {
                    reader.pending_read_promise = None;
                    reader.pending_byob_view = None;
                }
            }
            // fulfill 结果 { done: false, value: resultView }
            let result = build_reader_result(caller, false, Some(result_view));
            settle_promise(
                caller.data_mut(),
                promise,
                PromiseSettlement::Fulfill(result),
            );
            Some(value::encode_undefined())
        }
    }
}
