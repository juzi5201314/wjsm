//! Fetch-backed ReadableStream body bridge.
//!
//! 拥有 reqwest Response → ReadableStream 之间的 pull/read/materialize 逻辑。
//! 唯一的 HTTP body owner 仍然是 `http_response_table`；本模块仅通过 take/spawn/
//! materialize-put-back 与表交互，不在 `.await` 期间持有任何 runtime 表锁或
//! `MutexGuard`。每个 spawn 出去的 task 都携带 `AsyncOpGuard`，保证 post-main
//! scheduler 在 in-flight chunk pull 期间不会提前退出。
use crate::scheduler::{AsyncHostCompletion, AsyncOpGuard};
use crate::*;
use wasmtime::Caller;

use super::fetch_core::alloc_type_error_from_caller;
use super::streams_readable::{
    build_reader_result, build_reader_result_with_env, create_uint8array_with_env,
    write_u8_bytes_to_view,
};

pub(crate) fn call_fetch_body_reader_read(
    caller: &mut Caller<'_, RuntimeState>,
    reader_handle: u32,
    http_handle: u32,
    byob_view: Option<i64>,
) -> Option<i64> {
    enum ReadDecision {
        Missing,
        Eof,
        Error(String),
        SharePending(i64),
        ByobConflict,
        ServeBuffer(Vec<u8>),
        Spawn(reqwest::Response),
        SpawnEof,
    }

    let decision = {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        match table.get_mut(http_handle as usize) {
            None => ReadDecision::Missing,
            Some(entry) if entry.eof && entry.pending_bytes.is_empty() => ReadDecision::Eof,
            Some(entry) if entry.error.is_some() && entry.pending_read_promise.is_none() => {
                ReadDecision::Error(entry.error.clone().unwrap())
            }
            Some(entry) if entry.pending_read_promise.is_some() => {
                if byob_view.is_some() {
                    ReadDecision::ByobConflict
                } else {
                    ReadDecision::SharePending(entry.pending_read_promise.unwrap())
                }
            }
            Some(entry) if !entry.pending_bytes.is_empty() => {
                ReadDecision::ServeBuffer(entry.pending_bytes.front().cloned().unwrap_or_default())
            }
            Some(entry) if entry.response.is_none() => {
                entry.eof = true;
                ReadDecision::SpawnEof
            }
            Some(entry) => ReadDecision::Spawn(entry.response.take().unwrap()),
        }
    };

    match decision {
        ReadDecision::Missing | ReadDecision::Eof | ReadDecision::SpawnEof => {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let result = build_reader_result(caller, true, None);
            settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(result));
            Some(p)
        }
        ReadDecision::Error(msg) => {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let err = alloc_type_error_from_caller(caller, &msg);
            settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(err));
            Some(p)
        }
        ReadDecision::SharePending(existing) => Some(existing),
        ReadDecision::ByobConflict => {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let err = alloc_type_error_from_caller(
                caller,
                "BYOB reader already has a pending read",
            );
            settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(err));
            Some(p)
        }
        ReadDecision::ServeBuffer(front_chunk) => {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            Some(fulfill_read_from_buffer(
                caller,
                http_handle,
                reader_handle,
                byob_view,
                &front_chunk,
                p,
            ))
        }
        ReadDecision::Spawn(response) => {
            spawn_chunk_pull(caller, reader_handle, http_handle, byob_view, response)
        }
    }
}

