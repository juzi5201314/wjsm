//! `node:vm` host bridge：createContext / isContext / runIn* 等。
//!
//! 单堆多 realm：createContext 克隆 pristine 图并 contextify sandbox；
//! runInContext 在 execution_realm 帧内 eval，scope_env = sandbox。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wasmtime::{AsContextMut, Caller};

use crate::realm::RealmId;
use crate::realm_clone::clone_pristine_realm;
use crate::runtime_encoding::js_string_lossy;
use crate::runtime_eval::perform_eval_from_caller_async;
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
    let options = args.get(2).copied();

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

    let timeout_ms = parse_timeout_ms(caller, options);
    eval_in_realm(caller, code_val, Some(sandbox), realm_id, timeout_ms).await
}

async fn run_in_new_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    // sandbox 可选：args[1]；options 可能在 args[1]（无 sandbox）或 args[2]
    let (sandbox_arg, options) = match args.get(1).copied() {
        Some(s) if value::is_object(s) || value::is_array(s) || value::is_undefined(s) || value::is_null(s) => {
            // 若看起来像 options（含 timeout 字段）且未 contextify，仍当 sandbox 用
            (Some(s), args.get(2).copied())
        }
        Some(s) => (None, Some(s)),
        None => (None, None),
    };
    let create_args: Vec<i64> = match sandbox_arg {
        Some(s) if !value::is_undefined(s) && !value::is_null(s) => vec![s],
        _ => vec![],
    };
    let sandbox = create_context(caller, &create_args);
    if value::is_exception(sandbox) {
        return sandbox;
    }
    let timeout_ms = parse_timeout_ms(caller, options);
    // 若 options 在 args[1] 且无独立 sandbox 对象
    let timeout_ms = timeout_ms.or_else(|| parse_timeout_ms(caller, sandbox_arg));
    eval_in_realm(
        caller,
        code_val,
        Some(sandbox),
        // realm from create_context side table
        {
            let h = object_handle_idx(sandbox).unwrap_or(0);
            caller
                .data()
                .contextified
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&h)
                .copied()
                .unwrap_or(RealmId(0))
        },
        timeout_ms,
    )
    .await
}

async fn run_in_this_context(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let options = args.get(1).copied();
    let timeout_ms = parse_timeout_ms(caller, options);
    eval_in_realm(caller, code_val, None, RealmId(0), timeout_ms).await
}

/// 从 options 对象读取 `timeout` 毫秒（Node 兼容）。
fn parse_timeout_ms(caller: &mut Caller<'_, RuntimeState>, options: Option<i64>) -> Option<u64> {
    let Some(opts) = options else {
        return None;
    };
    if !value::is_object(opts) {
        return None;
    }
    let Some(ptr) = resolve_handle(caller, opts) else {
        return None;
    };
    let raw = read_object_property_by_name(caller, ptr, "timeout")?;
    if value::is_undefined(raw) || value::is_null(raw) {
        return None;
    }
    let n = value::decode_f64(raw);
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some(n as u64)
}

async fn eval_in_realm(
    caller: &mut Caller<'_, RuntimeState>,
    code_val: i64,
    scope_env: Option<i64>,
    realm_id: RealmId,
    timeout_ms: Option<u64>,
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
    let prev_realm = caller
        .data()
        .execution_realm
        .swap(realm_id.0, Ordering::Relaxed);
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

    // timeout：epoch trap 作用域 + 解释器 Instant deadline
    let timeout_guard = timeout_ms.map(|ms| arm_vm_timeout(caller, ms));

    let result = perform_eval_from_caller_async(caller, code_val, scope_env).await;

    // 必定恢复 epoch 策略与 deadline（含 trap 路径）
    if let Some(g) = timeout_guard {
        g.disarm(caller);
    }

    let _ = env
        .array_proto_handle
        .set(&mut *caller, wasmtime::Val::I32(prev_array));
    let _ = env
        .object_proto_handle
        .set(&mut *caller, wasmtime::Val::I32(prev_object));
    caller
        .data()
        .execution_realm
        .store(prev_realm, Ordering::Relaxed);

    // 将 epoch interrupt trap 映射为 Node 风格 timeout 错误
    if value::is_undefined(result) {
        let err = caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(msg) = err {
            if msg.contains("epoch")
                || msg.contains("interrupt")
                || msg.contains("timed out")
                || msg.contains("timeout")
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                return make_type_error_exception(caller, "Error: Script execution timed out.");
            }
        }
    }
    if value::is_exception(result) {
        // 解释器路径可能以 exception 抛出 timeout 文案
        return result;
    }
    result
}

/// 武装 vm timeout：切换 epoch 为 trap + 后台 increment_epoch；设置解释器 deadline。
struct VmTimeoutGuard {
    cancel: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

fn arm_vm_timeout(caller: &mut Caller<'_, RuntimeState>, timeout_ms: u64) -> VmTimeoutGuard {
    // 解释器 deadline
    {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        *caller
            .data()
            .vm_deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(deadline);
    }

    // 编译路径：临时把 epoch 策略改为 trap（退出时恢复 async_yield）
    {
        let mut store = caller.as_context_mut();
        store.epoch_deadline_trap();
        store.set_epoch_deadline(1);
    }

    let engine = caller.engine().clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_bg = Arc::clone(&cancel);
    let join = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(timeout_ms.max(1)) {
            if cancel_bg.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if !cancel_bg.load(Ordering::Relaxed) {
            engine.increment_epoch();
        }
    });

    VmTimeoutGuard {
        cancel,
        join: Some(join),
    }
}

impl VmTimeoutGuard {
    fn disarm(mut self, caller: &mut Caller<'_, RuntimeState>) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        *caller
            .data()
            .vm_deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        // 恢复 async-yield epoch 策略
        let mut store = caller.as_context_mut();
        store.epoch_deadline_async_yield_and_update(1);
        store.set_epoch_deadline(1);
    }
}

impl Drop for VmTimeoutGuard {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        // Drop 路径无法拿 Caller 恢复 epoch；正常路径必须走 disarm。
    }
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
