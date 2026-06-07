use super::*;

/// 0=is_strict, 1=has_arguments, 2=home_object, 3=new_target
const META_IS_STRICT: u8 = 0;
const META_HAS_ARGUMENTS: u8 = 1;
const META_HOME_OBJECT: u8 = 2;
const META_NEW_TARGET: u8 = 3;

/// Host-allocated scope record implementing spec-like scope behavior.
#[derive(Clone)]
pub(crate) struct ScopeRecord {
    pub(crate) bindings: Vec<(String, i64, bool, bool)>,
    pub(crate) home_object: Option<i64>,
    pub(crate) new_target: Option<i64>,
    pub(crate) has_arguments_binding: bool,
    pub(crate) is_strict: bool,
}

pub(crate) fn try_compiled_eval_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    code: &str,
    module: &swc_ast::Module,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
) -> Result<i64> {
    let data_base = reserve_eval_data_segment(caller, code.len() as u32)?;
    let wasm_bytes = cached_eval_wasm(
        caller.data(),
        code,
        module,
        scope_env.is_some(),
        var_writes_to_scope,
        data_base,
    )?;
    let eval_module = Module::new(caller.engine(), &wasm_bytes)?;
    let mut imports = Vec::with_capacity(eval_module.imports().count());

    for import in eval_module.imports() {
        match import.ty() {
            ExternType::Func(func_ty) => {
                let func = compiled_eval_import(caller, import.name(), &func_ty);
                imports.push(func.into());
            }
            ExternType::Memory(_) => {
                let memory = caller
                    .get_export(import.name())
                    .and_then(Extern::into_memory)
                    .ok_or_else(|| anyhow::anyhow!("eval parent missing memory import"))?;
                imports.push(memory.into());
            }
            ExternType::Global(_) => {
                let global = caller
                    .get_export(import.name())
                    .and_then(Extern::into_global)
                    .ok_or_else(|| {
                        anyhow::anyhow!("eval parent missing global import `{}`", import.name())
                    })?;
                imports.push(global.into());
            }
            _ => {
                anyhow::bail!("unsupported eval import `{}`", import.name());
            }
        }
    }

    let instance = Instance::new(&mut *caller, &eval_module, &imports)?;
    let entry = instance.get_typed_func::<i64, i64>(&mut *caller, "__eval_entry")?;
    Ok(entry.call(
        &mut *caller,
        scope_env.unwrap_or_else(value::encode_undefined),
    )?)
}
/// Phase 3 must-convert 之 compiled eval 路径（按 2026-05-31-async-scheduler-implementation-plan.md 审计条目 + 26-async-audit-refactor-design.md）：
/// 为 `try_compiled_eval_from_caller`（eval 编译路径的 Instance::new + __eval_entry.call 点，perform_eval_from_caller 唯一 caller）添加 async 版本，与现有 sync `try_compiled_eval_from_caller` 并存。
///
/// 规则：
/// - 严格与 sync 版本并存，供保留的 sync execute 路径继续使用
/// - 所有 data segment reservation、import resolution（via compiled_eval_import）、scope handling 逻辑必须 100% 相同
/// - 仅 Wasm 实例化（Instance::new）和调用（entry.call）完全等价；唯一差异是将 `Instance::new(...)` 替换为 `Instance::new_async(...).await` ， `entry.call(...)` 替换为 `entry.call_async(...).await`
/// - 本阶段保持调用点不变（perform_eval_from_caller 仍调用 sync 版本；未来 async eval 路径激活时同步转换）
/// - 精确保留原有行为，无任何语义或顺序差异
///
/// 特别提醒（plan Correction 3 + lib.rs 已有注释 + 审计计划）：
///   在 Store::epoch_deadline_async_yield_and_update 之后，
///   *所有* 经由该 Store 的 Wasm 实例化与调用（主 + 回调，包括此处 compiled eval 中的 Instance::new + call）都必须走 async API（new_async / call_async 等）。
///   本文件中的 async 版本即为此准备；sync 版本仅留给未切换的 sync execute 路径。
pub(crate) async fn try_compiled_eval_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    code: &str,
    module: &swc_ast::Module,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
) -> Result<i64> {
    let data_base = reserve_eval_data_segment(caller, code.len() as u32)?;
    let wasm_bytes = cached_eval_wasm(
        caller.data(),
        code,
        module,
        scope_env.is_some(),
        var_writes_to_scope,
        data_base,
    )?;
    let eval_module = Module::new(caller.engine(), &wasm_bytes)?;
    let mut imports = Vec::with_capacity(eval_module.imports().count());

    for import in eval_module.imports() {
        match import.ty() {
            ExternType::Func(func_ty) => {
                let func = compiled_eval_import(caller, import.name(), &func_ty);
                imports.push(func.into());
            }
            ExternType::Memory(_) => {
                let memory = caller
                    .get_export(import.name())
                    .and_then(Extern::into_memory)
                    .ok_or_else(|| anyhow::anyhow!("eval parent missing memory import"))?;
                imports.push(memory.into());
            }
            ExternType::Global(_) => {
                let global = caller
                    .get_export(import.name())
                    .and_then(Extern::into_global)
                    .ok_or_else(|| {
                        anyhow::anyhow!("eval parent missing global import `{}`", import.name())
                    })?;
                imports.push(global.into());
            }
            _ => {
                anyhow::bail!("unsupported eval import `{}`", import.name());
            }
        }
    }

    let instance = Instance::new_async(&mut *caller, &eval_module, &imports).await?;
    let entry = instance.get_typed_func::<i64, i64>(&mut *caller, "__eval_entry")?;
    Ok(entry
        .call_async(
            &mut *caller,
            scope_env.unwrap_or_else(value::encode_undefined),
        )
        .await?)
}

pub(crate) fn reserve_eval_data_segment(
    caller: &mut Caller<'_, RuntimeState>,
    code_len: u32,
) -> Result<u32> {
    let heap_ptr = caller
        .get_export("__heap_ptr")
        .and_then(Extern::into_global)
        .ok_or_else(|| anyhow::anyhow!("eval parent missing heap pointer"))?;
    let current = match heap_ptr.get(&mut *caller) {
        Val::I32(value) => value as u32,
        other => anyhow::bail!("eval parent heap pointer has unexpected type {other:?}"),
    };
    let base = (current + 7) & !7;
    let reserve = (constants::USER_STRING_START + code_len + 4096 + 7) & !7;
    let need_end = base.saturating_add(reserve) as usize;
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| anyhow::anyhow!("eval parent missing memory"))?;
    let page_size = 65536usize;
    loop {
        let mem_len = memory.data_size(&*caller);
        if need_end <= mem_len {
            break;
        }
        let grow_pages = ((need_end - mem_len + page_size - 1) / page_size).max(1) as u64;
        memory
            .grow(&mut *caller, grow_pages)
            .map_err(|e| anyhow::anyhow!("eval memory grow failed: {e}"))?;
    }
    heap_ptr.set(&mut *caller, Val::I32((base + reserve) as i32))?;
    Ok(base)
}