fn spawn_chunk_pull(
    caller: &mut Caller<'_, RuntimeState>,
    reader_handle: u32,
    http_handle: u32,
    byob_view: Option<i64>,
    mut response: reqwest::Response,
) -> Option<i64> {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        if let Some(entry) = table.get_mut(http_handle as usize) {
            entry.pending_read_promise = Some(promise);
        }
    }
    {
        let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
        if let Some(reader) = reader_table.get_mut(reader_handle as usize) {
            reader.pending_read_promise = Some(promise);
            reader.pending_byob_view = byob_view;
        }
    }
    let tx = match caller.data().host_completion_tx.clone() {
        Some(t) => t,
        None => {
            let err = alloc_type_error_from_caller(caller, "fetch runtime unavailable");
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
            return Some(promise);
        }
    };
    let guard = caller.data().async_op_counter.as_ref().map(|c| c.begin());
    let promise_clone = promise;
    let mut response_opt = Some(response);
    tokio::spawn(async move {
        let _guard: Option<AsyncOpGuard> = guard;
        let mut response = response_opt.take().unwrap();
        let outcome = response.chunk().await;
        let _ = tx.send(AsyncHostCompletion::Materialize {
            promise: promise_clone,
            materialize: Box::new(move |store, env| match outcome {
                Ok(Some(chunk)) => {
                    let settlement = materialize_chunk_into_entry(
                        store,
                        env,
                        http_handle,
                        reader_handle,
                        byob_view,
                        chunk.to_vec(),
                    );
                    // Put response back so the next read can pull another chunk.
                    let mut table = store
                        .data()
                        .http_response_table
                        .lock()
                        .expect("http_response mutex");
                    if let Some(entry) = table.get_mut(http_handle as usize) {
                        if entry.response.is_none() && !entry.eof && entry.error.is_none() {
                            entry.response = Some(response);
                        }
                    }
                    settlement
                }
                Ok(None) => {
                    {
                        let mut table = store
                            .data()
                            .http_response_table
                            .lock()
                            .expect("http_response mutex");
                        if let Some(entry) = table.get_mut(http_handle as usize) {
                            entry.response = None;
                            entry.pending_read_promise = None;
                            entry.eof = true;
                        }
                    }
                    clear_reader_pending(store, reader_handle);
                    let result = build_reader_result_with_env(store, env, true, None);
                    PromiseSettlement::Fulfill(result)
                }
                Err(e) => {
                    {
                        let mut table = store
                            .data()
                            .http_response_table
                            .lock()
                            .expect("http_response mutex");
                        if let Some(entry) = table.get_mut(http_handle as usize) {
                            entry.response = None;
                            entry.pending_read_promise = None;
                            entry.error = Some(e.to_string());
                        }
                    }
                    clear_reader_pending(store, reader_handle);
                    let err = crate::runtime_heap::alloc_type_error_with_env(
                        store,
                        env,
                        e.to_string(),
                    );
                    PromiseSettlement::Reject(err)
                }
            }),
        });
    });
    Some(promise)
}

pub(crate) fn consume_fetch_body_to_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    http_handle: u32,
    promise: i64,
    kind: ResponseMethodKind,
) -> bool {
    enum ConsumeDecision {
        Spawn(reqwest::Response),
        Reject(&'static str),
    }

    let decision = {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        match table.get_mut(http_handle as usize) {
            Some(entry) => {
                if entry.pending_read_promise.is_some() {
                    ConsumeDecision::Reject("body stream is already being read")
                } else {
                    entry.pending_read_promise = Some(promise);
                    match entry.response.take() {
                        Some(r) => ConsumeDecision::Spawn(r),
                        None => ConsumeDecision::Reject("body stream already read"),
                    }
                }
            }
            None => ConsumeDecision::Reject("HTTP response not available"),
        }
    };

    let mut response = match decision {
        ConsumeDecision::Spawn(r) => r,
        ConsumeDecision::Reject(msg) => {
            let err = alloc_type_error_from_caller(caller, msg);
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
            return true;
        }
    };

    let tx = match caller.data().host_completion_tx.clone() {
        Some(t) => t,
        None => {
            let err = alloc_type_error_from_caller(caller, "fetch runtime unavailable");
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
            return true;
        }
    };
    let guard = caller.data().async_op_counter.as_ref().map(|c| c.begin());
    let promise_clone = promise;
    tokio::spawn(async move {
        let _guard: Option<AsyncOpGuard> = guard;
        let outcome = response.bytes().await;
        let _ = tx.send(AsyncHostCompletion::Materialize {
            promise: promise_clone,
            materialize: Box::new(move |store, env| {
                {
                    let mut table = store
                        .data()
                        .http_response_table
                        .lock()
                        .expect("http_response mutex");
                    if let Some(entry) = table.get_mut(http_handle as usize) {
                        entry.response = None;
                        entry.pending_read_promise = None;
                        entry.pending_bytes.clear();
                        entry.eof = true;
                    }
                }
                match outcome {
                    Ok(bytes) => match kind {
                        ResponseMethodKind::Text => {
                            let text = String::from_utf8_lossy(&bytes).to_string();
                            let handle = crate::runtime_render::store_runtime_string_in_state(
                                store.data(),
                                text,
                            );
                            PromiseSettlement::Fulfill(handle)
                        }
                        ResponseMethodKind::Json => {
                            let text = String::from_utf8_lossy(&bytes).to_string();
                            let mut parser = crate::runtime_json::JsonParser::new(text.as_bytes());
                            match parser.parse_value() {
                                Ok(json_value) => {
                                    let wasm_value =
                                        crate::runtime_json::build_wasm_value_with_env(
                                            store,
                                            env,
                                            &json_value,
                                        );
                                    PromiseSettlement::Fulfill(wasm_value)
                                }
                                Err(e) => {
                                    let err = crate::runtime_heap::alloc_type_error_with_env(
                                        store, env, e,
                                    );
                                    PromiseSettlement::Reject(err)
                                }
                            }
                        }
                        ResponseMethodKind::ArrayBuffer => {
                            let ab_handle = {
                                let mut ab_table = store
                                    .data()
                                    .arraybuffer_table
                                    .lock()
                                    .expect("arraybuffer mutex");
                                let ab_handle = ab_table.len() as u32;
                                ab_table.push(ArrayBufferEntry {
                                    data: bytes.to_vec(),
                                });
                                ab_handle
                            };
                            let ab_obj = crate::runtime_heap::alloc_host_object(store, env, 2);
                            let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
                                store,
                                env,
                                ab_obj,
                                "__arraybuffer_handle__",
                                wjsm_ir::value::encode_f64(ab_handle as f64),
                            );
                            let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
                                store,
                                env,
                                ab_obj,
                                "byteLength",
                                wjsm_ir::value::encode_f64(bytes.len() as f64),
                            );
                            PromiseSettlement::Fulfill(ab_obj)
                        }
                        ResponseMethodKind::Clone => {
                            let err = crate::runtime_heap::alloc_type_error_with_env(
                                store,
                                env,
                                "clone cannot consume body".to_string(),
                            );
                            PromiseSettlement::Reject(err)
                        }
                    },
                    Err(e) => {
                        let err = crate::runtime_heap::alloc_type_error_with_env(
                            store,
                            env,
                            e.to_string(),
                        );
                        PromiseSettlement::Reject(err)
                    }
                }
            }),
        });
    });
    true
}

