use wasmtime::{Caller, Store};
use wjsm_ir::value;

use crate::runtime_process::ProcessNextTickTask;
use crate::*;

use super::ipc::IpcMessage;
use super::make_type_error_exception;
use super::spawn_async::handle_arg;

pub(super) fn ipc_payload_to_js(store: &mut Store<RuntimeState>, env: &WasmEnv, text: &str) -> i64 {
    match crate::runtime_json::parse_json_text(text) {
        Ok(jv) => crate::runtime_json::build_wasm_value_with_env(store, env, &jv),
        Err(_) => store_runtime_string_in_state(store.data(), text.to_string()),
    }
}

pub(super) fn ipc_payload_to_js_caller(caller: &mut Caller<'_, RuntimeState>, text: &str) -> i64 {
    match crate::runtime_json::parse_json_text(text) {
        Ok(jv) => {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            crate::runtime_json::build_wasm_value_with_env(caller, &env, &jv)
        }
        Err(_) => store_runtime_string(caller, text.to_string()),
    }
}

fn queue_child_messages_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callback: i64,
    messages: Vec<IpcMessage>,
    scope: Option<crate::CapturedScope>,
) {
    for msg in messages {
        let payload = ipc_payload_to_js_caller(caller, &msg.payload);
        let fd_val = msg
            .fd
            .map(|fd| value::encode_f64(fd as f64))
            .unwrap_or_else(value::encode_undefined);
        caller
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback,
                args: vec![payload, fd_val],
                scope,
            });
    }
}

pub(super) fn child_on_message(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childOnMessage: invalid id");
    };
    let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let scope = {
        let mut map = caller
            .data()
            .child_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(binding) = map.get_mut(&handle) else {
            return value::encode_undefined();
        };
        binding.message_cb = Some(callback);
        binding.message_cb_ready = false;
        binding.scope
    };
    // 同一把锁内：pending 为空则 ready=true，否则取出 pending 并保持 ready=false。
    // 禁止在锁外把 ready 置 true，否则 drain 会在 ready=false 窗口把 inbox 塞进 pending，
    // 随后 ready=true 却永不 drain pending → IPC 消息永久丢失 → cluster queryServer 挂死。
    loop {
        let pending = {
            let mut map = caller
                .data()
                .child_bindings
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(binding) = map.get_mut(&handle) else {
                return value::encode_undefined();
            };
            if binding.pending_messages.is_empty() {
                binding.message_cb_ready = true;
                Vec::new()
            } else {
                // 回放期间保持 ready=false，新消息继续进 pending，下一轮再取
                binding.message_cb_ready = false;
                std::mem::take(&mut binding.pending_messages)
            }
        };
        if pending.is_empty() {
            break;
        }
        queue_child_messages_caller(caller, callback, pending, scope);
    }
    value::encode_undefined()
}

pub(crate) fn drain_child_messages(store: &mut Store<RuntimeState>, env: &WasmEnv, handle: u32) {
    let (messages, callback, scope) = {
        let endpoint = {
            let inner = store
                .data()
                .child_process_table
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            inner
                .entries
                .get(handle as usize)
                .and_then(|entry| entry.as_ref())
                .and_then(|entry| entry.ipc.as_ref())
                .and_then(|ipc| ipc.try_endpoint())
        };
        let mut messages = endpoint.map(|ipc| ipc.drain_inbox()).unwrap_or_default();
        let mut bindings = store
            .data()
            .child_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(binding) = bindings.get_mut(&handle) else {
            return;
        };
        let Some(callback) = binding.message_cb else {
            // 回调尚未注册：inbox 暂存 pending，等 child_on_message 回放
            binding.pending_messages.append(&mut messages);
            return;
        };
        if !binding.message_cb_ready {
            // 回放窗口：绝不能直接投递，否则与 onMessage 回放乱序/重复
            binding.pending_messages.append(&mut messages);
            return;
        }
        // ready：必须合并可能残留的 pending（防止历史窗口丢消息），再投递
        if binding.pending_messages.is_empty() {
            (messages, callback, binding.scope)
        } else {
            let mut pending = std::mem::take(&mut binding.pending_messages);
            pending.append(&mut messages);
            (pending, callback, binding.scope)
        }
    };
    for msg in messages {
        let payload = ipc_payload_to_js(store, env, &msg.payload);
        let fd_val = msg
            .fd
            .map(|fd| value::encode_f64(fd as f64))
            .unwrap_or_else(value::encode_undefined);
        store
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback,
                args: vec![payload, fd_val],
                scope,
            });
    }
}