pub(crate) fn cached_eval_wasm(
    state: &RuntimeState,
    code: &str,
    module: &swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
    data_base: u32,
) -> Result<Vec<u8>> {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    has_scope_bridge.hash(&mut hasher);
    var_writes_to_scope.hash(&mut hasher);
    data_base.hash(&mut hasher);
    const SCOPE_RECORD_CACHE_VERSION: u64 = 1;
    SCOPE_RECORD_CACHE_VERSION.hash(&mut hasher);
    let key = hasher.finish();

    if let Some(bytes) = state
        .eval_cache
        .lock()
        .expect("eval cache mutex")
        .get(&key)
        .cloned()
    {
        return Ok(bytes);
    }

    let program = wjsm_semantic::lower_eval_module_with_scope(
        module.clone(),
        has_scope_bridge,
        var_writes_to_scope,
    )?;
    let bytes = wjsm_backend_wasm::compile_eval_at_data_base(&program, data_base)?;
    state
        .eval_cache
        .lock()
        .expect("eval cache mutex")
        .insert(key, bytes.clone());
    Ok(bytes)
}

pub(crate) fn compiled_eval_import(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
    func_ty: &FuncType,
) -> Func {
    if let Some(func) = caller.get_export(name).and_then(Extern::into_func) {
        return func;
    }

    match name {
        "string_concat" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    if value::is_string(a) || value::is_string(b) {
                        let a_s = if value::is_string(a) {
                            read_value_string_bytes(&mut caller, a).unwrap_or_default()
                        } else {
                            render_value(&mut caller, a)
                                .unwrap_or_default()
                                .into_bytes()
                        };
                        let b_s = if value::is_string(b) {
                            read_value_string_bytes(&mut caller, b).unwrap_or_default()
                        } else {
                            render_value(&mut caller, b)
                                .unwrap_or_default()
                                .into_bytes()
                        };
                        let mut result = a_s;
                        result.extend(b_s);
                        let s = String::from_utf8(result).unwrap_or_default();
                        store_runtime_string(&caller, s)
                    } else {
                        value::encode_undefined()
                    }
                },
            );
        }
        "f64_mod" => {
            return Func::wrap(
                &mut *caller,
                |_: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    value::encode_f64(f64::from_bits(a as u64) % f64::from_bits(b as u64))
                },
            );
        }
        "f64_pow" => {
            return Func::wrap(
                &mut *caller,
                |_: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    value::encode_f64(f64::from_bits(a as u64).powf(f64::from_bits(b as u64)))
                },
            );
        }
        "create_unmapped_arguments_object" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64| -> i64 {
                    create_unmapped_arguments_object(&mut caller, args_array, param_count)
                },
            );
        }
        "create_mapped_arguments_object" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>,
                 args_array: i64,
                 param_count: i64,
                 func_ref: i64|
                 -> i64 {
                    create_mapped_arguments_object(&mut caller, args_array, param_count, func_ref)
                },
            );
        }
        _ => {}
    }

    let params: Vec<_> = func_ty.params().collect();
    let results: Vec<_> = func_ty.results().collect();
    let ty = FuncType::new(caller.engine(), params, results.clone());
    let name = name.to_string();
    Func::new(
        &mut *caller,
        ty,
        move |caller: Caller<'_, RuntimeState>, _params, values| {
            set_runtime_error(
                caller.data(),
                format!("RuntimeError: unsupported host import `{name}` called from compiled eval"),
            );
            for (slot, ty) in values.iter_mut().zip(results.iter()) {
                *slot = match ty {
                    ValType::I32 => Val::I32(0),
                    ValType::I64 => Val::I64(value::encode_handle(value::TAG_EXCEPTION, 0)),
                    ValType::F32 => Val::F32(0),
                    ValType::F64 => Val::F64(0),
                    ValType::V128 | ValType::Ref(_) => {
                        return Err(wasmtime::Error::msg(format!(
                            "unsupported compiled eval host result type {ty}"
                        )));
                    }
                };
            }
            Ok(())
        },
    )
}

pub(crate) fn perform_eval_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    code_value: i64,
    scope_env: Option<i64>,
) -> i64 {
    if !value::is_string(code_value) {
        return code_value;
    }

    let code = match read_value_string_bytes(caller, code_value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        Some(code) => code,
        None => return value::encode_undefined(),
    };
    if code.trim().is_empty() {
        return value::encode_undefined();
    }

    let eval_var_map = if scope_env.is_some() {
        read_eval_var_map(caller)
    } else {
        Vec::new()
    };
    let _eval_var_slots = eval_var_map
        .iter()
        .filter(|entry| {
            entry.offset % 8 == 0 && !entry.function_name.is_empty() && !entry.var_name.is_empty()
        })
        .count();
    // ── SyntaxError 检测：eval 代码中声明 var/function arguments ──
    // 检查规则（EvalDeclarationInstantiation）：
    // 如果 eval 代码声明 var arguments 且调用上下文没有 arguments 绑定 → SyntaxError
    // ── SyntaxError 检测：eval 代码中声明 var/function arguments ──
    // 检查规则（EvalDeclarationInstantiation）：
    // 如果调用上下文有 arguments 绑定且 eval 代码声明了某个同名绑定 → SyntaxError
    if let Some(env) = scope_env {
        let handle = value::decode_scope_record_handle(env);
        let has_arguments = caller
            .data()
            .scope_records
            .get(&handle)
            .map(|r| r.has_arguments_binding)
            .unwrap_or(false);
        if has_arguments {
            let binding_names = wjsm_semantic::eval_literal_binding_names(&code);
            if binding_names.iter().any(|n| n == "arguments") {
                let msg = "SyntaxError: declaring 'arguments' in eval code is invalid";
                set_runtime_error(caller.data(), msg.to_string());
                return value::encode_undefined();
            }
        }
    }

    let module = match wjsm_parser::parse_script_as_module(&code) {
        Ok(module) => module,
        Err(error) => {
            set_runtime_error(caller.data(), format!("SyntaxError: {error}"));
            return value::encode_undefined();
        }
    };
    let strict_eval_source = runtime_module_has_use_strict_directive(&module);
    let var_writes_to_scope = scope_env
        .map(|env| !strict_eval_source && !eval_scope_has_strict_marker(caller, env))
        .unwrap_or(false);

    // ── 非可定义函数检查（CanDeclareGlobalFunction）──
    // eval 代码中声明 function NaN/Infinity/undefined → TypeError
    for item in &module.body {
        if let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl))) = item {
            let name = fn_decl.ident.sym.as_ref();
            if matches!(name, "NaN" | "Infinity" | "undefined") {
                let msg = format!("Cannot define function '{}' in eval", fn_decl.ident.sym);
                let msg_val = store_runtime_string(caller, msg.clone());
                let error_obj = create_error_object(caller, "TypeError", msg_val);
                {
                    let mut errors = caller.data().error_table.lock().expect("error table mutex");
                    let idx = errors.len() as u32;
                    // create_error_object 已 push 了第一项（value=undefined），
                    // 我们需要再 push 一项（value=错误对象），以便 ExceptionValue 能恢复
                    errors.push(crate::ErrorEntry {
                        name: "TypeError".to_string(),
                        message: msg,
                        value: error_obj,
                    });
                    return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                }
            }
        }
    }

    let output_len = caller
        .data()
        .output
        .lock()
        .expect("runtime output buffer mutex")
        .len();
    let previous_runtime_error = caller
        .data()
        .runtime_error
        .lock()
        .expect("runtime_error mutex")
        .clone();
    let previous_error_count = caller.data().error_table.lock().unwrap().len();

    match try_compiled_eval_from_caller(caller, &code, &module, scope_env, var_writes_to_scope) {
        Ok(value) => value,
        Err(error) => {
            let thrown_exception = {
                let errors = caller.data().error_table.lock().unwrap();
                if errors.len() > previous_error_count {
                    Some((errors.len() - 1) as u32)
                } else {
                    None
                }
            };

            if let Some(idx) = thrown_exception {
                caller
                    .data()
                    .output
                    .lock()
                    .expect("runtime output buffer mutex")
                    .truncate(output_len);
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime_error mutex") = previous_runtime_error;
                return value::encode_handle(value::TAG_EXCEPTION, idx);
            }

            set_runtime_error(caller.data(), format_eval_error(error));
            value::encode_undefined()
        }
    }
}