fn clear_reader_pending<C: wasmtime::AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    reader_handle: u32,
) {
    let mut store = ctx.as_context_mut();
    let mut reader_table = store.data().reader_table.lock().expect("reader mutex");
    if let Some(reader) = reader_table.get_mut(reader_handle as usize) {
        reader.pending_read_promise = None;
        reader.pending_byob_view = None;
    }
}

fn materialize_chunk_into_entry(
    store: &mut wasmtime::Store<RuntimeState>,
    env: &WasmEnv,
    http_handle: u32,
    reader_handle: u32,
    byob_view: Option<i64>,
    chunk: Vec<u8>,
) -> PromiseSettlement {
    let (value, written) = if let Some(view) = byob_view {
        let written = write_u8_bytes_to_view_store(store, view, &chunk).unwrap_or(0);
        (view, written)
    } else {
        let arr = create_uint8array_with_env(store, env, &chunk);
        (arr, chunk.len())
    };
    let overflow = if written < chunk.len() {
        Some(chunk[written..].to_vec())
    } else {
        None
    };
    {
        let mut table = store
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        if let Some(entry) = table.get_mut(http_handle as usize) {
            entry.pending_read_promise = None;
            if let Some(overflow_bytes) = overflow {
                entry.pending_bytes.push_back(overflow_bytes);
            }
        }
    }
    clear_reader_pending(store, reader_handle);
    let result = build_reader_result_with_env(store, env, false, Some(value));
    PromiseSettlement::Fulfill(result)
}

fn write_u8_bytes_to_view_store(
    store: &mut wasmtime::Store<RuntimeState>,
    view: i64,
    bytes: &[u8],
) -> Option<usize> {
    if !value::is_object(view) {
        return None;
    }
    let handle = value::decode_handle(view) as usize;
    let entry = {
        let table = store.data().typedarray_table.lock().expect("typedarray mutex");
        table.get(handle).cloned()?
    };
    if entry.element_size != 1 {
        return None;
    }
    let write_len = (entry.length as usize).min(bytes.len());
    let start = entry.byte_offset as usize;
    let mut ab_table = store.data().arraybuffer_table.lock().expect("arraybuffer mutex");
    let buffer = ab_table.get_mut(entry.buffer_handle as usize)?;
    let end = start.checked_add(write_len)?;
    buffer.data.get_mut(start..end)?.copy_from_slice(&bytes[..write_len]);
    Some(write_len)
}

fn fulfill_read_from_buffer(
    caller: &mut Caller<'_, RuntimeState>,
    http_handle: u32,
    reader_handle: u32,
    byob_view: Option<i64>,
    front_chunk: &[u8],
    promise: i64,
) -> i64 {
    let (value, written) = if let Some(view) = byob_view {
        let written = write_u8_bytes_to_view(caller, view, front_chunk).unwrap_or(0);
        (view, written)
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        let arr = create_uint8array_with_env(caller, &env, front_chunk);
        (arr, front_chunk.len())
    };
    let result = build_reader_result(caller, false, Some(value));
    settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(result));

    {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        if let Some(entry) = table.get_mut(http_handle as usize) {
            if written < front_chunk.len() {
                // front_chunk 是 entry.pending_bytes.front() 的拷贝。pop 掉旧的，
                // 再把剩余字节重新压回 front。
                entry.pending_bytes.pop_front();
                entry
                    .pending_bytes
                    .push_front(front_chunk[written..].to_vec());
            } else {
                entry.pending_bytes.pop_front();
            }
        }
    }
    {
        let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
        if let Some(reader) = reader_table.get_mut(reader_handle as usize) {
            reader.pending_read_promise = None;
            reader.pending_byob_view = None;
        }
    }
    promise
}
