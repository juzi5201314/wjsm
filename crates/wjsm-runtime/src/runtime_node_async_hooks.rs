//! `node:async_hooks` host bridge（Phase 0–1）。
//! ALS / AsyncResource 的用户回调在 **JS 侧** 调用；host 只做 id / frame enter-exit。

use crate::runtime_async_hooks::{CapturedScope, FrameId};
use crate::runtime_encoding::js_string_lossy;
use crate::*;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AsyncHooksMethodKind {
    ExecutionAsyncId = 0,
    TriggerAsyncId = 1,
    ExecutionAsyncResource = 2,
    AlsNew = 3,
    AlsEnterWith = 4,
    AlsGetStore = 5,
    AlsDisable = 6,
    AsyncResourceNew = 7,
    AsyncResourceEnter = 8,
    AsyncResourceExit = 9,
    AsyncResourceEmitDestroy = 10,
    AsyncResourceAsyncId = 11,
    AsyncResourceTriggerAsyncId = 12,
    SetImmediate = 13,
    ClearImmediate = 14,
    Providers = 15,
    CreateHook = 16,
    AlsCaptureFrame = 17,
    AlsPushFrame = 18,
    AlsPopFrame = 19,
    HookEnable = 20,
    HookDisable = 21,
}
#[allow(dead_code)]
impl AsyncHooksMethodKind {
    pub(crate) fn method(self) -> u8 {
        self as u8
    }
    pub(crate) fn from_method(m: u8) -> Option<Self> {
        match m {
            x if x == Self::ExecutionAsyncId as u8 => Some(Self::ExecutionAsyncId),
            x if x == Self::TriggerAsyncId as u8 => Some(Self::TriggerAsyncId),
            x if x == Self::ExecutionAsyncResource as u8 => Some(Self::ExecutionAsyncResource),
            x if x == Self::AlsNew as u8 => Some(Self::AlsNew),
            x if x == Self::AlsEnterWith as u8 => Some(Self::AlsEnterWith),
            x if x == Self::AlsGetStore as u8 => Some(Self::AlsGetStore),
            x if x == Self::AlsDisable as u8 => Some(Self::AlsDisable),
            x if x == Self::AsyncResourceNew as u8 => Some(Self::AsyncResourceNew),
            x if x == Self::AsyncResourceEnter as u8 => Some(Self::AsyncResourceEnter),
            x if x == Self::AsyncResourceExit as u8 => Some(Self::AsyncResourceExit),
            x if x == Self::AsyncResourceEmitDestroy as u8 => Some(Self::AsyncResourceEmitDestroy),
            x if x == Self::AsyncResourceAsyncId as u8 => Some(Self::AsyncResourceAsyncId),
            x if x == Self::AsyncResourceTriggerAsyncId as u8 => {
                Some(Self::AsyncResourceTriggerAsyncId)
            }
            x if x == Self::SetImmediate as u8 => Some(Self::SetImmediate),
            x if x == Self::ClearImmediate as u8 => Some(Self::ClearImmediate),
            x if x == Self::Providers as u8 => Some(Self::Providers),
            x if x == Self::CreateHook as u8 => Some(Self::CreateHook),
            x if x == Self::AlsCaptureFrame as u8 => Some(Self::AlsCaptureFrame),
            x if x == Self::AlsPushFrame as u8 => Some(Self::AlsPushFrame),
            x if x == Self::AlsPopFrame as u8 => Some(Self::AlsPopFrame),
            x if x == Self::HookEnable as u8 => Some(Self::HookEnable),
            x if x == Self::HookDisable as u8 => Some(Self::HookDisable),
            _ => None,
        }
    }
}