pub(crate) async fn perform_eval_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    code_value: i64,
    scope_env: Option<i64>,
) -> i64 {
    if !value::is_string(code_value) {
        return code_value;
    }

    let code = match read_value_string_bytes(caller, code_value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        Some(code) => code,
        None => return value::encode_undefined(),
    };
    if code.trim().is_empty() {
        return value::encode_undefined();
    }

    let eval_var_map = if scope_env.is_some() {
        read_eval_var_map(caller)
    } else {
        Vec::new()
    };
    let _eval_var_slots = eval_var_map
        .iter()
        .filter(|entry| {
            entry.offset % 8 == 0 && !entry.function_name.is_empty() && !entry.var_name.is_empty()
        })
        .count();
    if let Some(env) = scope_env {
        let handle = value::decode_scope_record_handle(env);
        let has_arguments = caller
            .data()
            .scope_records
            .get(&handle)
            .map(|r| r.has_arguments_binding)
            .unwrap_or(false);
        if has_arguments {
            let binding_names = wjsm_semantic::eval_literal_binding_names(&code);
            if binding_names.iter().any(|n| n == "arguments") {
                let msg = "SyntaxError: declaring 'arguments' in eval code is invalid";
                set_runtime_error(caller.data(), msg.to_string());
                return value::encode_undefined();
            }
        }
    }

    let module = match wjsm_parser::parse_script_as_module(&code) {
        Ok(module) => module,
        Err(error) => {
            set_runtime_error(caller.data(), format!("SyntaxError: {error}"));
            return value::encode_undefined();
        }
    };
    let strict_eval_source = runtime_module_has_use_strict_directive(&module);
    let var_writes_to_scope = scope_env
        .map(|env| !strict_eval_source && !eval_scope_has_strict_marker(caller, env))
        .unwrap_or(false);

    for item in &module.body {
        if let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl))) = item {
            let name = fn_decl.ident.sym.as_ref();
            if matches!(name, "NaN" | "Infinity" | "undefined") {
                let msg = format!("Cannot define function '{}' in eval", fn_decl.ident.sym);
                let msg_val = store_runtime_string(caller, msg.clone());
                let error_obj = create_error_object(caller, "TypeError", msg_val);
                {
                    let mut errors = caller.data().error_table.lock().expect("error table mutex");
                    let idx = errors.len() as u32;
                    errors.push(crate::ErrorEntry {
                        name: "TypeError".to_string(),
                        message: msg,
                        value: error_obj,
                    });
                    return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                }
            }
        }
    }

    let output_len = caller
        .data()
        .output
        .lock()
        .expect("runtime output buffer mutex")
        .len();
    let previous_runtime_error = caller
        .data()
        .runtime_error
        .lock()
        .expect("runtime_error mutex")
        .clone();
    let previous_error_count = caller.data().error_table.lock().unwrap().len();

    match try_compiled_eval_from_caller_async(
        caller,
        &code,
        &module,
        scope_env,
        var_writes_to_scope,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            let thrown_exception = {
                let errors = caller.data().error_table.lock().unwrap();
                if errors.len() > previous_error_count {
                    Some((errors.len() - 1) as u32)
                } else {
                    None
                }
            };

            if let Some(idx) = thrown_exception {
                caller
                    .data()
                    .output
                    .lock()
                    .expect("runtime output buffer mutex")
                    .truncate(output_len);
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime_error mutex") = previous_runtime_error;
                return value::encode_handle(value::TAG_EXCEPTION, idx);
            }

            set_runtime_error(caller.data(), format_eval_error(error));
            value::encode_undefined()
        }
    }
}

pub(crate) fn format_eval_error(error: anyhow::Error) -> String {
    let raw = error.to_string();
    let message = raw
        .split_once(": ")
        .and_then(|(prefix, message)| {
            prefix
                .starts_with("semantic lowering error [")
                .then_some(message)
        })
        .unwrap_or(raw.as_str());

    if message.starts_with("cannot reassign a const-declared variable") {
        let name = message
            .split_once('`')
            .and_then(|(_, rest)| rest.split_once('`'))
            .map(|(name, _)| name)
            .unwrap_or("unknown");
        format!("TypeError: assignment to constant `{name}`")
    } else if message.starts_with("assignment to constant") {
        format!("TypeError: {message}")
    } else if message.starts_with("cannot redeclare identifier") {
        let normalized = message.replace(" in the same scope", " in eval");
        format!("SyntaxError: {normalized}")
    } else if message.starts_with("const declarations must be initialised") {
        format!("SyntaxError: {message}")
    } else if message.starts_with("cannot access") || message.starts_with("undeclared identifier") {
        format!("ReferenceError: {message}")
    } else {
        format!("RuntimeError: {raw}")
    }
}

pub(crate) fn runtime_module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
    for item in &module.body {
        let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Expr(expr_stmt)) = item else {
            return false;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            return false;
        };
        if string.value.as_str() == Some("use strict") {
            return true;
        }
    }
    false
}

#[allow(dead_code)]
pub(crate) fn eval_module_items(
    caller: &mut Caller<'_, RuntimeState>,
    items: &[swc_ast::ModuleItem],
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    let mut completion = None;
    for item in items {
        match item {
            swc_ast::ModuleItem::Stmt(stmt) => {
                if let Some(value) =
                    eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)?
                {
                    completion = Some(value);
                }
            }
            swc_ast::ModuleItem::ModuleDecl(_) => {
                return Err(
                    "SyntaxError: import/export declarations are not valid in eval".to_string(),
                );
            }
        }
    }
    Ok(completion)
}

