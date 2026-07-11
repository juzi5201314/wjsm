//! MessageChannel + MessagePort host 方法。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use wasmtime::{Caller, Store};

use crate::runtime_process::ProcessNextTickTask;
use crate::runtime_worker_message::{deserialize_value, serialize_for_post_message};
use crate::scheduler::AsyncHostCompletion;
use crate::*;

use super::cluster::{LocalPortBinding, PortEndpoint, cluster_of};

/// MessageChannel 创建：返回 `{ port1, port2 }`（全局 port id）。
pub(super) fn create_message_channel(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let Some(cluster) = cluster_of(caller) else {
        return make_type_error_exception(caller, "worker_threads cluster is not available");
    };
    let (a, b) = cluster.alloc_port_pair();
    ensure_local_port(caller, a.id);
    ensure_local_port(caller, b.id);
    host_object_two_ids(caller, "port1", a.id, "port2", b.id)
}

pub(super) fn host_object_two_ids(
    caller: &mut Caller<'_, RuntimeState>,
    k1: &str,
    id1: u32,
    k2: &str,
    id2: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(caller, obj, k1, value::encode_f64(id1 as f64));
    let _ = define_host_data_property_from_caller(caller, obj, k2, value::encode_f64(id2 as f64));
    obj
}

pub(super) fn ensure_local_port(caller: &mut Caller<'_, RuntimeState>, global_id: u32) {
    let mut map = caller
        .data()
        .message_port_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.entry(global_id).or_insert(LocalPortBinding {
        global_id,
        deliver_cb: None,
        started: false,
        ref_guard: None,
    });
}

pub(super) fn port_id_arg(args: &[i64], index: usize) -> Option<u32> {
    let raw = args.get(index).copied()?;
    if value::is_f64(raw) {
        Some(value::decode_f64(raw) as u32)
    } else {
        None
    }
}

pub(super) fn port_post_message(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return make_type_error_exception(caller, "portPostMessage: invalid port id");
    };
    let value_raw = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let transfer = args
        .get(2)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let payload = match serialize_for_post_message(caller, value_raw, transfer) {
        Ok(v) => v,
        Err(msg) => return make_type_error_exception(caller, &msg),
    };
    let Some(cluster) = cluster_of(caller) else {
        return value::encode_undefined();
    };
    let Some(port) = cluster.port(port_id) else {
        return value::encode_undefined();
    };
    if port.closed.load(Ordering::SeqCst) {
        return value::encode_undefined();
    }
    let Some(peer) = cluster.port(port.peer_id) else {
        return value::encode_undefined();
    };
    if peer.closed.load(Ordering::SeqCst) {
        return value::encode_undefined();
    }
    peer.inbox
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(payload);
    wake_port_delivery(&peer, port.peer_id);
    value::encode_undefined()
}

fn wake_port_delivery(port: &PortEndpoint, port_id: u32) {
    let tx = port
        .wake_tx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(tx) = tx else {
        return;
    };
    let _ = tx.send(AsyncHostCompletion::HostTask {
        run: Box::new(move |store, env| {
            drain_port_to_next_tick(store, env, port_id);
        }),
    });
}

fn drain_port_to_next_tick(store: &mut Store<RuntimeState>, env: &WasmEnv, port_id: u32) {
    let (started, deliver_cb) = {
        let map = store
            .data()
            .message_port_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match map.get(&port_id) {
            Some(b) if b.started => (true, b.deliver_cb),
            _ => (false, None),
        }
    };
    if !started {
        return;
    }
    let Some(cb) = deliver_cb else {
        return;
    };
    let Some(cluster) = store
        .data()
        .shared_state
        .as_ref()
        .map(|s| Arc::clone(&s.worker_cluster))
    else {
        return;
    };
    let Some(port) = cluster.port(port_id) else {
        return;
    };
    let messages: Vec<_> = port
        .inbox
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    for msg in messages {
        let js_val = deserialize_value(store, env, &msg);
        store
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![js_val],
            });
    }
}

pub(super) fn port_start(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return make_type_error_exception(caller, "portStart: invalid port id");
    };
    let deliver = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    ensure_local_port(caller, port_id);
    {
        let mut map = caller
            .data()
            .message_port_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(binding) = map.get_mut(&port_id) {
            binding.deliver_cb = Some(deliver);
            binding.started = true;
            if binding.ref_guard.is_none()
                && let Some(counter) = caller.data().async_op_counter.clone() {
                    binding.ref_guard = Some(counter.begin());
                }
        }
    }
    if let Some(cluster) = cluster_of(caller)
        && let Some(port) = cluster.port(port_id)
    {
        *port.wake_tx.lock().unwrap_or_else(|e| e.into_inner()) =
            caller.data().host_completion_tx.clone();
        // 同步排空已有 inbox（同线程 MessageChannel 场景）
        drain_port_inbox_sync(caller, port_id, deliver);
    }
    value::encode_undefined()
}

fn drain_port_inbox_sync(caller: &mut Caller<'_, RuntimeState>, port_id: u32, deliver: i64) {
    let Some(cluster) = cluster_of(caller) else {
        return;
    };
    let Some(port) = cluster.port(port_id) else {
        return;
    };
    let messages: Vec<_> = port
        .inbox
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    for msg in messages {
        let js_val = deserialize_value(caller, &env, &msg);
        caller
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: deliver,
                args: vec![js_val],
            });
    }
}

pub(super) fn port_close(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    if let Some(cluster) = cluster_of(caller)
        && let Some(port) = cluster.port(port_id)
    {
        port.closed.store(true, Ordering::SeqCst);
        port.inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *port.wake_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    if let Some(binding) = caller
        .data()
        .message_port_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&port_id)
    {
        binding.started = false;
        binding.deliver_cb = None;
        binding.ref_guard = None;
    }
    value::encode_undefined()
}

pub(super) fn port_ref(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    ensure_local_port(caller, port_id);
    let mut map = caller
        .data()
        .message_port_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(binding) = map.get_mut(&port_id)
        && binding.ref_guard.is_none()
        && let Some(counter) = caller.data().async_op_counter.clone()
    {
        binding.ref_guard = Some(counter.begin());
    }
    value::encode_undefined()
}

pub(super) fn port_unref(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    if let Some(binding) = caller
        .data()
        .message_port_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&port_id)
    {
        binding.ref_guard = None;
    }
    value::encode_undefined()
}

pub(super) fn receive_message_on_port(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(port_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    let Some(cluster) = cluster_of(caller) else {
        return value::encode_undefined();
    };
    let Some(port) = cluster.port(port_id) else {
        return value::encode_undefined();
    };
    let Some(msg) = port
        .inbox
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
    else {
        return value::encode_undefined();
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let js_val = deserialize_value(caller, &env, &msg);
    let obj = alloc_host_object(caller, &env, 1);
    let _ = define_host_data_property_from_caller(caller, obj, "message", js_val);
    obj
}

/// 默认 ref 住 MessagePort，使 scheduler 在无 timer 时仍因 async op counter 存活。
pub(crate) fn auto_ref_port_on_store(store: &mut Store<RuntimeState>, port_id: u32) {
    {
        let mut map = store
            .data()
            .message_port_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(port_id).or_insert(LocalPortBinding {
            global_id: port_id,
            deliver_cb: None,
            started: false,
            ref_guard: None,
        });
    }
    let counter = store.data().async_op_counter.clone();
    let mut map = store
        .data()
        .message_port_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(binding) = map.get_mut(&port_id)
        && binding.ref_guard.is_none()
        && let Some(counter) = counter
    {
        binding.ref_guard = Some(counter.begin());
    }
}