static NEXT_IMMEDIATE_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) fn create_async_hooks_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 24);
    let temp = caller.data().push_host_temp_roots([obj]);
    for (name, kind) in [
        ("executionAsyncId", AsyncHooksMethodKind::ExecutionAsyncId),
        ("triggerAsyncId", AsyncHooksMethodKind::TriggerAsyncId),
        (
            "executionAsyncResource",
            AsyncHooksMethodKind::ExecutionAsyncResource,
        ),
        ("alsNew", AsyncHooksMethodKind::AlsNew),
        ("alsEnterWith", AsyncHooksMethodKind::AlsEnterWith),
        ("alsGetStore", AsyncHooksMethodKind::AlsGetStore),
        ("alsDisable", AsyncHooksMethodKind::AlsDisable),
        ("asyncResourceNew", AsyncHooksMethodKind::AsyncResourceNew),
        (
            "asyncResourceEnter",
            AsyncHooksMethodKind::AsyncResourceEnter,
        ),
        ("asyncResourceExit", AsyncHooksMethodKind::AsyncResourceExit),
        (
            "asyncResourceEmitDestroy",
            AsyncHooksMethodKind::AsyncResourceEmitDestroy,
        ),
        (
            "asyncResourceAsyncId",
            AsyncHooksMethodKind::AsyncResourceAsyncId,
        ),
        (
            "asyncResourceTriggerAsyncId",
            AsyncHooksMethodKind::AsyncResourceTriggerAsyncId,
        ),
        ("setImmediate", AsyncHooksMethodKind::SetImmediate),
        ("clearImmediate", AsyncHooksMethodKind::ClearImmediate),
        ("providers", AsyncHooksMethodKind::Providers),
        ("createHook", AsyncHooksMethodKind::CreateHook),
        ("alsCaptureFrame", AsyncHooksMethodKind::AlsCaptureFrame),
        ("alsPushFrame", AsyncHooksMethodKind::AlsPushFrame),
        ("alsPopFrame", AsyncHooksMethodKind::AlsPopFrame),
    ] {
        let callable =
            create_native_callable(caller.data(), NativeCallable::AsyncHooksMethod { kind });
        let _ = define_host_data_property_from_caller(caller, obj, name, callable);
    }
    {
        let sentinel = alloc_host_object(caller, &env, 0);
        let _ = caller.data().push_host_temp_roots([sentinel]);
        caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_top_level_resource(sentinel);
    }
    caller.data().truncate_host_temp_roots(temp);
    obj
}

