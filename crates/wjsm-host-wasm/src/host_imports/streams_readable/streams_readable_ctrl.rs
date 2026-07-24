use super::*;
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
            .map(|raw| get_string_utf8_lossy(caller, raw) == "bytes")
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
    let controller_handle = caller
        .data()
        .stream_controller_table
        .alloc(StreamControllerEntry {
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
            underlying_source: None,
            pull_callback: None,
            cancel_callback: None,
            write_callback: None,
            sink_close_callback: None,
            active_byob_request: None,
        });

    let stream_handle = caller
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
            controller_handle: Some(controller_handle),
            is_byte_stream,
            pipe_to: None,
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

    // 6. 如果 source.start 存在：创建 controller JS 对象，调用 start(controller)
    let controller_obj = create_controller_object(caller, controller_handle);

    if value::is_object(source) {
        let source_ptr = resolve_handle(caller, source);
        if let Some(ptr) = source_ptr {
            // 捕获 pull / cancel callbacks
            let pull_fn = read_object_property_by_name(caller, ptr, "pull")
                .unwrap_or_else(value::encode_undefined);
            let cancel_fn = read_object_property_by_name(caller, ptr, "cancel")
                .unwrap_or_else(value::encode_undefined);
            {
                let mut table = caller
                    .data()
                    .stream_controller_table
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ctrl) = table.get_mut(controller_handle as usize) {
                    ctrl.underlying_source = Some(source);
                    if value::is_callable(pull_fn) {
                        ctrl.pull_callback = Some(pull_fn);
                    }
                    if value::is_callable(cancel_fn) {
                        ctrl.cancel_callback = Some(cancel_fn);
                    }
                }
            }

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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.started = true;
        }
    }

    let obj = create_readable_stream_js_object(caller, stream_handle);
    let obj_handle = weak_target_handle_index_of(caller, obj).unwrap_or(0);
    caller
        .data()
        .readable_stream_table
        .bind_obj_handle(obj_handle, stream_handle);

    Some(obj)
}

// ── Controller 方法实现 ──────────────────────────────────────────────────────

/// controller.enqueue(chunk)
pub(crate) fn controller_enqueue(
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let mut reader_table = caller
            .data()
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending_info: Option<(ReaderKind, Option<i64>, i64)> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle
                && let Some(promise) = reader.pending_read_promise.take()
            {
                pending_info = Some((reader.kind, reader.pending_byob_view.take(), promise));
                break;
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ctrl) = table.get_mut(controller_handle as usize) {
            ctrl.chunk_queue.push_back(chunk);
        }
    }

    pump_readable_stream_pipe_to(caller, stream_handle);

    Some(value::encode_undefined())
}

/// controller.close()
pub(crate) fn controller_close(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
) -> Option<i64> {
    let (already_closed, stream_handle) = {
        let mut table = caller
            .data()
            .stream_controller_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(stream_handle as usize) {
            entry.state = StreamState::Closed;
        }
    }

    // 检查 pending_read_promise → resolve {done: true, value: undefined}
    let pending = {
        let mut reader_table = caller
            .data()
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending_info: Option<(Option<i64>, i64)> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle
                && let Some(promise) = reader.pending_read_promise.take()
            {
                pending_info = Some((reader.pending_byob_view.take(), promise));
                break;
            }
        }
        pending_info
    };

    if let Some((byob_view, promise)) = pending {
        let result = build_reader_result(caller, true, byob_view);
        settle_promise(caller.data(), promise, PromiseSettlement::Fulfill(result));
    }

    pump_readable_stream_pipe_to(caller, stream_handle);

    Some(value::encode_undefined())
}

/// controller.error(e)
pub(crate) fn controller_error(
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
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctrl = table.get(controller_handle as usize)?;
        ctrl.stream_handle
    };

    // 设置 stream state = Errored
    {
        let mut table = caller
            .data()
            .readable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(stream_handle as usize) {
            entry.state = StreamState::Errored;
            // 尝试存储错误消息
            if value::is_string(error_val) {
                entry.error = Some("stream error".to_string());
            }
        }
    }

    // 检查 pending_read_promise → reject
    let pending = {
        let mut reader_table = caller
            .data()
            .reader_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending_promise: Option<i64> = None;
        for reader in reader_table.iter_mut() {
            if reader.stream_handle == stream_handle
                && let Some(promise) = reader.pending_read_promise.take()
            {
                pending_promise = Some(promise);
                break;
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