pub(crate) fn eval_stmt(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Empty(_) => Ok(None),
        swc_ast::Stmt::Expr(expr) => {
            Ok(Some(eval_expr(caller, &expr.expr, scope_env, eval_locals)?))
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Var(var_decl)) => {
            for declarator in &var_decl.decls {
                let Some(name) = pat_ident_name(&declarator.name) else {
                    return Err("SyntaxError: unsupported eval declaration pattern".to_string());
                };
                let value = if let Some(init) = &declarator.init {
                    eval_expr(caller, init, scope_env, eval_locals)?
                } else {
                    value::encode_undefined()
                };
                match var_decl.kind {
                    swc_ast::VarDeclKind::Var if var_writes_to_scope => {
                        if eval_locals
                            .get(name)
                            .is_some_and(|binding| !matches!(binding.kind, EvalLocalKind::Var))
                        {
                            return Err(format!(
                                "SyntaxError: cannot redeclare identifier `{name}` in eval"
                            ));
                        }
                        eval_write_binding(caller, scope_env, eval_locals, name, value)?;
                    }
                    swc_ast::VarDeclKind::Var => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
                    }
                    swc_ast::VarDeclKind::Let => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Let, value)?;
                    }
                    swc_ast::VarDeclKind::Const => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Const, value)?;
                    }
                }
            }
            Ok(None)
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl)) => {
            let function = eval_function_from_decl(fn_decl, scope_env)?;
            let value = create_eval_function(caller.data(), function);
            let name = fn_decl.ident.sym.as_ref();
            if var_writes_to_scope {
                eval_write_binding(caller, scope_env, eval_locals, name, value)?;
            } else {
                eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
            }
            Ok(None)
        }
        swc_ast::Stmt::Block(block) => eval_block(
            caller,
            &block.stmts,
            scope_env,
            var_writes_to_scope,
            eval_locals,
        ),
        swc_ast::Stmt::If(if_stmt) => {
            let test = eval_expr(caller, &if_stmt.test, scope_env, eval_locals)?;
            if !value::is_falsy(test) {
                eval_stmt(
                    caller,
                    &if_stmt.cons,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                )
            } else if let Some(alt) = &if_stmt.alt {
                eval_stmt(caller, alt, scope_env, var_writes_to_scope, eval_locals)
            } else {
                Ok(None)
            }
        }
        swc_ast::Stmt::Throw(throw_stmt) => {
            let value = eval_expr(caller, &throw_stmt.arg, scope_env, eval_locals)?;
            let rendered = render_value(caller, value).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            Err(format!("Uncaught exception: {rendered}"))
        }
        _ => Err("SyntaxError: unsupported eval statement".to_string()),
    }
}

pub(crate) async fn eval_stmt_async(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Empty(_) => Ok(None),
        swc_ast::Stmt::Expr(expr) => Ok(Some(
            eval_expr_async(caller, &expr.expr, scope_env, eval_locals).await?,
        )),
        swc_ast::Stmt::Decl(swc_ast::Decl::Var(var_decl)) => {
            for declarator in &var_decl.decls {
                let Some(name) = pat_ident_name(&declarator.name) else {
                    return Err("SyntaxError: unsupported eval declaration pattern".to_string());
                };
                let value = if let Some(init) = &declarator.init {
                    eval_expr_async(caller, init, scope_env, eval_locals).await?
                } else {
                    value::encode_undefined()
                };
                match var_decl.kind {
                    swc_ast::VarDeclKind::Var if var_writes_to_scope => {
                        if eval_locals
                            .get(name)
                            .is_some_and(|binding| !matches!(binding.kind, EvalLocalKind::Var))
                        {
                            return Err(format!(
                                "SyntaxError: cannot redeclare identifier `{name}` in eval"
                            ));
                        }
                        eval_write_binding(caller, scope_env, eval_locals, name, value)?;
                    }
                    swc_ast::VarDeclKind::Var => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
                    }
                    swc_ast::VarDeclKind::Let => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Let, value)?;
                    }
                    swc_ast::VarDeclKind::Const => {
                        eval_declare_local(eval_locals, name, EvalLocalKind::Const, value)?;
                    }
                }
            }
            Ok(None)
        }
        swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl)) => {
            let function = eval_function_from_decl(fn_decl, scope_env)?;
            let value = create_eval_function(caller.data(), function);
            let name = fn_decl.ident.sym.as_ref();
            if var_writes_to_scope {
                eval_write_binding(caller, scope_env, eval_locals, name, value)?;
            } else {
                eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?;
            }
            Ok(None)
        }
        swc_ast::Stmt::Block(block) => {
            Box::pin(eval_block_async(
                caller,
                &block.stmts,
                scope_env,
                var_writes_to_scope,
                eval_locals,
            ))
            .await
        }
        swc_ast::Stmt::If(if_stmt) => {
            let test = Box::pin(eval_expr_async(
                caller,
                &if_stmt.test,
                scope_env,
                eval_locals,
            ))
            .await?;
            if !value::is_falsy(test) {
                Box::pin(eval_stmt_async(
                    caller,
                    &if_stmt.cons,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                ))
                .await
            } else if let Some(alt) = &if_stmt.alt {
                Box::pin(eval_stmt_async(
                    caller,
                    alt,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                ))
                .await
            } else {
                Ok(None)
            }
        }
        swc_ast::Stmt::Throw(throw_stmt) => {
            let value = eval_expr_async(caller, &throw_stmt.arg, scope_env, eval_locals).await?;
            let rendered = render_value(caller, value).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            Err(format!("Uncaught exception: {rendered}"))
        }
        _ => Err("SyntaxError: unsupported eval statement".to_string()),
    }
}

pub(crate) fn eval_block(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    let mut completion = None;
    for stmt in stmts {
        if let Some(value) = eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)? {
            completion = Some(value);
        }
    }
    Ok(completion)
}

pub(crate) async fn eval_block_async(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    let mut completion = None;
    for stmt in stmts {
        if let Some(value) =
            eval_stmt_async(caller, stmt, scope_env, var_writes_to_scope, eval_locals).await?
        {
            completion = Some(value);
        }
    }
    Ok(completion)
}

