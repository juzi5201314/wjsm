//! test262 `$262.agent` harness：多 OS 线程共享同一 `SharedRuntimeState`。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use wasmtime::Caller;
use wjsm_ir::value;

use crate::runtime_promises::set_runtime_error;
use crate::runtime_render::read_runtime_string;
use crate::shared_buffer::{
    SharedRuntimeState, materialize_shared_array_buffer_by_handle, read_sab_handle_from_object,
};
use crate::{RuntimeState, WasmEnv, call_wasm_callback_async};

const BROADCAST_WAIT_MS: u64 = 60_000;

pub(crate) fn push_agent_report(shared: &Arc<SharedRuntimeState>, text: String) {
    shared
        .agent_state
        .reports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(text);
}

pub(crate) fn agent_start(caller: &mut Caller<'_, RuntimeState>, script_raw: i64) -> Option<i64> {
    let shared = caller.data().shared_state.clone()?;
    let script = read_runtime_string(caller, script_raw);
    let _ = shared
        .agent_state
        .next_agent_id
        .fetch_add(1, Ordering::Relaxed);

    std::thread::spawn(move || {
        if let Err(e) = run_agent_script(&script, shared.clone()) {
            push_agent_report(&shared, format!("agent error: {e:#}"));
        }
    });

    Some(value::encode_undefined())
}

fn run_agent_script(script: &str, shared: Arc<SharedRuntimeState>) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("agent tokio runtime")?;

    rt.block_on(async {
        let module = wjsm_parser::parse_script_as_module(script).context("agent parse")?;
        let program = wjsm_semantic::lower_module(module, true).context("agent lower")?;
        let wasm_bytes = wjsm_backend_wasm::compile(&program).context("agent compile")?;
        let mut out = Vec::new();
        crate::execute_with_writer_shared_agent(&wasm_bytes, &mut out, shared)
            .await
            .context("agent execute")?;
        Ok(())
    })
}

fn wait_broadcast_callback_done(shared: &Arc<SharedRuntimeState>) {
    let agent = &shared.agent_state;
    let deadline = Instant::now() + Duration::from_millis(BROADCAST_WAIT_MS);
    let mut done = agent
        .broadcast_callback_done
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    while !*done {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        done = agent
            .broadcast_callback_done_cv
            .wait_timeout(done, remaining)
            .expect("callback done wait")
            .0;
    }
}

fn signal_broadcast_callback_done(shared: &Arc<SharedRuntimeState>) {
    let agent = &shared.agent_state;
    {
        let mut done = agent
            .broadcast_callback_done
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *done = true;
    }
    agent.broadcast_callback_done_cv.notify_all();
}

pub(crate) fn agent_broadcast(caller: &mut Caller<'_, RuntimeState>, sab_obj: i64) -> Option<i64> {
    let shared = caller.data().shared_state.clone()?;
    let handle = read_sab_handle_from_object(caller, sab_obj)?;
    let agent = &shared.agent_state;
    let deadline = Instant::now() + Duration::from_millis(BROADCAST_WAIT_MS);
    loop {
        let mut slot = agent
            .broadcast_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if slot.lock == 0 {
            {
                let mut done = agent
                    .broadcast_callback_done
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *done = false;
            }
            slot.sab_handle = Some(handle);
            slot.lock = 1;
            agent.broadcast_cv.notify_all();
            drop(slot);
            wait_broadcast_callback_done(&shared);
            return Some(value::encode_undefined());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            set_runtime_error(
                caller.data(),
                "agent broadcast: timeout waiting for broadcast lock".to_string(),
            );
            return Some(value::encode_undefined());
        }
        slot = agent
            .broadcast_cv
            .wait_timeout(slot, remaining)
            .expect("broadcast wait")
            .0;
    }
}

/// Agent 线程内阻塞：等待 broadcast 并同步执行回调（test262 receiveBroadcast 语义）。
pub(crate) fn agent_receive_broadcast(
    caller: &mut Caller<'_, RuntimeState>,
    callback: i64,
) -> Option<i64> {
    let shared = caller.data().shared_state.clone()?;
    if value::is_undefined(callback) || value::is_null(callback) {
        set_runtime_error(
            caller.data(),
            "TypeError: receiveBroadcast requires a callable".to_string(),
        );
        return Some(value::encode_undefined());
    }

    let handle = wait_broadcast_sab_handle(caller, &shared)?;
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let sab_obj = materialize_shared_array_buffer_by_handle(caller, &env, &shared, handle);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("agent receiveBroadcast tokio runtime");
    if let Err(e) = rt.block_on(call_wasm_callback_async(
        caller,
        callback,
        value::encode_undefined(),
        &[sab_obj],
    )) {
        push_agent_report(&shared, format!("agent callback error: {e:#}"));
    }
    signal_broadcast_callback_done(&shared);
    Some(value::encode_undefined())
}

pub(crate) async fn agent_receive_broadcast_async(
    caller: &mut Caller<'_, RuntimeState>,
    callback: i64,
) -> Option<i64> {
    let shared = match caller.data().shared_state.clone() {
        Some(s) => s,
        None => return Some(value::encode_undefined()),
    };
    if value::is_undefined(callback) || value::is_null(callback) {
        set_runtime_error(
            caller.data(),
            "TypeError: receiveBroadcast requires a callable".to_string(),
        );
        return Some(value::encode_undefined());
    }

    let handle = wait_broadcast_sab_handle(caller, &shared)?;
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let sab_obj = materialize_shared_array_buffer_by_handle(caller, &env, &shared, handle);
    if let Err(e) =
        call_wasm_callback_async(caller, callback, value::encode_undefined(), &[sab_obj]).await
    {
        push_agent_report(&shared, format!("agent callback error: {e:#}"));
    }
    signal_broadcast_callback_done(&shared);
    Some(value::encode_undefined())
}

fn wait_broadcast_sab_handle(
    caller: &mut Caller<'_, RuntimeState>,
    shared: &Arc<SharedRuntimeState>,
) -> Option<u32> {
    let agent = &shared.agent_state;
    let deadline = Instant::now() + Duration::from_millis(BROADCAST_WAIT_MS);
    loop {
        let mut slot = agent
            .broadcast_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if slot.lock == 1 {
            let h = slot.sab_handle.take();
            slot.lock = 0;
            agent.broadcast_cv.notify_all();
            if let Some(h) = h {
                return Some(h);
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            set_runtime_error(caller.data(), "agent receiveBroadcast: timeout".to_string());
            return None;
        }
        slot = agent
            .broadcast_cv
            .wait_timeout(slot, remaining)
            .expect("receive wait")
            .0;
    }
}

pub(crate) fn agent_report(caller: &mut Caller<'_, RuntimeState>, msg: i64) -> Option<i64> {
    let shared = caller.data().shared_state.clone()?;
    let text = read_runtime_string(caller, msg);
    push_agent_report(&shared, text);
    Some(value::encode_undefined())
}