pub(crate) fn call_async_hooks_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: AsyncHooksMethodKind,
    this_val: i64,
    args: &[i64],
) -> i64 {
    match kind {
        AsyncHooksMethodKind::ExecutionAsyncId => encode_id(caller, |h| h.execution_async_id()),
        AsyncHooksMethodKind::TriggerAsyncId => encode_id(caller, |h| h.trigger_async_id()),
        AsyncHooksMethodKind::ExecutionAsyncResource => {
            let hooks = caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let r = hooks.execution_async_resource();
            if r == 0 { value::encode_undefined() } else { r }
        }
        AsyncHooksMethodKind::AlsNew => als_new(caller, args),
        AsyncHooksMethodKind::AlsGetStore => {
            let store_obj = args.first().copied().unwrap_or(this_val);
            als_get_store(caller, store_obj)
        }
        AsyncHooksMethodKind::AlsEnterWith => {
            let store_obj = args.first().copied().unwrap_or(this_val);
            let rest = if args.len() > 1 { &args[1..] } else { &[] };
            als_enter_with(caller, store_obj, rest)
        }
        AsyncHooksMethodKind::AlsDisable => {
            let store_obj = args.first().copied().unwrap_or(this_val);
            als_disable(caller, store_obj)
        }
        AsyncHooksMethodKind::AsyncResourceNew => async_resource_new(caller, args),
        AsyncHooksMethodKind::AsyncResourceEnter => {
            let res = args.first().copied().unwrap_or(this_val);
            async_resource_enter(caller, res)
        }
        AsyncHooksMethodKind::AsyncResourceExit => {
            let res = args.first().copied().unwrap_or(this_val);
            async_resource_exit(caller, res)
        }
        AsyncHooksMethodKind::AsyncResourceEmitDestroy => {
            let res = args.first().copied().unwrap_or(this_val);
            async_resource_emit_destroy(caller, res)
        }
        AsyncHooksMethodKind::AsyncResourceAsyncId => {
            let res = args.first().copied().unwrap_or(this_val);
            async_resource_async_id(caller, res)
        }
        AsyncHooksMethodKind::AsyncResourceTriggerAsyncId => {
            let res = args.first().copied().unwrap_or(this_val);
            async_resource_trigger_async_id(caller, res)
        }
        AsyncHooksMethodKind::SetImmediate => set_immediate(caller, args),
        AsyncHooksMethodKind::ClearImmediate => clear_immediate(caller, args),
        AsyncHooksMethodKind::Providers => providers_object(caller),
        AsyncHooksMethodKind::CreateHook => create_hook(caller, args),
        AsyncHooksMethodKind::HookEnable => set_hook_enabled(caller, this_val, true),
        AsyncHooksMethodKind::HookDisable => set_hook_enabled(caller, this_val, false),
        AsyncHooksMethodKind::AlsCaptureFrame => {
            let fid = caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain_current_frame();
            match fid {
                Some(crate::FrameId(id)) => value::encode_f64(id as f64),
                None => value::encode_f64(-1.0),
            }
        }
        AsyncHooksMethodKind::AlsPushFrame => {
            let id = args
                .first()
                .copied()
                .filter(|v| value::is_f64(*v))
                .map(|v| value::decode_f64(v));
            let mut hooks = caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prior = hooks.current_frame();
            if let Some(fid) = id {
                if fid >= 0.0 {
                    hooks.set_current_frame(Some(crate::FrameId(fid as u64)));
                } else {
                    hooks.set_current_frame(None);
                }
            }
            match prior {
                Some(crate::FrameId(id)) => value::encode_f64(id as f64),
                None => value::encode_f64(-1.0),
            }
        }
        AsyncHooksMethodKind::AlsPopFrame => {
            let id = args
                .first()
                .copied()
                .filter(|v| value::is_f64(*v))
                .map(|v| value::decode_f64(v));
            let mut hooks = caller
                .data()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(fid) = id {
                if fid >= 0.0 {
                    hooks.set_current_frame(Some(crate::FrameId(fid as u64)));
                } else {
                    hooks.set_current_frame(None);
                }
            }
            value::encode_undefined()
        }
    }
}

fn encode_id(
    caller: &Caller<'_, RuntimeState>,
    f: impl FnOnce(&crate::AsyncHooksState) -> u64,
) -> i64 {
    let hooks = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    value::encode_f64(f(&hooks) as f64)
}

fn prop_f64(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str) -> Option<f64> {
    let env = WasmEnv::from_caller(caller)?;
    let ptr = resolve_handle_idx_with_env(caller, &env, (obj as u64 & 0xFFFF_FFFF) as usize)?;
    let v = read_object_property_by_name(caller, ptr, name)?;
    if value::is_f64(v) {
        Some(value::decode_f64(v))
    } else {
        None
    }
}