pub(crate) fn eval_expr(
    caller: &mut Caller<'_, RuntimeState>,
    expr: &swc_ast::Expr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    match expr {
        swc_ast::Expr::Lit(lit) => eval_lit(caller, lit),
        swc_ast::Expr::Ident(ident) => {
            Ok(
                eval_read_binding(caller, scope_env, eval_locals, ident.sym.as_ref())
                    .unwrap_or_else(value::encode_undefined),
            )
        }
        swc_ast::Expr::Paren(paren) => eval_expr(caller, &paren.expr, scope_env, eval_locals),
        swc_ast::Expr::Seq(seq) => {
            let mut result = value::encode_undefined();
            for expr in &seq.exprs {
                result = eval_expr(caller, expr, scope_env, eval_locals)?;
            }
            Ok(result)
        }
        swc_ast::Expr::Bin(bin) => {
            if matches!(
                bin.op,
                swc_ast::BinaryOp::LogicalAnd
                    | swc_ast::BinaryOp::LogicalOr
                    | swc_ast::BinaryOp::NullishCoalescing
            ) {
                return eval_logical(caller, bin, scope_env, eval_locals);
            }
            let lhs = eval_expr(caller, &bin.left, scope_env, eval_locals)?;
            let rhs = eval_expr(caller, &bin.right, scope_env, eval_locals)?;
            eval_binary(caller, bin.op, lhs, rhs)
        }
        swc_ast::Expr::Unary(unary) => {
            let val = eval_expr(caller, &unary.arg, scope_env, eval_locals)?;
            eval_unary(unary.op, val)
        }
        swc_ast::Expr::Cond(cond) => {
            let test = eval_expr(caller, &cond.test, scope_env, eval_locals)?;
            if value::is_falsy(test) {
                eval_expr(caller, &cond.alt, scope_env, eval_locals)
            } else {
                eval_expr(caller, &cond.cons, scope_env, eval_locals)
            }
        }
        swc_ast::Expr::Assign(assign) => eval_assign(caller, assign, scope_env, eval_locals),
        swc_ast::Expr::Call(call) => eval_call(caller, call, scope_env, eval_locals),
        _ => Err("SyntaxError: unsupported eval expression".to_string()),
    }
}

pub(crate) async fn eval_expr_async(
    caller: &mut Caller<'_, RuntimeState>,
    expr: &swc_ast::Expr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    match expr {
        swc_ast::Expr::Lit(lit) => eval_lit(caller, lit),
        swc_ast::Expr::Ident(ident) => {
            Ok(
                eval_read_binding(caller, scope_env, eval_locals, ident.sym.as_ref())
                    .unwrap_or_else(value::encode_undefined),
            )
        }
        swc_ast::Expr::Paren(paren) => {
            Box::pin(eval_expr_async(caller, &paren.expr, scope_env, eval_locals)).await
        }
        swc_ast::Expr::Seq(seq) => {
            let mut result = value::encode_undefined();
            for expr in &seq.exprs {
                result = Box::pin(eval_expr_async(caller, expr, scope_env, eval_locals)).await?;
            }
            Ok(result)
        }
        swc_ast::Expr::Bin(bin) => {
            if matches!(
                bin.op,
                swc_ast::BinaryOp::LogicalAnd
                    | swc_ast::BinaryOp::LogicalOr
                    | swc_ast::BinaryOp::NullishCoalescing
            ) {
                return Box::pin(eval_logical_async(caller, bin, scope_env, eval_locals)).await;
            }
            let lhs = Box::pin(eval_expr_async(caller, &bin.left, scope_env, eval_locals)).await?;
            let rhs = Box::pin(eval_expr_async(caller, &bin.right, scope_env, eval_locals)).await?;
            eval_binary(caller, bin.op, lhs, rhs)
        }
        swc_ast::Expr::Unary(unary) => {
            let val = Box::pin(eval_expr_async(caller, &unary.arg, scope_env, eval_locals)).await?;
            eval_unary(unary.op, val)
        }
        swc_ast::Expr::Cond(cond) => {
            let test =
                Box::pin(eval_expr_async(caller, &cond.test, scope_env, eval_locals)).await?;
            if value::is_falsy(test) {
                Box::pin(eval_expr_async(caller, &cond.alt, scope_env, eval_locals)).await
            } else {
                Box::pin(eval_expr_async(caller, &cond.cons, scope_env, eval_locals)).await
            }
        }
        swc_ast::Expr::Assign(assign) => {
            Box::pin(eval_assign_async(caller, assign, scope_env, eval_locals)).await
        }
        swc_ast::Expr::Call(call) => {
            Box::pin(eval_call_async(caller, call, scope_env, eval_locals)).await
        }
        _ => Err("SyntaxError: unsupported eval expression".to_string()),
    }
}

pub(crate) fn eval_lit(
    caller: &Caller<'_, RuntimeState>,
    lit: &swc_ast::Lit,
) -> Result<i64, String> {
    match lit {
        swc_ast::Lit::Str(string) => Ok(store_runtime_string(
            caller,
            string.value.to_string_lossy().into_owned(),
        )),
        swc_ast::Lit::Num(number) => Ok(value::encode_f64(number.value)),
        swc_ast::Lit::Bool(boolean) => Ok(value::encode_bool(boolean.value)),
        swc_ast::Lit::Null(_) => Ok(value::encode_null()),
        _ => Err("SyntaxError: unsupported eval literal".to_string()),
    }
}

pub(crate) fn eval_binary(
    caller: &mut Caller<'_, RuntimeState>,
    op: swc_ast::BinaryOp,
    lhs: i64,
    rhs: i64,
) -> Result<i64, String> {
    if matches!(op, swc_ast::BinaryOp::Add) && (value::is_string(lhs) || value::is_string(rhs)) {
        let lhs_string = eval_to_string(caller, lhs);
        let rhs_string = eval_to_string(caller, rhs);
        return Ok(store_runtime_string(
            caller,
            format!("{lhs_string}{rhs_string}"),
        ));
    }

    let a = eval_to_number(lhs);
    let b = eval_to_number(rhs);
    let result = match op {
        swc_ast::BinaryOp::Add => a + b,
        swc_ast::BinaryOp::Sub => a - b,
        swc_ast::BinaryOp::Mul => a * b,
        swc_ast::BinaryOp::Div => a / b,
        swc_ast::BinaryOp::Mod => a - b * (a / b).trunc(),
        swc_ast::BinaryOp::EqEq => return Ok(value::encode_bool(a == b)),
        swc_ast::BinaryOp::NotEq => return Ok(value::encode_bool(a != b)),
        swc_ast::BinaryOp::Lt => return Ok(value::encode_bool(a < b)),
        swc_ast::BinaryOp::LtEq => return Ok(value::encode_bool(a <= b)),
        swc_ast::BinaryOp::Gt => return Ok(value::encode_bool(a > b)),
        swc_ast::BinaryOp::GtEq => return Ok(value::encode_bool(a >= b)),
        _ => return Err("SyntaxError: unsupported eval binary operator".to_string()),
    };
    Ok(value::encode_f64(result))
}

