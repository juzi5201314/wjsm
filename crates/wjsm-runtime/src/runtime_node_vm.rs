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
use crate::realm::MicrotaskMode;

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
        VmMethodKind::CompileFunction => compile_function(caller, args),
        VmMethodKind::ScriptRunInContext => run_in_context(caller, args).await,
        VmMethodKind::ScriptRunInNewContext => run_in_new_context(caller, args).await,
        VmMethodKind::ScriptRunInThisContext => run_in_this_context(caller, args).await,
    }
}

/// `vm.compileFunction(code, params?, options?)` → 可复用 EvalFunction。
///
/// - `params`：字符串形参数组（仅 Ident）
/// - `options.parsingContext`：contextified sandbox；未传时绑定当前全局对象
/// - `options.contextExtensions`：对象环境链（后入者更靠近函数）
fn compile_function(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code_val = args.first().copied().unwrap_or_else(value::encode_undefined);
    let params_val = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let options_val = args.get(2).copied().unwrap_or_else(value::encode_undefined);

    let code = if value::is_string(code_val) {
        js_string_lossy(caller, code_val)
    } else {
        js_string_lossy(caller, code_val)
    };

    let params = match read_compile_function_params(caller, params_val) {
        Ok(p) => p,
        Err(msg) => return make_type_error_exception(caller, &msg),
    };

    let parsing_context = read_options_object_prop(caller, options_val, "parsingContext");
    let context_extensions = read_options_object_prop(caller, options_val, "contextExtensions");

    // parsingContext 必须是 contextified sandbox（若提供）
    let (scope_env, _realm_id) = match resolve_compile_scope_env(
        caller,
        parsing_context,
        context_extensions,
    ) {
        Ok(v) => v,
        Err(msg) => return make_type_error_exception(caller, &msg),
    };

    // Node: compileFunction 不受 codeGeneration.strings 限制（仅 eval/Function 构造器）


    let body_stmts = match parse_function_body_stmts(&code) {
        Ok(stmts) => stmts,
        Err(msg) => {
            return make_syntax_error_exception(caller, &msg);
        }
    };

    let function = EvalFunction {
        params,
        body: body_stmts,
        scope_env: Some(scope_env),
    };
    create_eval_function(caller.data(), function)
}

fn read_compile_function_params(
    caller: &mut Caller<'_, RuntimeState>,
    params_val: i64,
) -> Result<Vec<String>, String> {
    if value::is_undefined(params_val) || value::is_null(params_val) {
        return Ok(Vec::new());
    }
    if !value::is_array(params_val) {
        return Err("params must be an array of strings".to_string());
    }
    let Some(ptr) = resolve_array_ptr(caller, params_val) else {
        return Err("params must be an array of strings".to_string());
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
        if !value::is_string(elem) {
            return Err("params must be an array of strings".to_string());
        }
        let name = js_string_lossy(caller, elem);
        if name.is_empty() || !is_simple_ident(&name) {
            return Err(format!("Invalid parameter name '{name}' for compileFunction"));
        }
        out.push(name);
    }
    Ok(out)
}

fn is_simple_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

fn read_options_object_prop(
    caller: &mut Caller<'_, RuntimeState>,
    options_val: i64,
    name: &str,
) -> i64 {
    if !value::is_object(options_val) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_handle(caller, options_val) else {
        return value::encode_undefined();
    };
    read_object_property_by_name(caller, ptr, name).unwrap_or_else(value::encode_undefined)
}