fn als_key_from_this(_caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> Option<u64> {
    value::is_f64(this_val).then(|| value::decode_f64(this_val) as u64)
}

fn als_new(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let has_default_value = args
        .first()
        .copied()
        .is_some_and(|raw| value::is_bool(raw) && value::decode_bool(raw));
    let default_value = if has_default_value {
        args.get(1).copied().unwrap_or_else(value::encode_undefined)
    } else {
        value::encode_undefined()
    };
    let key = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .alloc_als_key(default_value);
    value::encode_f64(key as f64)
}

fn als_get_store(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    let Some(key) = als_key_from_this(caller, this_val) else {
        return value::encode_undefined();
    };
    caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_store(key)
        .unwrap_or_else(value::encode_undefined)
}

fn als_enter_with(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(key) = als_key_from_this(caller, this_val) else {
        return value::encode_undefined();
    };
    let store_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .enter_with_store(key, store_val);
    value::encode_undefined()
}

fn als_disable(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    let Some(key) = als_key_from_this(caller, this_val) else {
        return value::encode_undefined();
    };
    caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .disable_store(key);
    value::encode_undefined()
}

fn async_resource_new(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let type_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let reject_empty_type = js_string_lossy(caller, type_value).is_empty()
        && caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .init_hooks_exist();
    if reject_empty_type {
        return make_type_error_exception(caller, "Invalid type: \"\"");
    }
    let mut trigger = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .default_trigger_async_id();
    let mut manual = false;
    if let Some(t) = args.get(1).copied() {
        if value::is_f64(t) {
            trigger = value::decode_f64(t) as u64;
        } else if value::is_object(t)
            && let Some(ptr) =
                resolve_handle_idx_with_env(caller, &env, (t as u64 & 0xFFFF_FFFF) as usize)
        {
            if let Some(v) = read_object_property_by_name(caller, ptr, "triggerAsyncId")
                && value::is_f64(v)
            {
                trigger = value::decode_f64(v) as u64;
            }
            if let Some(v) = read_object_property_by_name(caller, ptr, "requireManualDestroy")
                && value::is_bool(v)
            {
                manual = value::decode_bool(v);
            }
        }
    }
    let (async_id, frame) = {
        let mut hooks = caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (hooks.new_async_id(), hooks.retain_current_frame())
    };
    let obj = alloc_host_object(caller, &env, 10);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__async_id__",
        value::encode_f64(async_id as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__trigger_async_id__",
        value::encode_f64(trigger as f64),
    );
    if let Some(FrameId(fid)) = frame {
        let _ = define_host_data_property_from_caller(
            caller,
            obj,
            "__frame_id__",
            value::encode_f64(fid as f64),
        );
    }
    caller.data().push_host_temp_roots([type_value]);
    let _ = define_host_data_property_from_caller(caller, obj, "__type__", type_value);
    caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register_resource(
            async_id,
            crate::ResourceMeta {
                resource: obj,
                manual_destroy: manual,
                destroyed: false,
            },
        );
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn async_resource_enter(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    let Some(aid) = prop_f64(caller, this_val, "__async_id__").map(|f| f as u64) else {
        return value::encode_undefined();
    };
    let tid = prop_f64(caller, this_val, "__trigger_async_id__")
        .map(|f| f as u64)
        .unwrap_or(0);
    let frame = prop_f64(caller, this_val, "__frame_id__").map(|f| FrameId(f as u64));
    let scope = CapturedScope {
        async_id: aid,
        trigger_async_id: tid,
        resource: this_val,
        frame_id: frame,
    };
    let prior = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .enter_captured_scope(scope);
    // 把 prior frame id 存到 this 上供 exit 使用
    let prior_f = prior.map(|FrameId(x)| x as f64).unwrap_or(-1.0);
    let _ = define_host_data_property_from_caller(
        caller,
        this_val,
        "__prior_frame__",
        value::encode_f64(prior_f),
    );
    value::encode_undefined()
}

fn async_resource_exit(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    let Some(aid) = prop_f64(caller, this_val, "__async_id__").map(|f| f as u64) else {
        return value::encode_undefined();
    };
    let tid = prop_f64(caller, this_val, "__trigger_async_id__")
        .map(|f| f as u64)
        .unwrap_or(0);
    let frame = prop_f64(caller, this_val, "__frame_id__").map(|f| FrameId(f as u64));
    let prior = prop_f64(caller, this_val, "__prior_frame__").and_then(|f| {
        if f < 0.0 {
            None
        } else {
            Some(FrameId(f as u64))
        }
    });
    let scope = CapturedScope {
        async_id: aid,
        trigger_async_id: tid,
        resource: this_val,
        frame_id: frame,
    };
    caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .exit_captured_scope(scope, prior);
    value::encode_undefined()
}

fn async_resource_async_id(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    prop_f64(caller, this_val, "__async_id__")
        .map(value::encode_f64)
        .unwrap_or_else(value::encode_undefined)
}

fn async_resource_trigger_async_id(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    prop_f64(caller, this_val, "__trigger_async_id__")
        .map(value::encode_f64)
        .unwrap_or_else(value::encode_undefined)
}

fn async_resource_emit_destroy(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    if let Some(aid) = prop_f64(caller, this_val, "__async_id__").map(|f| f as u64) {
        caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue_destroy(aid);
    }
    this_val
}

fn set_immediate(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let callback = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if !is_callable_in_runtime(caller, callback) {
        return make_type_error_exception(
            caller,
            "TypeError: setImmediate callback must be a function",
        );
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let id = NEXT_IMMEDIATE_ID.fetch_add(1, Ordering::Relaxed);
    let resource = create_timer_resource_object(caller, &env, id, "Immediate");
    let scope = {
        let mut hooks = caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hooks.capture_for_scheduled_callback(resource, true)
    };
    caller
        .data()
        .immediate_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(ImmediateEntry {
            id,
            callback,
            resource,
            scope,
        });
    resource
}
async fn set_immediate_async(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let result = set_immediate(caller, args);
    if value::is_exception(result) {
        return result;
    }
    let scope = caller
        .data()
        .immediate_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .back()
        .and_then(|entry| entry.scope);
    if let Some(scope) = scope {
        let type_value = store_runtime_string(caller, "Immediate".to_string());
        let _ = crate::runtime_async_hooks::emit::emit_init_from_caller(
            caller,
            scope.async_id,
            type_value,
            scope.trigger_async_id,
            scope.resource,
            false,
        )
        .await;
    }
    result
}

async fn async_resource_new_async(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let resource = async_resource_new(caller, args);
    if value::is_exception(resource) {
        return resource;
    }
    let async_id = prop_f64(caller, resource, "__async_id__").unwrap_or(0.0) as u64;
    let trigger_async_id = prop_f64(caller, resource, "__trigger_async_id__").unwrap_or(0.0) as u64;
    let type_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let _ = crate::runtime_async_hooks::emit::emit_init_from_caller(
        caller,
        async_id,
        type_value,
        trigger_async_id,
        resource,
        false,
    )
    .await;
    resource
}

async fn async_resource_enter_async(caller: &mut Caller<'_, RuntimeState>, resource: i64) -> i64 {
    let result = async_resource_enter(caller, resource);
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    if let Some(async_id) = prop_f64(caller, resource, "__async_id__").map(|id| id as u64) {
        let _ = crate::runtime_async_hooks::emit::emit_before(caller, &env, async_id, false).await;
    }
    result
}

async fn async_resource_exit_async(caller: &mut Caller<'_, RuntimeState>, resource: i64) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    if let Some(async_id) = prop_f64(caller, resource, "__async_id__").map(|id| id as u64) {
        let _ = crate::runtime_async_hooks::emit::emit_after(caller, &env, async_id, false).await;
    }
    async_resource_exit(caller, resource)
}

pub(crate) async fn call_async_hooks_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    kind: AsyncHooksMethodKind,
    this_val: i64,
    args: &[i64],
) -> i64 {
    match kind {
        AsyncHooksMethodKind::SetImmediate => set_immediate_async(caller, args).await,
        AsyncHooksMethodKind::AsyncResourceNew => async_resource_new_async(caller, args).await,
        AsyncHooksMethodKind::AsyncResourceEnter => {
            async_resource_enter_async(caller, args.first().copied().unwrap_or(this_val)).await
        }
        AsyncHooksMethodKind::AsyncResourceExit => {
            async_resource_exit_async(caller, args.first().copied().unwrap_or(this_val)).await
        }
        _ => call_async_hooks_method(caller, kind, this_val, args),
    }
}

fn clear_immediate(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    if let Some(id) = timer_id_from_arg(caller, args.first().copied()) {
        caller
            .data()
            .immediate_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|e| e.id != id);
    }
    value::encode_undefined()
}