pub(crate) fn eval_logical(
    caller: &mut Caller<'_, RuntimeState>,
    bin: &swc_ast::BinExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let left = eval_expr(caller, &bin.left, scope_env, eval_locals)?;
    match bin.op {
        swc_ast::BinaryOp::LogicalAnd if value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalAnd => eval_expr(caller, &bin.right, scope_env, eval_locals),
        swc_ast::BinaryOp::LogicalOr if !value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalOr => eval_expr(caller, &bin.right, scope_env, eval_locals),
        swc_ast::BinaryOp::NullishCoalescing
            if value::is_null(left) || value::is_undefined(left) =>
        {
            eval_expr(caller, &bin.right, scope_env, eval_locals)
        }
        swc_ast::BinaryOp::NullishCoalescing => Ok(left),
        _ => Err("SyntaxError: unsupported eval logical operator".to_string()),
    }
}

pub(crate) async fn eval_logical_async(
    caller: &mut Caller<'_, RuntimeState>,
    bin: &swc_ast::BinExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let left = eval_expr_async(caller, &bin.left, scope_env, eval_locals).await?;
    match bin.op {
        swc_ast::BinaryOp::LogicalAnd if value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalAnd => {
            eval_expr_async(caller, &bin.right, scope_env, eval_locals).await
        }
        swc_ast::BinaryOp::LogicalOr if !value::is_falsy(left) => Ok(left),
        swc_ast::BinaryOp::LogicalOr => {
            eval_expr_async(caller, &bin.right, scope_env, eval_locals).await
        }
        swc_ast::BinaryOp::NullishCoalescing
            if value::is_null(left) || value::is_undefined(left) =>
        {
            eval_expr_async(caller, &bin.right, scope_env, eval_locals).await
        }
        swc_ast::BinaryOp::NullishCoalescing => Ok(left),
        _ => Err("SyntaxError: unsupported eval logical operator".to_string()),
    }
}

pub(crate) fn eval_unary(op: swc_ast::UnaryOp, val: i64) -> Result<i64, String> {
    match op {
        swc_ast::UnaryOp::Minus => Ok(value::encode_f64(-eval_to_number(val))),
        swc_ast::UnaryOp::Plus => Ok(value::encode_f64(eval_to_number(val))),
        swc_ast::UnaryOp::Bang => Ok(value::encode_bool(value::is_falsy(val))),
        swc_ast::UnaryOp::Void => Ok(value::encode_undefined()),
        _ => Err("SyntaxError: unsupported eval unary operator".to_string()),
    }
}

pub(crate) fn eval_assign(
    caller: &mut Caller<'_, RuntimeState>,
    assign: &swc_ast::AssignExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let val = eval_expr(caller, &assign.right, scope_env, eval_locals)?;
    let swc_ast::AssignTarget::Simple(simple) = &assign.left else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    let swc_ast::SimpleAssignTarget::Ident(ident) = simple else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    eval_write_binding(caller, scope_env, eval_locals, ident.id.sym.as_ref(), val)?;
    Ok(val)
}

pub(crate) async fn eval_assign_async(
    caller: &mut Caller<'_, RuntimeState>,
    assign: &swc_ast::AssignExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let val = eval_expr_async(caller, &assign.right, scope_env, eval_locals).await?;
    let swc_ast::AssignTarget::Simple(simple) = &assign.left else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    let swc_ast::SimpleAssignTarget::Ident(ident) = simple else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    eval_write_binding(caller, scope_env, eval_locals, ident.id.sym.as_ref(), val)?;
    Ok(val)
}

pub(crate) fn eval_call(
    caller: &mut Caller<'_, RuntimeState>,
    call: &swc_ast::CallExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    if let swc_ast::Callee::Expr(callee) = &call.callee {
        if let swc_ast::Expr::Ident(ident) = callee.as_ref()
            && ident.sym.as_ref() == "eval"
        {
            let arg = if let Some(first) = call.args.first() {
                eval_expr(caller, &first.expr, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            return Ok(perform_eval_from_caller(caller, arg, scope_env));
        }
        if let swc_ast::Expr::Member(member) = callee.as_ref()
            && let swc_ast::Expr::Ident(obj) = member.obj.as_ref()
            && obj.sym.as_ref() == "console"
            && let swc_ast::MemberProp::Ident(prop) = &member.prop
            && prop.sym.as_ref() == "log"
        {
            let arg = if let Some(first) = call.args.first() {
                eval_expr(caller, &first.expr, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            write_console_value(caller, arg, None);
            return Ok(value::encode_undefined());
        }
    }
    let swc_ast::Callee::Expr(callee_expr) = &call.callee else {
        return Err("SyntaxError: unsupported eval call".to_string());
    };
    let callee = eval_expr(caller, callee_expr.as_ref(), scope_env, eval_locals)?;
    if value::is_native_callable(callee) {
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            args.push(eval_expr(caller, &arg.expr, scope_env, eval_locals)?);
        }
        return call_native_callable_with_args_from_caller(
            caller,
            callee,
            value::encode_undefined(),
            args,
        )
        .ok_or_else(|| "TypeError: eval callee is not callable".to_string());
    }
    Err("SyntaxError: unsupported eval call".to_string())
}

pub(crate) async fn eval_call_async(
    caller: &mut Caller<'_, RuntimeState>,
    call: &swc_ast::CallExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    if let swc_ast::Callee::Expr(callee) = &call.callee {
        if let swc_ast::Expr::Ident(ident) = callee.as_ref()
            && ident.sym.as_ref() == "eval"
        {
            let arg = if let Some(first) = call.args.first() {
                eval_expr_async(caller, &first.expr, scope_env, eval_locals).await?
            } else {
                value::encode_undefined()
            };
            return Ok(perform_eval_from_caller_async(caller, arg, scope_env).await);
        }
        if let swc_ast::Expr::Member(member) = callee.as_ref()
            && let swc_ast::Expr::Ident(obj) = member.obj.as_ref()
            && obj.sym.as_ref() == "console"
            && let swc_ast::MemberProp::Ident(prop) = &member.prop
            && prop.sym.as_ref() == "log"
        {
            let arg = if let Some(first) = call.args.first() {
                eval_expr_async(caller, &first.expr, scope_env, eval_locals).await?
            } else {
                value::encode_undefined()
            };
            write_console_value(caller, arg, None);
            return Ok(value::encode_undefined());
        }
    }
    let swc_ast::Callee::Expr(callee_expr) = &call.callee else {
        return Err("SyntaxError: unsupported eval call".to_string());
    };
    let callee = eval_expr_async(caller, callee_expr.as_ref(), scope_env, eval_locals).await?;
    if value::is_native_callable(callee) {
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            args.push(eval_expr_async(caller, &arg.expr, scope_env, eval_locals).await?);
        }
        return call_native_callable_with_args_from_caller_async(
            caller,
            callee,
            value::encode_undefined(),
            args,
        )
        .await
        .ok_or_else(|| "TypeError: eval callee is not callable".to_string());
    }
    Err("SyntaxError: unsupported eval call".to_string())
}

pub(crate) fn pat_ident_name(pat: &swc_ast::Pat) -> Option<&str> {
    match pat {
        swc_ast::Pat::Ident(ident) => Some(ident.id.sym.as_ref()),
        _ => None,
    }
}

pub(crate) fn eval_scope_has_strict_marker(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: i64,
) -> bool {
    let Some(ptr) = resolve_handle(caller, scope_env) else {
        return false;
    };
    read_object_property_by_name(caller, ptr, "__wjsm_eval_strict")
        .map(nanbox_to_bool)
        .unwrap_or(false)
}

pub(crate) fn eval_read_binding(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
    eval_locals: &HashMap<String, EvalLocalBinding>,
    name: &str,
) -> Option<i64> {
    if let Some(binding) = eval_locals.get(name) {
        return Some(binding.value);
    }
    match name {
        "undefined" => return Some(value::encode_undefined()),
        "NaN" => return Some(value::encode_f64(f64::NAN)),
        "Infinity" => return Some(value::encode_f64(f64::INFINITY)),
        _ => {}
    }
    let env = scope_env?;
    let ptr = resolve_handle(caller, env)?;
    read_object_property_by_name(caller, ptr, name)
}

pub(crate) fn eval_write_binding(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
    name: &str,
    val: i64,
) -> Result<(), String> {
    if let Some(binding) = eval_locals.get_mut(name) {
        if matches!(binding.kind, EvalLocalKind::Const) {
            return Err(format!("TypeError: assignment to constant `{name}`"));
        }
        binding.value = val;
        return Ok(());
    }
    let Some(env) = scope_env else {
        return Ok(());
    };
    let _ = set_host_data_property_from_caller(caller, env, name, val);
    Ok(())
}

pub(crate) fn eval_function_from_decl(
    fn_decl: &swc_ast::FnDecl,
    scope_env: Option<i64>,
) -> Result<EvalFunction, String> {
    let mut params = Vec::with_capacity(fn_decl.function.params.len());
    for param in &fn_decl.function.params {
        let Some(name) = pat_ident_name(&param.pat) else {
            return Err("SyntaxError: unsupported eval function parameter".to_string());
        };
        params.push(name.to_string());
    }
    let Some(body) = &fn_decl.function.body else {
        return Err("SyntaxError: eval function body is missing".to_string());
    };
    Ok(EvalFunction {
        params,
        body: body.stmts.clone(),
        scope_env,
    })
}

pub(crate) fn create_eval_function(state: &RuntimeState, function: EvalFunction) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::EvalFunction(function));
    value::encode_native_callable_idx(handle)
}