fn resolve_compile_scope_env(
    caller: &mut Caller<'_, RuntimeState>,
    parsing_context: i64,
    context_extensions: i64,
) -> Result<(i64, RealmId), String> {
    // 解析 parsingContext → (object_env, realm_id)
    let (base_object_env, realm_id) = if value::is_undefined(parsing_context)
        || value::is_null(parsing_context)
    {
        let global = caller
            .data()
            .js_global_object
            .load(Ordering::Relaxed);
        let env = if value::is_object(global) || value::is_array(global) {
            global
        } else {
            // 尚无全局对象时退回 undefined object env
            value::encode_undefined()
        };
        (env, RealmId(0))
    } else if value::is_object(parsing_context) || value::is_array(parsing_context) {
        let Some(h) = object_handle_idx(parsing_context) else {
            return Err("sandbox argument must be an object".to_string());
        };
        let realm_id = {
            let table = caller
                .data()
                .contextified
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match table.get(&h).copied() {
                Some(id) => id,
                None => {
                    return Err(
                        "The second argument must be of type object which has been contextified."
                            .to_string(),
                    );
                }
            }
        };
        (parsing_context, realm_id)
    } else {
        return Err(
            "The second argument must be of type object which has been contextified.".to_string(),
        );
    };

    // 收集 contextExtensions（从后往前包，后入者更靠近函数）
    let mut extension_objs: Vec<i64> = Vec::new();
    if value::is_array(context_extensions) {
        if let Some(ptr) = resolve_array_ptr(caller, context_extensions) {
            let len = read_array_length(caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem =
                    read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
                if !value::is_object(elem) && !value::is_array(elem) {
                    return Err("contextExtensions must be an array of objects".to_string());
                }
                extension_objs.push(elem);
            }
        }
    } else if !value::is_undefined(context_extensions) && !value::is_null(context_extensions) {
        return Err("contextExtensions must be an array of objects".to_string());
    }

    // 环境链：base object_env → ext[0] → ext[1] → ... → ext[n-1]（最后一项最内层）
    // 用 ScopeRecord 链表示：最外层 ScopeRecord.object_env = base
    let mut current_outer: Option<i64>;
    // 先 base
    {
        let data = caller.data_mut();
        let handle = data.scope_record_next_handle;
        data.scope_record_next_handle += 1;
        data.scope_records.insert(
            handle,
            crate::runtime_eval::ScopeRecord {
                bindings: Vec::new(),
                home_object: None,
                new_target: None,
                has_arguments_binding: false,
                is_strict: false,
                outer: None,
                object_env: if value::is_object(base_object_env) || value::is_array(base_object_env)
                {
                    Some(base_object_env)
                } else {
                    None
                },
            },
        );
        current_outer = Some(value::encode_scope_record_handle(handle));
    }
    for obj in extension_objs {
        let data = caller.data_mut();
        let handle = data.scope_record_next_handle;
        data.scope_record_next_handle += 1;
        data.scope_records.insert(
            handle,
            crate::runtime_eval::ScopeRecord {
                bindings: Vec::new(),
                home_object: None,
                new_target: None,
                has_arguments_binding: false,
                is_strict: false,
                outer: current_outer,
                object_env: Some(obj),
            },
        );
        current_outer = Some(value::encode_scope_record_handle(handle));
    }

    let scope_env = current_outer.ok_or_else(|| "Error: failed to build compile scope".to_string())?;
    Ok((scope_env, realm_id))
}