pub(crate) fn create_timer_resource_object(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    id: u32,
    brand: &str,
) -> i64 {
    let obj = alloc_host_object(caller, env, 4);
    let temp = caller.data().push_host_temp_roots([obj]);
    let brand_val = store_runtime_string(caller, brand.to_string());
    let _ = caller.data().push_host_temp_roots([brand_val]);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__timer_id__",
        value::encode_f64(id as f64),
    );
    let _ = define_host_data_property_from_caller(caller, obj, "__brand__", brand_val);
    let ctor = alloc_host_object(caller, env, 1);
    let _ = caller.data().push_host_temp_roots([ctor]);
    let name_val = store_runtime_string(caller, brand.to_string());
    let _ = define_host_data_property_from_caller(caller, ctor, "name", name_val);
    let _ = define_host_data_property_from_caller(caller, obj, "constructor", ctor);
    caller.data().truncate_host_temp_roots(temp);
    obj
}

pub(crate) fn timer_id_from_arg(
    caller: &mut Caller<'_, RuntimeState>,
    arg: Option<i64>,
) -> Option<u32> {
    let v = arg?;
    if value::is_f64(v) {
        return Some(value::decode_f64(v) as u32);
    }
    if value::is_object(v) {
        return prop_f64(caller, v, "__timer_id__").map(|f| f as u32);
    }
    None
}