pub(crate) fn call_eval_function_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    function: EvalFunction,
    args: Vec<i64>,
) -> i64 {
    match eval_call_function(caller, &function, args) {
        Ok(value) => value,
        Err(message) => {
            set_runtime_error(caller.data(), message);
            value::encode_handle(value::TAG_EXCEPTION, 0)
        }
    }
}

pub(crate) async fn call_eval_function_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    function: EvalFunction,
    args: Vec<i64>,
) -> i64 {
    match eval_call_function_async(caller, &function, args).await {
        Ok(value) => value,
        Err(message) => {
            set_runtime_error(caller.data(), message);
            value::encode_handle(value::TAG_EXCEPTION, 0)
        }
    }
}

pub(crate) fn eval_call_function(
    caller: &mut Caller<'_, RuntimeState>,
    function: &EvalFunction,
    args: Vec<i64>,
) -> Result<i64, String> {
    let mut locals = HashMap::new();
    for (index, param) in function.params.iter().enumerate() {
        let value = args
            .get(index)
            .copied()
            .unwrap_or_else(value::encode_undefined);
        eval_declare_local(&mut locals, param, EvalLocalKind::Var, value)?;
    }
    eval_function_block(caller, &function.body, function.scope_env, &mut locals)
        .map(|value| value.unwrap_or_else(value::encode_undefined))
}

pub(crate) async fn eval_call_function_async(
    caller: &mut Caller<'_, RuntimeState>,
    function: &EvalFunction,
    args: Vec<i64>,
) -> Result<i64, String> {
    let mut locals = HashMap::new();
    for (index, param) in function.params.iter().enumerate() {
        let value = args
            .get(index)
            .copied()
            .unwrap_or_else(value::encode_undefined);
        eval_declare_local(&mut locals, param, EvalLocalKind::Var, value)?;
    }
    eval_function_block_async(caller, &function.body, function.scope_env, &mut locals)
        .await
        .map(|value| value.unwrap_or_else(value::encode_undefined))
}

pub(crate) fn eval_function_block(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    for stmt in stmts {
        if let Some(value) = eval_function_stmt(caller, stmt, scope_env, eval_locals)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) async fn eval_function_block_async(
    caller: &mut Caller<'_, RuntimeState>,
    stmts: &[swc_ast::Stmt],
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    for stmt in stmts {
        if let Some(value) = eval_function_stmt_async(caller, stmt, scope_env, eval_locals).await? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) fn eval_function_stmt(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Return(return_stmt) => {
            let value = if let Some(arg) = &return_stmt.arg {
                eval_expr(caller, arg, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            Ok(Some(value))
        }
        swc_ast::Stmt::Block(block) => {
            eval_function_block(caller, &block.stmts, scope_env, eval_locals)
        }
        swc_ast::Stmt::If(if_stmt) => {
            let test = eval_expr(caller, &if_stmt.test, scope_env, eval_locals)?;
            if !value::is_falsy(test) {
                eval_function_stmt(caller, &if_stmt.cons, scope_env, eval_locals)
            } else if let Some(alt) = &if_stmt.alt {
                eval_function_stmt(caller, alt, scope_env, eval_locals)
            } else {
                Ok(None)
            }
        }
        _ => {
            let _ = eval_stmt(caller, stmt, scope_env, false, eval_locals)?;
            Ok(None)
        }
    }
}

pub(crate) async fn eval_function_stmt_async(
    caller: &mut Caller<'_, RuntimeState>,
    stmt: &swc_ast::Stmt,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<Option<i64>, String> {
    match stmt {
        swc_ast::Stmt::Return(return_stmt) => {
            let value = if let Some(arg) = &return_stmt.arg {
                eval_expr_async(caller, arg, scope_env, eval_locals).await?
            } else {
                value::encode_undefined()
            };
            Ok(Some(value))
        }
        swc_ast::Stmt::Block(block) => {
            Box::pin(eval_function_block_async(
                caller,
                &block.stmts,
                scope_env,
                eval_locals,
            ))
            .await
        }
        swc_ast::Stmt::If(if_stmt) => {
            let test = eval_expr_async(caller, &if_stmt.test, scope_env, eval_locals).await?;
            if !value::is_falsy(test) {
                Box::pin(eval_function_stmt_async(
                    caller,
                    &if_stmt.cons,
                    scope_env,
                    eval_locals,
                ))
                .await
            } else if let Some(alt) = &if_stmt.alt {
                Box::pin(eval_function_stmt_async(
                    caller,
                    alt,
                    scope_env,
                    eval_locals,
                ))
                .await
            } else {
                Ok(None)
            }
        }
        _ => {
            let _ = eval_stmt_async(caller, stmt, scope_env, false, eval_locals).await?;
            Ok(None)
        }
    }
}

pub(crate) fn eval_declare_local(
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
    name: &str,
    kind: EvalLocalKind,
    value: i64,
) -> Result<(), String> {
    if let Some(binding) = eval_locals.get_mut(name) {
        if !matches!(binding.kind, EvalLocalKind::Var) || !matches!(kind, EvalLocalKind::Var) {
            return Err(format!(
                "SyntaxError: cannot redeclare identifier `{name}` in eval"
            ));
        }
        binding.value = value;
        return Ok(());
    }
    eval_locals.insert(name.to_string(), EvalLocalBinding { kind, value });
    Ok(())
}

pub(crate) fn set_host_data_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id = find_memory_c_string_global(caller, name)
        .or_else(|| alloc_heap_c_string_global(caller, name))?;
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize)?;
    if let Some((slot_offset, flags, _old)) =
        find_property_slot_by_name_id(caller, obj_ptr, name_id)
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if flags & constants::FLAG_WRITABLE == 0 || slot_offset + 16 > data.len() {
            return None;
        }
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        Some(())
    } else {
        define_host_data_property_from_caller(caller, obj, name, val)
    }
}

pub(crate) fn eval_to_number(val: i64) -> f64 {
    if value::is_f64(val) {
        f64::from_bits(val as u64)
    } else if value::is_bool(val) {
        if value::decode_bool(val) { 1.0 } else { 0.0 }
    } else if value::is_null(val) {
        0.0
    } else {
        f64::NAN
    }
}

pub(crate) fn eval_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_string(val) {
        read_value_string_bytes(caller, val)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    } else if value::is_f64(val) {
        let number = f64::from_bits(val as u64);
        if number.fract() == 0.0 {
            format!("{}", number as i64)
        } else {
            number.to_string()
        }
    } else if value::is_bool(val) {
        value::decode_bool(val).to_string()
    } else if value::is_null(val) {
        "null".to_string()
    } else if value::is_undefined(val) {
        "undefined".to_string()
    } else {
        "[object Object]".to_string()
    }
}