fn parse_function_body_stmts(code: &str) -> Result<Vec<swc_core::ecma::ast::Stmt>, String> {
    // 用 script 模式解析 body 语句序列
    let module = wjsm_parser::parse_script_as_module(code).map_err(|e| e.to_string())?;
    let mut stmts = Vec::with_capacity(module.body.len());
    for item in module.body {
        match item {
            swc_core::ecma::ast::ModuleItem::Stmt(stmt) => stmts.push(stmt),
            swc_core::ecma::ast::ModuleItem::ModuleDecl(_) => {
                return Err("import/export not allowed in compileFunction body".to_string());
            }
        }
    }
    Ok(stmts)
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
    let options = args.get(1).copied();

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

    let mut realm = match clone_pristine_realm(caller, &env, sandbox) {
        Ok(r) => r,
        Err(e) => {
            return make_type_error_exception(
                caller,
                &format!("Error: vm.createContext failed: {e}"),
            );
        }
    };

    // contextCodeGeneration / codeGeneration + microtaskMode
    apply_codegen_options(caller, &mut realm, options);
    apply_microtask_mode_option(caller, &mut realm, options);

    // 写回 active_realms 中刚 push 的条目
    {
        let mut realms = caller
            .data()
            .active_realms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = realms.iter_mut().find(|r| r.id == realm.id) {
            slot.code_generation = realm.code_generation;
            slot.microtask_mode = realm.microtask_mode;
        }
    }

    // sandbox 即该 realm 的 globalThis：安装构造器 / eval / queueMicrotask 等
    if let Err(e) = install_realm_global_builtins(caller, sandbox) {
        return make_type_error_exception(
            caller,
            &format!("Error: vm.createContext failed to install globals: {e}"),
        );
    }

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

fn apply_codegen_options(
    caller: &mut Caller<'_, RuntimeState>,
    realm: &mut crate::realm::Realm,
    options: Option<i64>,
) {
    let Some(opts) = options else {
        return;
    };
    if !value::is_object(opts) {
        return;
    }
    let Some(ptr) = resolve_handle(caller, opts) else {
        return;
    };
    // Node: contextCodeGeneration 或 codeGeneration: { strings, wasm }
    let cg = read_object_property_by_name(caller, ptr, "contextCodeGeneration")
        .or_else(|| read_object_property_by_name(caller, ptr, "codeGeneration"));
    let Some(cg) = cg else {
        return;
    };
    if !value::is_object(cg) {
        return;
    }
    let Some(cg_ptr) = resolve_handle(caller, cg) else {
        return;
    };
    if let Some(raw) = read_object_property_by_name(caller, cg_ptr, "strings") {
        if value::is_bool(raw) {
            realm.code_generation.strings = value::decode_bool(raw);
        }
    }
    if let Some(raw) = read_object_property_by_name(caller, cg_ptr, "wasm") {
        if value::is_bool(raw) {
            realm.code_generation.wasm = value::decode_bool(raw);
        }
    }
}

fn apply_microtask_mode_option(
    caller: &mut Caller<'_, RuntimeState>,
    realm: &mut crate::realm::Realm,
    options: Option<i64>,
) {
    let Some(opts) = options else {
        return;
    };
    if !value::is_object(opts) {
        return;
    }
    let Some(ptr) = resolve_handle(caller, opts) else {
        return;
    };
    let Some(raw) = read_object_property_by_name(caller, ptr, "microtaskMode") else {
        return;
    };
    if !value::is_string(raw) {
        return;
    }
    let mode = js_string_lossy(caller, raw);
    if mode == "afterEvaluate" {
        realm.microtask_mode = MicrotaskMode::AfterEvaluate;
    }
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
        Some(s)
            if value::is_object(s)
                || value::is_array(s)
                || value::is_undefined(s)
                || value::is_null(s) =>
        {
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
    let timeout_ms = timeout_ms.or_else(|| parse_timeout_ms(caller, sandbox_arg));
    let realm_id = {
        let h = object_handle_idx(sandbox).unwrap_or(0);
        caller
            .data()
            .contextified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&h)
            .copied()
            .unwrap_or(RealmId(0))
    };
    eval_in_realm(caller, code_val, Some(sandbox), realm_id, timeout_ms).await
}

/// 将主 realm 全局上的内建装到 sandbox，并覆盖需门控的 `eval` / `Function`。
///
/// multi-realm free-var 解析依赖 sandbox 上的 Promise/Object/queueMicrotask 等；
/// 主 global 上的构造器带完整静态方法表（`Promise.resolve` / `Object.keys`）。
/// 执行期仍读 `execution_realm` 选 intrinsic（见 `new Array()` 等）。
fn install_realm_global_builtins(
    caller: &mut Caller<'_, RuntimeState>,
    sandbox: i64,
) -> Result<(), String> {
    if !(value::is_object(sandbox) || value::is_array(sandbox)) {
        return Err("sandbox is not an object".into());
    }

    let main_global = caller.data().js_global_object.load(Ordering::Relaxed);
    if value::is_object(main_global) || value::is_array(main_global) {
        if let Some(main_ptr) = resolve_handle(caller, main_global) {
            // 从主 global 拷贝标准内建（共享函数值；不拷贝用户属性）
            const NAMES: &[&str] = &[
                "Array",
                "Object",
                "String",
                "Boolean",
                "Number",
                "Symbol",
                "BigInt",
                "RegExp",
                "Error",
                "TypeError",
                "RangeError",
                "SyntaxError",
                "ReferenceError",
                "URIError",
                "EvalError",
                "AggregateError",
                "Map",
                "Set",
                "WeakMap",
                "WeakSet",
                "Promise",
                "Proxy",
                "Date",
                "ArrayBuffer",
                "DataView",
                "JSON",
                "Math",
                "Reflect",
                "console",
                "queueMicrotask",
                "setTimeout",
                "clearTimeout",
                "setInterval",
                "clearInterval",
            ];
            for name in NAMES {
                if let Some(val) = read_object_property_by_name(caller, main_ptr, name) {
                    if !value::is_undefined(val) {
                        let _ = define_host_data_property_from_caller(caller, sandbox, name, val);
                    }
                }
            }
        }
    }

    // Function：用可门控的构造器覆盖（codeGeneration.strings）
    let function_val = {
        let mut native_callables = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = native_callables.len() as u32;
        native_callables.push(NativeCallable::FunctionConstructor);
        value::encode_native_callable_idx(idx)
    };
    let _ = define_host_data_property_from_caller(caller, sandbox, "Function", function_val);

    // eval：间接 eval，受 codeGeneration.strings 门控
    let eval_val = {
        let mut native_callables = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idx = native_callables.len() as u32;
        native_callables.push(NativeCallable::EvalIndirect);
        value::encode_native_callable_idx(idx)
    };
    let _ = define_host_data_property_from_caller(caller, sandbox, "eval", eval_val);

    let _ = define_host_data_property_from_caller(caller, sandbox, "globalThis", sandbox);

    // 补齐 node web globals（若主 global 尚未带上）
    if let Some(ptr) = resolve_handle(caller, sandbox) {
        if read_object_property_by_name(caller, ptr, "queueMicrotask").is_none() {
            let _ =
                crate::runtime_node_globals::install_node_web_globals_from_caller(caller, sandbox);
        }
    }

    Ok(())
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

    // 注意：runIn* / Script 不受 codeGeneration.strings 限制；
    // strings:false 只拦截 context 内 eval / Function 构造器。
    let drain_after = realm_microtask_mode(caller, realm_id) == MicrotaskMode::AfterEvaluate;

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
    // 仅 microtaskMode === "afterEvaluate" 时在 run 边界 drain 到稳态
    if drain_after {
        if let Some(env) = WasmEnv::from_caller(caller) {
            let _ = drain_microtasks_after_eval(caller, &env).await;
        }
    }

    if value::is_exception(result) {
        // 解释器路径可能以 exception 抛出 timeout 文案
        return result;
    }
    result
}

fn realm_allows_strings(caller: &Caller<'_, RuntimeState>, realm_id: RealmId) -> bool {
    let realms = caller
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let r = if realm_id.0 == 0 {
        realms.first()
    } else {
        realms.iter().find(|r| r.id == realm_id)
    };
    r.map(|r| r.code_generation.strings).unwrap_or(true)
}

fn realm_microtask_mode(caller: &Caller<'_, RuntimeState>, realm_id: RealmId) -> MicrotaskMode {
    let realms = caller
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let r = if realm_id.0 == 0 {
        realms.first()
    } else {
        realms.iter().find(|r| r.id == realm_id)
    };
    r.map(|r| r.microtask_mode).unwrap_or_default()
}

/// 当前 execution_realm 是否允许从字符串生成代码（eval / Function）。
pub(crate) fn current_realm_allows_string_codegen(caller: &Caller<'_, RuntimeState>) -> bool {
    let rid = caller.data().execution_realm.load(Ordering::Relaxed);
    realm_allows_strings(caller, RealmId(rid))
}

async fn drain_microtasks_after_eval(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
) -> anyhow::Result<()> {
    // 稳态：排空整个 microtask 队列；上限防止死循环
    for _ in 0..10_000 {
        let len = caller
            .data()
            .microtask_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        if len == 0 {
            return Ok(());
        }
        crate::runtime_microtask::drain_microtasks_async(caller, env).await;
    }
    Ok(())
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
