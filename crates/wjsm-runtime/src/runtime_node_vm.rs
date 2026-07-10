//! `node:vm` host bridge：createContext / isContext / runIn* 等。
//!
//! 单堆多 realm：createContext 克隆 pristine 图并 contextify sandbox；
//! runInContext 在 execution_realm 帧内 eval，scope_env = sandbox。

use std::collections::HashMap;
use std::sync::Mutex;

use wasmtime::Caller;

use crate::realm::RealmId;
use crate::realm_clone::clone_pristine_realm;
use crate::runtime_eval::perform_eval_from_caller_async;
use crate::runtime_encoding::js_string_lossy;
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VmMethodKind {
    CreateContext,
    IsContext,
    RunInContext,
    RunInNewContext,
    RunInThisContext,
    CompileFunction,
    ScriptRunInContext,
    ScriptRunInNewContext,
    ScriptRunInThisContext,
}

impl VmMethodKind {
    pub(crate) fn method(self) -> u8 {
        match self {
            Self::CreateContext => 0,
            Self::IsContext => 1,
            Self::RunInContext => 2,
            Self::RunInNewContext => 3,
            Self::RunInThisContext => 4,
            Self::CompileFunction => 5,
            Self::ScriptRunInContext => 6,
            Self::ScriptRunInNewContext => 7,
            Self::ScriptRunInThisContext => 8,
        }
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::CreateContext),
            1 => Some(Self::IsContext),
            2 => Some(Self::RunInContext),
            3 => Some(Self::RunInNewContext),
            4 => Some(Self::RunInThisContext),
            5 => Some(Self::CompileFunction),
            6 => Some(Self::ScriptRunInContext),
            7 => Some(Self::ScriptRunInNewContext),
            8 => Some(Self::ScriptRunInThisContext),
            _ => None,
        }
    }
}

/// contextified sandbox handle → RealmId（side table，不改对象布局）。
pub(crate) type ContextifiedTable = Mutex<HashMap<u32, RealmId>>;

pub(crate) fn empty_contextified_table() -> ContextifiedTable {
    Mutex::new(HashMap::new())
}

pub(crate) fn create_vm_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 16);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    for (name, kind) in [
        ("createContext", VmMethodKind::CreateContext),
        ("isContext", VmMethodKind::IsContext),
        ("runInContext", VmMethodKind::RunInContext),
        ("runInNewContext", VmMethodKind::RunInNewContext),
        ("runInThisContext", VmMethodKind::RunInThisContext),
        ("compileFunction", VmMethodKind::CompileFunction),
        ("scriptRunInContext", VmMethodKind::ScriptRunInContext),
        ("scriptRunInNewContext", VmMethodKind::ScriptRunInNewContext),
        ("scriptRunInThisContext", VmMethodKind::ScriptRunInThisContext),
    ] {
        install_vm_method(caller, obj, name, kind);
    }
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn install_vm_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: VmMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::VmMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

pub(crate) fn call_vm_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: VmMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        VmMethodKind::CreateContext => create_context(caller, args),
        VmMethodKind::IsContext => is_context(caller, args),
        // 异步路径见 call_vm_method_async
        VmMethodKind::RunInContext
        | VmMethodKind::RunInNewContext
        | VmMethodKind::RunInThisContext
        | VmMethodKind::CompileFunction
        | VmMethodKind::ScriptRunInContext
        | VmMethodKind::ScriptRunInNewContext
        | VmMethodKind::ScriptRunInThisContext => make_type_error_exception(
            caller,
            "TypeError: vm async method must be invoked on async path",
        ),
    }
}

pub(crate) async fn call_vm_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    kind: VmMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        VmMethodKind::CreateContext => create_context(caller, args),
        VmMethodKind::IsContext => is_context(caller, args),
        VmMethodKind::RunInContext => run_in_context(caller, args).await,
        VmMethodKind::RunInNewContext => run_in_new_context(caller, args).await,
        VmMethodKind::RunInThisContext => run_in_this_context(caller, args).await,
        VmMethodKind::CompileFunction => {
            make_type_error_exception(caller, "Error: vm.compileFunction not fully wired yet")
        }
        VmMethodKind::ScriptRunInContext => run_in_context(caller, args).await,
        VmMethodKind::ScriptRunInNewContext => run_in_new_context(caller, args).await,
        VmMethodKind::ScriptRunInThisContext => run_in_this_context(caller, args).await,
    }
}