/// ToPropertyKey 抽象操作 (ECMAScript 7.1.14)
/// 先 ToPrimitive hint String，若结果是 Symbol 则抛出 TypeError
pub(crate) fn to_property_key(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    let key = to_primitive(caller, val);
    if value::is_symbol(key) {
        *caller
            .data()
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
            Some("TypeError: Cannot convert a Symbol to a string".to_string());
        return String::new();
    }
    eval_to_string(caller, key)
}

pub(crate) fn scope_record_create(mut caller: Caller<'_, RuntimeState>, capacity: i64) -> i64 {
    let data = caller.data_mut();
    let handle = data.scope_record_next_handle;
    data.scope_record_next_handle += 1;
    let cap = f64::from_bits(capacity as u64);
    let cap = if cap.is_finite() && cap >= 0.0 {
        cap as usize
    } else {
        0
    };
    data.scope_records.insert(
        handle,
        ScopeRecord {
            bindings: Vec::with_capacity(cap),
            home_object: None,
            new_target: None,
            has_arguments_binding: false,
            is_strict: false,
        },
    );
    value::encode_scope_record_handle(handle)
}

pub(crate) fn scope_record_add_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
    val: i64,
    is_tdz: i64,
    is_const: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return 0i64;
    }
    let initialized = !value::decode_bool(is_tdz);
    let constant = value::decode_bool(is_const);
    if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
        rec.bindings.push((name_str, val, initialized, constant));
    }
    0i64
}

pub(crate) fn eval_get_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return value::encode_undefined();
    }
    // Magic name for new.target stored via scope_record_set_meta(key=3)
    if name_str == "__wjsm_new_target" {
        if let Some(rec) = caller.data().scope_records.get(&handle) {
            return rec.new_target.unwrap_or(value::encode_undefined());
        }
        return value::encode_undefined();
    }
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        for (n, v, init, _) in &rec.bindings {
            if n == &name_str {
                if !init {
                    let msg = format!("Cannot access '{}' before initialization", name_str);
                    let msg_val = store_runtime_string(&caller, msg.clone());
                    let error_obj = create_error_object(&mut caller, "ReferenceError", msg_val);
                    {
                        let mut errors = caller.data().error_table.lock().unwrap();
                        let idx = errors.len() as u32;
                        errors.push(crate::ErrorEntry {
                            name: "ReferenceError".to_string(),
                            message: msg,
                            value: error_obj,
                        });
                        return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                    }
                }
                return *v;
            }
        }
    }
    value::encode_undefined()
}

pub(crate) fn eval_set_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
    val: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return value::encode_undefined();
    }
    if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
        for (n, v, init, is_const) in rec.bindings.iter_mut() {
            if n == &name_str {
                if *is_const {
                    let msg = format!("assignment to constant `{}`", name_str);
                    let msg_val = store_runtime_string(&caller, msg.clone());
                    let error_obj = create_error_object(&mut caller, "TypeError", msg_val);
                    {
                        let mut errors = caller.data().error_table.lock().unwrap();
                        let idx = errors.len() as u32;
                        errors.push(crate::ErrorEntry {
                            name: "TypeError".to_string(),
                            message: msg,
                            value: error_obj,
                        });
                        return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                    }
                }
                *v = val;
                *init = true;
                return val;
            }
        }
        return val;
    }
    value::encode_undefined()
}

pub(crate) fn eval_has_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return value::encode_bool(false);
    }
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        let found = rec.bindings.iter().any(|(n, _, _, _)| n == &name_str);
        return value::encode_bool(found);
    }
    value::encode_bool(false)
}

pub(crate) fn eval_super_base(caller: Caller<'_, RuntimeState>, record: i64) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        if let Some(home) = rec.home_object {
            return home;
        }
        if let Some((_, val, initialized, _)) = rec
            .bindings
            .iter()
            .find(|(name, _, _, _)| name == "__wjsm_super_base")
            && *initialized
        {
            return *val;
        }
    }
    value::encode_undefined()
}

pub(crate) fn scope_record_set_meta(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    key: i64,
    val: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let tag = if value::is_f64(key) {
        value::decode_f64(key) as u8
    } else {
        key as u8
    };
    if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
        match tag {
            0 => rec.is_strict = value::decode_bool(val),
            1 => rec.has_arguments_binding = value::decode_bool(val),
            2 => rec.home_object = Some(val),
            3 => rec.new_target = Some(val),
            _ => debug_assert!(false, "unknown scope record meta key: {}", tag),
        }
    }
    0i64
}

pub(crate) fn scope_record_destroy(mut caller: Caller<'_, RuntimeState>, record: i64) {
    let handle = value::decode_scope_record_handle(record);
    caller.data_mut().scope_records.remove(&handle);
}