fn providers_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    let _ = define_host_data_property_from_caller(caller, obj, "NONE", value::encode_f64(0.0));
    let _ = define_host_data_property_from_caller(caller, obj, "PROMISE", value::encode_f64(27.0));
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn create_hook(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let callback = |index: usize| {
        args.get(index)
            .copied()
            .filter(|raw| !value::is_undefined(*raw))
            .unwrap_or(0)
    };
    let track_promises = args
        .get(5)
        .copied()
        .is_some_and(|raw| value::is_bool(raw) && value::decode_bool(raw));
    let id = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register_hook(crate::runtime_async_hooks::HookRecord {
            id: 0,
            init: callback(0),
            before: callback(1),
            after: callback(2),
            destroy: callback(3),
            promise_resolve: callback(4),
            track_promises,
            enabled: false,
        });

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 3);
    let root = caller.data().push_host_temp_roots([obj]);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__hook_id__",
        value::encode_f64(id as f64),
    );
    for (name, kind) in [
        ("enable", AsyncHooksMethodKind::HookEnable),
        ("disable", AsyncHooksMethodKind::HookDisable),
    ] {
        let callable =
            create_native_callable(caller.data(), NativeCallable::AsyncHooksMethod { kind });
        let _ = define_host_data_property_from_caller(caller, obj, name, callable);
    }
    caller.data().truncate_host_temp_roots(root);
    obj
}

fn set_hook_enabled(caller: &mut Caller<'_, RuntimeState>, this_val: i64, enabled: bool) -> i64 {
    if let Some(id) = prop_f64(caller, this_val, "__hook_id__") {
        caller
            .data()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_hook_enabled(id as u64, enabled);
    }
    this_val
}

/// 在 nextTick 之后、timers 之前 drain setImmediate。
pub(crate) async fn drain_immediates_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    loop {
        let entry = {
            let mut q = ctx
                .state_mut()
                .immediate_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            q.pop_front()
        };
        let Some(entry) = entry else {
            break;
        };
        let prior = if let Some(scope) = entry.scope {
            let mut hooks = ctx
                .state_mut()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            Some((scope, hooks.enter_captured_scope(scope)))
        } else {
            None
        };
        if let Some(scope) = entry.scope
            && crate::runtime_async_hooks::emit::emit_before(ctx, env, scope.async_id, false).await
        {
            return;
        }

        if is_callable_with_env(ctx, env, entry.callback) {
            let _ = call_host_function_with_args_async(
                ctx,
                env,
                entry.callback,
                value::encode_undefined(),
                &[],
            )
            .await;
        }
        if let Some(scope) = entry.scope
            && crate::runtime_async_hooks::emit::emit_after(ctx, env, scope.async_id, false).await
        {
            return;
        }

        if let Some((scope, prior_frame)) = prior {
            let mut hooks = ctx
                .state_mut()
                .async_hooks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            hooks.exit_captured_scope(scope, prior_frame);
        }
        if let Some(scope) = entry.scope
            && crate::runtime_async_hooks::emit::emit_destroy(ctx, env, scope.async_id, false).await
        {
            return;
        }

        if crate::runtime_process::pending_process_exit_signal(ctx.state_mut()).is_some() {
            return;
        }
    }
}