fn create_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let env = match WasmEnv::from_caller(caller) {
        Some(e) => e,
        None => return value::encode_undefined(),
    };
    let sandbox = match args.first().copied() {
        Some(s) if value::is_object(s) || value::is_array(s) => s,
        Some(s) if value::is_undefined(s) || value::is_null(s) => {
            alloc_host_object(caller, &env, 16)
        }
        Some(_) => {
            return make_type_error_exception(
                caller,
                "TypeError: sandbox argument must be an object",
            );
        }
        None => alloc_host_object(caller, &env, 16),
    };

    // 已 contextified 则幂等返回
    if let Some(h) = object_handle_idx(sandbox) {
        let table = caller
            .data()
            .contextified
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if table.contains_key(&h) {
            return sandbox;
        }
    }

    let realm = match clone_pristine_realm(caller, &env, sandbox) {
        Ok(r) => r,
        Err(e) => {
            return make_type_error_exception(
                caller,
                &format!("Error: vm.createContext failed: {e}"),
            );
        }
    };

    if let Some(h) = object_handle_idx(sandbox) {
        caller
            .data()
            .contextified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(h, realm.id);
    }
    sandbox
}

fn is_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(sandbox) = args.first().copied() else {
        return value::encode_bool(false);
    };
    let Some(h) = object_handle_idx(sandbox) else {
        return value::encode_bool(false);
    };
    let yes = caller
        .data()
        .contextified
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&h);
    value::encode_bool(yes)
}

async fn run_in_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let sandbox = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);

    let Some(h) = object_handle_idx(sandbox) else {
        return make_type_error_exception(
            caller,
            "TypeError: contextifiedSandbox must be a contextified object",
        );
    };
    let realm_id = caller
        .data()
        .contextified
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&h)
        .copied();
    let Some(realm_id) = realm_id else {
        return make_type_error_exception(
            caller,
            "TypeError: contextifiedSandbox must be a contextified object",
        );
    };

    eval_in_realm(caller, code_val, Some(sandbox), realm_id).await
}

async fn run_in_new_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    // sandbox 可选：args[1]
    let sandbox_arg = args.get(1).copied();
    let create_args: Vec<i64> = match sandbox_arg {
        Some(s) => vec![s],
        None => vec![],
    };
    let sandbox = create_context(caller, &create_args);
    if value::is_exception(sandbox) {
        return sandbox;
    }
    run_in_context(caller, &[code_val, sandbox]).await
}

async fn run_in_this_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    // 主 realm，不绑 sandbox
    eval_in_realm(caller, code_val, None, RealmId(0)).await
}

async fn eval_in_realm(
    caller: &mut Caller<'_, RuntimeState>,
    code_val: i64,
    scope_env: Option<i64>,
    realm_id: RealmId,
) -> i64 {
    let env = match WasmEnv::from_caller(caller) {
        Some(e) => e,
        None => return value::encode_undefined(),
    };

    // 字符串化 code（允许非 string，对齐 Node ToString）
    let code_val = if value::is_string(code_val) {
        code_val
    } else {
        let s = js_string_lossy(caller, code_val);
        store_runtime_string(caller, s)
    };

    // 帧内 eval：swap proto globals + execution_realm
    // with_execution_realm_frame 是同步的，内部用 block_on 不合适；
    // 手动 enter/exit 包住 await。
    let prev_realm = caller
        .data()
        .execution_realm
        .swap(realm_id.0, std::sync::atomic::Ordering::Relaxed);
    let prev_array = env
        .array_proto_handle
        .get(&mut *caller)
        .i32()
        .unwrap_or(-1);
    let prev_object = env
        .object_proto_handle
        .get(&mut *caller)
        .i32()
        .unwrap_or(-1);

    if let Some((arr, obj)) = resolve_realm_proto_i32(caller, realm_id) {
        let _ = env
            .array_proto_handle
            .set(&mut *caller, wasmtime::Val::I32(arr));
        let _ = env
            .object_proto_handle
            .set(&mut *caller, wasmtime::Val::I32(obj));
    }

    let result = perform_eval_from_caller_async(caller, code_val, scope_env).await;

    let _ = env
        .array_proto_handle
        .set(&mut *caller, wasmtime::Val::I32(prev_array));
    let _ = env
        .object_proto_handle
        .set(&mut *caller, wasmtime::Val::I32(prev_object));
    caller
        .data()
        .execution_realm
        .store(prev_realm, std::sync::atomic::Ordering::Relaxed);

    result
}

fn resolve_realm_proto_i32(
    caller: &Caller<'_, RuntimeState>,
    realm_id: RealmId,
) -> Option<(i32, i32)> {
    let realms = caller
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let r = if realm_id.0 == 0 {
        realms.first()
    } else {
        realms.iter().find(|r| r.id == realm_id)
    }?;
    let arr = if value::is_object(r.intrinsics.array_proto) {
        value::decode_object_handle(r.intrinsics.array_proto) as i32
    } else {
        return None;
    };
    let obj = if value::is_object(r.intrinsics.object_proto) {
        value::decode_object_handle(r.intrinsics.object_proto) as i32
    } else {
        return None;
    };
    Some((arr, obj))
}

fn object_handle_idx(val: i64) -> Option<u32> {
    if value::is_object(val) || value::is_array(val) {
        Some(value::decode_object_handle(val))
    } else {
        None
    }
}
