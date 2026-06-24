use super::*;

/// Host-allocated scope record implementing spec-like scope behavior.
#[derive(Clone)]
pub(crate) struct ScopeRecord {
    pub(crate) bindings: Vec<(String, i64, bool, bool)>,
    pub(crate) home_object: Option<i64>,
    pub(crate) new_target: Option<i64>,
    pub(crate) has_arguments_binding: bool,
    pub(crate) is_strict: bool,
}

fn sync_eval_new_target_from_scope_record(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
) {
    let Some(env) = scope_env else {
        return;
    };
    let handle = value::decode_scope_record_handle(env);
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        let nt = rec
            .new_target
            .filter(|v| !value::is_undefined(*v))
            .or_else(|| {
                rec.bindings.iter().find_map(|(n, v, init, _)| {
                    (n == "__wjsm_new_target" && *init && !value::is_undefined(*v)).then_some(*v)
                })
            });
        if let Some(nt) = nt {
            caller
                .data()
                .new_target
                .store(nt, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

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

    sync_eval_new_target_from_scope_record(caller, scope_env);
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
        let grow_pages = (need_end - mem_len).div_ceil(page_size).max(1) as u64;
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
    const SCOPE_RECORD_CACHE_VERSION: u64 = 5;
    SCOPE_RECORD_CACHE_VERSION.hash(&mut hasher);
    let key = hasher.finish();

    if let Some(bytes) = state
        .eval_cache.lock().unwrap_or_else(|e| e.into_inner())
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
        .eval_cache.lock().unwrap_or_else(|e| e.into_inner())
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
                    value::encode_f64(value::decode_f64(a) % value::decode_f64(b))
                },
            );
        }
        "f64_pow" => {
            return Func::wrap(
                &mut *caller,
                |_: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    value::encode_f64(value::decode_f64(a).powf(value::decode_f64(b)))
                },
            );
        }
        "new_target" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, _dummy: i64| {
                    caller
                        .data()
                        .new_target
                        .load(std::sync::atomic::Ordering::Relaxed)
                },
            );
        }
        "new_target_set" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, new_target: i64| {
                    caller
                        .data()
                        .new_target
                        .swap(new_target, std::sync::atomic::Ordering::Relaxed)
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
        "scope_record_create" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, capacity: i64| {
                    scope_record_create(caller, capacity)
                },
            );
        }
        "scope_record_add_binding" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>,
                 record: i64,
                 name: i64,
                 val: i64,
                 is_tdz: i64,
                 is_const: i64|
                 -> i64 {
                    scope_record_add_binding(caller, record, name, val, is_tdz, is_const)
                },
            );
        }
        "eval_get_binding" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
                    eval_get_binding(&mut caller, record, name)
                },
            );
        }
        "eval_set_binding" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64| -> i64 {
                    eval_set_binding(&mut caller, record, name, val)
                },
            );
        }
        "eval_has_binding" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
                    eval_has_binding(caller, record, name)
                },
            );
        }
        "eval_super_base" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, record: i64| eval_super_base(caller, record),
            );
        }
        "scope_record_set_meta" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, record: i64, key: i64, val: i64| -> i64 {
                    scope_record_set_meta(caller, record, key, val)
                },
            );
        }
        "scope_record_destroy" => {
            return Func::wrap(
                &mut *caller,
                |caller: Caller<'_, RuntimeState>, record: i64| {
                    scope_record_destroy(caller, record)
                },
            );
        }
        "symbol_property_key" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
                    if let Some(name_id) = symbol_value_to_name_id(key) {
                        return name_id as i32;
                    }
                    if value::is_runtime_string_handle(key) || value::is_f64(key) {
                        if let Ok(s) = render_value(&mut caller, key)
                            && let Some(id) = find_memory_c_string(&mut caller, &s)
                                .or_else(|| alloc_heap_c_string(&mut caller, &s))
                        {
                            return id as i32;
                        }
                        return 0;
                    }
                    key as i32
                },
            );
        }
        "string_to_array_index" => {
            return Func::wrap(
                &mut *caller,
                |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
                    if !value::is_string(key) {
                        return -1;
                    }
                    let Ok(s) = render_value(&mut caller, key) else {
                        return -1;
                    };
                    match s.parse::<u32>() {
                        Ok(n) if (n as i64) < i32::MAX as i64 && n.to_string() == s => n as i32,
                        _ => -1,
                    }
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

    let module = match wjsm_parser::parse_script_as_module(&code) {
        Ok(module) => module,
        Err(error) => {
            set_runtime_error(caller.data(), format!("SyntaxError: {error}"));
            return value::encode_undefined();
        }
    };
    let strict_eval_source = runtime_module_has_use_strict_directive(&module);
    if strict_eval_source
        && let Some(env) = scope_env
    {
        let handle = value::decode_scope_record_handle(env);
        if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
            rec.is_strict = true;
        }
    }
    let caller_is_strict = scope_env
        .map(|env| eval_scope_has_strict_marker(caller, env))
        .unwrap_or(false);
    let strict_eval = strict_eval_source || caller_is_strict;
    let var_writes_to_scope = scope_env.map(|_| !strict_eval).unwrap_or(false);

    for item in &module.body {
        if let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl))) = item {
            let name = fn_decl.ident.sym.as_ref();
            if matches!(name, "NaN" | "Infinity" | "undefined") {
                let msg = format!("Cannot define function '{}' in eval", fn_decl.ident.sym);
                let msg_val = store_runtime_string(caller, msg.clone());
                let error_obj = create_error_object(caller, "TypeError", msg_val);
                {
                    let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                    let idx = errors.len() as u32;
                    errors.push(crate::ErrorEntry {
                        name: "TypeError".to_string(),
                        message: msg,
                        value: error_obj,
                    });
                    return value::encode_handle(value::TAG_EXCEPTION, idx);
                }
            }
        }
    }

    let output_len = caller
        .data()
        .output.lock().unwrap_or_else(|e| e.into_inner())
        .len();
    let previous_runtime_error = caller
        .data()
        .runtime_error.lock().unwrap_or_else(|e| e.into_inner())
        .clone();
    let previous_error_count = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner()).len();

    match try_compiled_eval_from_caller_async(
        caller,
        &code,
        &module,
        scope_env,
        var_writes_to_scope,
    )
    .await
    {
        Ok(value) => {
            if value::is_exception(value) {
                return value;
            }
            let current_runtime_error = caller
                .data()
                .runtime_error.lock().unwrap_or_else(|e| e.into_inner())
                .clone();
            if value::is_undefined(value) && current_runtime_error != previous_runtime_error {
                return value::encode_undefined();
            }
            value
        }
        Err(_error) => {
            let thrown_exception = {
                let errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                if errors.len() > previous_error_count {
                    Some((errors.len() - 1) as u32)
                } else {
                    None
                }
            };

            if let Some(idx) = thrown_exception {
                caller
                    .data()
                    .output.lock().unwrap_or_else(|e| e.into_inner())
                    .truncate(output_len);
                *caller
                    .data()
                    .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = previous_runtime_error;
                return value::encode_handle(value::TAG_EXCEPTION, idx);
            }

            caller
                .data()
                .output.lock().unwrap_or_else(|e| e.into_inner())
                .truncate(output_len);
            *caller
                .data()
                .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = previous_runtime_error;
            let mut eval_locals = HashMap::new();
            match eval_module_items(
                caller,
                &module.body,
                scope_env,
                var_writes_to_scope,
                &mut eval_locals,
            ) {
                Ok(completion) => completion.unwrap_or_else(value::encode_undefined),
                Err(msg) => eval_exception_from_message(caller, msg),
            }
        }
    }
}

fn eval_exception_from_message(caller: &mut Caller<'_, RuntimeState>, msg: String) -> i64 {
    let thrown = msg
        .strip_prefix("Uncaught exception: ")
        .unwrap_or(msg.as_str())
        .to_string();
    let value = store_runtime_string(caller, thrown.clone());
    let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(crate::ErrorEntry {
        name: "Error".to_string(),
        message: thrown,
        value,
    });
    value::encode_exception(idx)
}

pub(crate) fn runtime_module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
    let mut found = false;
    for item in &module.body {
        let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Expr(expr_stmt)) = item else {
            break;
        };
        let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr_stmt.expr.as_ref() else {
            break;
        };
        if string.value.as_str() == Some("use strict") {
            found = true;
        }
    }
    found
}

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

fn eval_for_head_init(
    caller: &mut Caller<'_, RuntimeState>,
    head: &swc_ast::VarDeclOrExpr,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<(), String> {
    match head {
        swc_ast::VarDeclOrExpr::VarDecl(var_decl) => {
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
                        eval_declare_or_write_var_binding(
                            caller,
                            scope_env,
                            eval_locals,
                            name,
                            value,
                        )?;
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
            Ok(())
        }
        swc_ast::VarDeclOrExpr::Expr(expr) => {
            eval_expr(caller, expr, scope_env, eval_locals)?;
            Ok(())
        }
    }
}

fn eval_for_in_lhs(
    caller: &mut Caller<'_, RuntimeState>,
    left: &swc_ast::ForHead,
    key_val: i64,
    scope_env: Option<i64>,
    var_writes_to_scope: bool,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<(), String> {
    match left {
        swc_ast::ForHead::VarDecl(var_decl) => {
            let declarator = var_decl
                .decls
                .first()
                .ok_or_else(|| "SyntaxError: unsupported eval for-in declaration".to_string())?;
            let Some(name) = pat_ident_name(&declarator.name) else {
                return Err("SyntaxError: unsupported eval for-in pattern".to_string());
            };
            match var_decl.kind {
                swc_ast::VarDeclKind::Var if var_writes_to_scope => {
                    eval_declare_or_write_var_binding(
                        caller,
                        scope_env,
                        eval_locals,
                        name,
                        key_val,
                    )?;
                }
                _ => {
                    eval_declare_local(eval_locals, name, EvalLocalKind::Var, key_val)?;
                }
            }
            Ok(())
        }
        swc_ast::ForHead::Pat(pat) => {
            let Some(name) = pat_ident_name(pat) else {
                return Err("SyntaxError: unsupported eval for-in pattern".to_string());
            };
            if var_writes_to_scope {
                eval_write_binding(caller, scope_env, eval_locals, name, key_val)?;
            } else {
                eval_declare_local(eval_locals, name, EvalLocalKind::Var, key_val)?;
            }
            Ok(())
        }
        _ => Err("SyntaxError: unsupported eval for-in left-hand side".to_string()),
    }
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
                        eval_declare_or_write_var_binding(
                            caller,
                            scope_env,
                            eval_locals,
                            name,
                            value,
                        )?;
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
                eval_declare_or_write_var_binding(caller, scope_env, eval_locals, name, value)?;
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

            Err(format!("Uncaught exception: {rendered}"))
        }
        swc_ast::Stmt::For(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                eval_for_head_init(caller, init, scope_env, var_writes_to_scope, eval_locals)?;
            }
            let mut completion = None;
            loop {
                if let Some(test) = &for_stmt.test {
                    let test_val = eval_expr(caller, test, scope_env, eval_locals)?;
                    if value::is_falsy(test_val) {
                        break;
                    }
                }
                if let Some(value) = eval_stmt(
                    caller,
                    &for_stmt.body,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                )? {
                    completion = Some(value);
                }
                if let Some(update) = &for_stmt.update {
                    eval_expr(caller, update, scope_env, eval_locals)?;
                }
            }
            Ok(completion)
        }
        swc_ast::Stmt::ForIn(for_in) => {
            let iterable = eval_expr(caller, &for_in.right, scope_env, eval_locals)?;
            let mut completion = None;
            if value::is_js_object(iterable) {
                let keys = enumerate_object_keys(caller, iterable);
                for key in keys {
                    let key_val = store_runtime_string(caller, key);
                    eval_for_in_lhs(
                        caller,
                        &for_in.left,
                        key_val,
                        scope_env,
                        var_writes_to_scope,
                        eval_locals,
                    )?;
                    if let Some(value) = eval_stmt(
                        caller,
                        &for_in.body,
                        scope_env,
                        var_writes_to_scope,
                        eval_locals,
                    )? {
                        completion = Some(value);
                    }
                }
            }
            Ok(completion)
        }
        swc_ast::Stmt::ForOf(_) => Err("SyntaxError: unsupported eval statement".to_string()),
        swc_ast::Stmt::While(while_stmt) => {
            let mut completion = None;
            loop {
                let test = eval_expr(caller, &while_stmt.test, scope_env, eval_locals)?;
                if value::is_falsy(test) {
                    break;
                }
                if let Some(value) = eval_stmt(
                    caller,
                    &while_stmt.body,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                )? {
                    completion = Some(value);
                }
            }
            Ok(completion)
        }
        swc_ast::Stmt::DoWhile(dw) => {
            let mut completion = None;
            loop {
                if let Some(value) = eval_stmt(
                    caller,
                    &dw.body,
                    scope_env,
                    var_writes_to_scope,
                    eval_locals,
                )? {
                    completion = Some(value);
                }
                let test = eval_expr(caller, &dw.test, scope_env, eval_locals)?;
                if value::is_falsy(test) {
                    break;
                }
            }
            Ok(completion)
        }
        swc_ast::Stmt::Switch(switch_stmt) => {
            let discriminant =
                eval_expr(caller, &switch_stmt.discriminant, scope_env, eval_locals)?;
            let mut completion = None;
            let mut matched = false;
            let mut default_case: Option<&[swc_ast::Stmt]> = None;
            for case in &switch_stmt.cases {
                if case.test.is_none() {
                    default_case = Some(&case.cons);
                    continue;
                }
                if !matched {
                    let test =
                        eval_expr(caller, case.test.as_ref().unwrap(), scope_env, eval_locals)?;
                    if !value::is_falsy(strict_eq(caller, test, discriminant)) {
                        matched = true;
                    }
                }
                if matched {
                    for stmt in &case.cons {
                        if matches!(stmt, swc_ast::Stmt::Break(_)) {
                            return Ok(completion);
                        }
                        if let Some(value) =
                            eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)?
                        {
                            completion = Some(value);
                        }
                    }
                }
            }
            if !matched
                && let Some(stmts) = default_case
            {
                for stmt in stmts {
                    if matches!(stmt, swc_ast::Stmt::Break(_)) {
                        return Ok(completion);
                    }
                    if let Some(value) =
                        eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)?
                    {
                        completion = Some(value);
                    }
                }
            }
            Ok(completion)
        }
        swc_ast::Stmt::Try(try_stmt) => {
            let result = eval_block(
                caller,
                &try_stmt.block.stmts,
                scope_env,
                var_writes_to_scope,
                eval_locals,
            );
            match result {
                Ok(value) => {
                    if let Some(finally) = &try_stmt.finalizer {
                        eval_block(
                            caller,
                            &finally.stmts,
                            scope_env,
                            var_writes_to_scope,
                            eval_locals,
                        )?;
                    }
                    Ok(value)
                }
                Err(err_msg) => {
                    if let Some(handler) = &try_stmt.handler {
                        let param_name = handler
                            .param
                            .as_ref()
                            .and_then(|p| pat_ident_name(p))
                            .unwrap_or("err");
                        let err_val = store_runtime_string(caller, err_msg.clone());
                        eval_declare_local(eval_locals, param_name, EvalLocalKind::Let, err_val)?;
                        eval_block(
                            caller,
                            &handler.body.stmts,
                            scope_env,
                            var_writes_to_scope,
                            eval_locals,
                        )?;
                        if let Some(finally) = &try_stmt.finalizer {
                            eval_block(
                                caller,
                                &finally.stmts,
                                scope_env,
                                var_writes_to_scope,
                                eval_locals,
                            )?;
                        }
                        Ok(None)
                    } else if let Some(finally) = &try_stmt.finalizer {
                        eval_block(
                            caller,
                            &finally.stmts,
                            scope_env,
                            var_writes_to_scope,
                            eval_locals,
                        )?;
                        Err(err_msg)
                    } else {
                        Err(err_msg)
                    }
                }
            }
        }
        swc_ast::Stmt::Return(ret_stmt) => {
            let value = if let Some(arg) = &ret_stmt.arg {
                eval_expr(caller, arg, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            Ok(Some(value))
        }
        swc_ast::Stmt::Labeled(label_stmt) => eval_stmt(
            caller,
            &label_stmt.body,
            scope_env,
            var_writes_to_scope,
            eval_locals,
        ),
        swc_ast::Stmt::Break(_) | swc_ast::Stmt::Continue(_) => {
            Err("SyntaxError: unsupported eval statement".to_string())
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
    eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)
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

fn eval_array_lit(
    caller: &mut Caller<'_, RuntimeState>,
    arr: &swc_ast::ArrayLit,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let len = arr.elems.len() as u32;
    let array_val = alloc_array(caller, len.max(1));
    let Some(ptr) = resolve_array_ptr(caller, array_val) else {
        return Ok(array_val);
    };
    let mut index = 0u32;
    for elem in &arr.elems {
        match elem {
            None => {
                write_array_hole(caller, ptr, index);
                index += 1;
            }
            Some(swc_ast::ExprOrSpread {
                spread: Some(_), ..
            }) => {
                return Err("SyntaxError: unsupported eval array spread".to_string());
            }
            Some(swc_ast::ExprOrSpread { spread: None, expr }) => {
                let v = eval_expr(caller, expr, scope_env, eval_locals)?;
                write_array_elem(caller, ptr, index, v);
                index += 1;
            }
        }
    }
    write_array_length(caller, ptr, index);
    Ok(array_val)
}

fn eval_member_expr(
    caller: &mut Caller<'_, RuntimeState>,
    mem: &swc_ast::MemberExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let obj = eval_expr(caller, &mem.obj, scope_env, eval_locals)?;
    let key = match &mem.prop {
        swc_ast::MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
        swc_ast::MemberProp::Computed(computed) => {
            let key_val = eval_expr(caller, &computed.expr, scope_env, eval_locals)?;
            to_property_key(caller, key_val)
        }
        _ => return Err("SyntaxError: unsupported eval member property".to_string()),
    };
    if value::is_array(obj) {
        let idx = key
            .parse::<u32>()
            .map_err(|_| "SyntaxError: invalid array index in eval".to_string())?;
        let Some(ptr) = resolve_array_ptr(caller, obj) else {
            return Ok(value::encode_undefined());
        };
        return Ok(read_array_elem(caller, ptr, idx).unwrap_or(value::encode_undefined()));
    }
    let Some(ptr) = resolve_handle(caller, obj) else {
        return Ok(value::encode_undefined());
    };
    Ok(read_object_property_by_name(caller, ptr, &key).unwrap_or(value::encode_undefined()))
}

fn eval_update_expr(
    caller: &mut Caller<'_, RuntimeState>,
    update: &swc_ast::UpdateExpr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    let swc_ast::Expr::Ident(ident) = update.arg.as_ref() else {
        return Err("SyntaxError: unsupported eval update target".to_string());
    };
    let name = ident.sym.as_ref();
    let old_value = eval_read_binding(caller, scope_env, eval_locals, name)
        .unwrap_or_else(value::encode_undefined);
    let old_number = eval_to_number(caller, old_value);
    let new_number = match update.op {
        swc_ast::UpdateOp::PlusPlus => old_number + 1.0,
        swc_ast::UpdateOp::MinusMinus => old_number - 1.0,
    };
    let new_value = value::encode_f64(new_number);
    eval_write_binding(caller, scope_env, eval_locals, name, new_value)?;
    if update.prefix {
        Ok(new_value)
    } else {
        Ok(old_value)
    }
}

fn eval_assignment_value(
    caller: &mut Caller<'_, RuntimeState>,
    op: swc_ast::AssignOp,
    current: i64,
    rhs: i64,
) -> Result<i64, String> {
    match op {
        swc_ast::AssignOp::Assign => Ok(rhs),
        swc_ast::AssignOp::AddAssign => eval_binary(caller, swc_ast::BinaryOp::Add, current, rhs),
        swc_ast::AssignOp::SubAssign => eval_binary(caller, swc_ast::BinaryOp::Sub, current, rhs),
        swc_ast::AssignOp::MulAssign => eval_binary(caller, swc_ast::BinaryOp::Mul, current, rhs),
        swc_ast::AssignOp::DivAssign => eval_binary(caller, swc_ast::BinaryOp::Div, current, rhs),
        swc_ast::AssignOp::ModAssign => eval_binary(caller, swc_ast::BinaryOp::Mod, current, rhs),
        _ => Err("SyntaxError: unsupported eval assignment operator".to_string()),
    }
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
            let val = eval_read_binding(caller, scope_env, eval_locals, ident.sym.as_ref())
                .unwrap_or_else(value::encode_undefined);
            if value::is_exception(val) {
                return Err("ReferenceError".to_string());
            }
            Ok(val)
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
            eval_unary(caller, unary.op, val)
        }
        swc_ast::Expr::Update(update) => eval_update_expr(caller, update, scope_env, eval_locals),
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
        swc_ast::Expr::MetaProp(meta) => match meta.kind {
            swc_ast::MetaPropKind::NewTarget => {
                use std::sync::atomic::Ordering;
                if let Some(env) = scope_env {
                    let handle = value::decode_scope_record_handle(env);
                    if let Some(rec) = caller.data().scope_records.get(&handle)
                        && let Some(nt) = rec.new_target
                    {
                        return Ok(nt);
                    }
                }
                Ok(caller.data().new_target.load(Ordering::Relaxed))
            }
            swc_ast::MetaPropKind::ImportMeta => {
                Err("SyntaxError: import.meta is not supported in eval".to_string())
            }
        },
        swc_ast::Expr::Array(arr) => eval_array_lit(caller, arr, scope_env, eval_locals),
        swc_ast::Expr::Member(mem) => eval_member_expr(caller, mem, scope_env, eval_locals),
        _ => Err("SyntaxError: unsupported eval expression".to_string()),
    }
}

pub(crate) async fn eval_expr_async(
    caller: &mut Caller<'_, RuntimeState>,
    expr: &swc_ast::Expr,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
) -> Result<i64, String> {
    eval_expr(caller, expr, scope_env, eval_locals)
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

    let a = eval_to_number(caller, lhs);
    let b = eval_to_number(caller, rhs);
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

pub(crate) fn eval_unary(
    caller: &mut Caller<'_, RuntimeState>,
    op: swc_ast::UnaryOp,
    val: i64,
) -> Result<i64, String> {
    match op {
        swc_ast::UnaryOp::Minus => Ok(value::encode_f64(-eval_to_number(caller, val))),
        swc_ast::UnaryOp::Plus => Ok(value::encode_f64(eval_to_number(caller, val))),
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
    let swc_ast::AssignTarget::Simple(simple) = &assign.left else {
        return Err("SyntaxError: unsupported eval assignment target".to_string());
    };
    match simple {
        swc_ast::SimpleAssignTarget::Ident(ident) => {
            let name = ident.id.sym.as_ref();
            let rhs = eval_expr(caller, &assign.right, scope_env, eval_locals)?;
            let current = if matches!(assign.op, swc_ast::AssignOp::Assign) {
                value::encode_undefined()
            } else {
                eval_read_binding(caller, scope_env, eval_locals, name)
                    .unwrap_or_else(value::encode_undefined)
            };
            let val = eval_assignment_value(caller, assign.op, current, rhs)?;
            eval_write_binding(caller, scope_env, eval_locals, name, val)?;
            Ok(val)
        }
        swc_ast::SimpleAssignTarget::Member(member) => {
            let obj = eval_expr(caller, &member.obj, scope_env, eval_locals)?;
            let key = match &member.prop {
                swc_ast::MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                swc_ast::MemberProp::Computed(computed) => {
                    let key_val = eval_expr(caller, &computed.expr, scope_env, eval_locals)?;
                    to_property_key(caller, key_val)
                }
                _ => return Err("SyntaxError: unsupported eval assignment target".to_string()),
            };
            let rhs = eval_expr(caller, &assign.right, scope_env, eval_locals)?;
            let current = if matches!(assign.op, swc_ast::AssignOp::Assign) {
                value::encode_undefined()
            } else {
                eval_member_expr(caller, member, scope_env, eval_locals)?
            };
            let val = eval_assignment_value(caller, assign.op, current, rhs)?;
            if value::is_array(obj) {
                let idx = key
                    .parse::<u32>()
                    .map_err(|_| "SyntaxError: invalid array index in eval".to_string())?;
                let Some(ptr) = resolve_array_ptr(caller, obj) else {
                    return Ok(val);
                };
                write_array_elem(caller, ptr, idx, val);
                if read_array_length(caller, ptr).is_some_and(|len| idx >= len) {
                    write_array_length(caller, ptr, idx + 1);
                }
            } else if value::is_js_object(obj) {
                set_host_data_property_from_caller(caller, obj, &key, val);
            } else {
                return Err("SyntaxError: unsupported eval assignment target".to_string());
            }
            Ok(val)
        }
        _ => Err("SyntaxError: unsupported eval assignment target".to_string()),
    }
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
            let _arg = if let Some(first) = call.args.first() {
                eval_expr(caller, &first.expr, scope_env, eval_locals)?
            } else {
                value::encode_undefined()
            };
            return Err(
                "direct eval is unsupported on the sync interpreter path; reaches via async perform_eval_from_caller_async".to_string(),
            );
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
    let handle = value::decode_scope_record_handle(scope_env);
    caller
        .data()
        .scope_records
        .get(&handle)
        .map(|rec| rec.is_strict)
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
    if let Some(env) = scope_env
        && value::is_scope_record(env)
    {
        let name_val = store_runtime_string(caller, name.to_string());
        let got = eval_get_binding(caller, env, name_val);
        if value::is_exception(got) {
            // TDZ 或其他错误：直接抛出而非返回 None
            return Some(got);
        }
        return Some(got);
    }
    let env = scope_env?;
    let ptr = resolve_handle(caller, env)?;
    read_object_property_by_name(caller, ptr, name)
}

fn eval_apply_set_binding_result(
    caller: &mut Caller<'_, RuntimeState>,
    result: i64,
) -> Result<(), String> {
    if value::is_exception(result) {
        let idx = value::decode_handle(result) as u32;
        let msg = caller
            .data()
            .error_table
            .lock()
            .ok()
            .and_then(|e| e.get(idx as usize).map(|x| x.message.clone()))
            .unwrap_or_else(|| "ReferenceError".to_string());
        set_runtime_error(caller.data(), msg.clone());
        return Err(msg);
    }
    Ok(())
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
    if value::is_scope_record(env) {
        let name_val = store_runtime_string(caller, name.to_string());
        let result = eval_set_binding(caller, env, name_val, val);
        return eval_apply_set_binding_result(caller, result);
    }
    let _ = set_host_data_property_from_caller(caller, env, name, val);
    Ok(())
}

pub(crate) fn eval_declare_or_write_var_binding(
    caller: &mut Caller<'_, RuntimeState>,
    scope_env: Option<i64>,
    eval_locals: &mut HashMap<String, EvalLocalBinding>,
    name: &str,
    val: i64,
) -> Result<(), String> {
    if let Some(binding) = eval_locals.get(name)
        && !matches!(binding.kind, EvalLocalKind::Var)
    {
        return Err(format!(
            "SyntaxError: cannot redeclare identifier `{name}` in eval"
        ));
    }

    let Some(env) = scope_env else {
        return eval_declare_local(eval_locals, name, EvalLocalKind::Var, val);
    };

    if value::is_scope_record(env) {
        let handle = value::decode_scope_record_handle(env);
        if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle)
            && !rec
                .bindings
                .iter()
                .any(|(binding_name, _, _, _)| binding_name == name)
        {
            rec.bindings.push((name.to_string(), val, true, false));
            return Ok(());
        }
    }

    eval_write_binding(caller, Some(env), eval_locals, name, val)
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
        .native_callables.lock().unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(NativeCallable::EvalFunction(function));
    value::encode_native_callable_idx(handle)
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

pub(crate) fn eval_to_number(caller: &mut Caller<'_, RuntimeState>, val: i64) -> f64 {
    value::decode_f64(to_number(caller, val))
}

pub(crate) fn eval_to_string(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_string(val) {
        read_value_string_bytes(caller, val)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    } else if value::is_f64(val) {
        let number = value::decode_f64(val);
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
            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
            Some("TypeError: Cannot convert a Symbol to a string".to_string());
        return String::new();
    }
    eval_to_string(caller, key)
}

pub(crate) fn scope_record_create(mut caller: Caller<'_, RuntimeState>, capacity: i64) -> i64 {
    let data = caller.data_mut();
    let handle = data.scope_record_next_handle;
    data.scope_record_next_handle += 1;
    let cap = value::decode_f64(capacity);
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
    let initialized = !decode_scope_record_meta_bool(is_tdz);
    let constant = decode_scope_record_meta_bool(is_const);
    if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
        rec.bindings.push((name_str, val, initialized, constant));
    }
    0i64
}

pub(crate) fn eval_get_binding(
    caller: &mut Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return value::encode_undefined();
    }
    if name_str == "__wjsm_new_target" {
        if let Some(rec) = caller.data().scope_records.get(&handle) {
            if let Some(nt) = rec.new_target.filter(|v| !value::is_undefined(*v)) {
                return nt;
            }
            for (n, v, init, _) in &rec.bindings {
                if n == "__wjsm_new_target" && *init && !value::is_undefined(*v) {
                    return *v;
                }
            }
            return caller
                .data()
                .new_target
                .load(std::sync::atomic::Ordering::Relaxed);
        }
        return value::encode_undefined();
    }
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        for (n, v, init, _) in &rec.bindings {
            if n == &name_str {
                if !init {
                    let msg = format!("Cannot access '{}' before initialization", name_str);
                    let msg_val = store_runtime_string(caller, msg.clone());
                    let error_obj = create_error_object(caller, "ReferenceError", msg_val);
                    {
                        let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                        let idx = errors.len() as u32;
                        errors.push(crate::ErrorEntry {
                            name: "ReferenceError".to_string(),
                            message: msg,
                            value: error_obj,
                        });
                        return value::encode_handle(value::TAG_EXCEPTION, idx);
                    }
                }
                return *v;
            }
        }
    }
    value::encode_undefined()
}

pub(crate) fn eval_set_binding(
    caller: &mut Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
    val: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if name_str.is_empty() {
        return value::encode_undefined();
    }
    if let Some(rec) = caller.data_mut().scope_records.get_mut(&handle) {
        for (n, v, init, is_const) in rec.bindings.iter_mut() {
            if n == &name_str {
                if *is_const && *init {
                    let msg = format!("assignment to constant `{}`", name_str);
                    let msg_val = store_runtime_string(caller, msg.clone());
                    let error_obj = create_error_object(caller, "TypeError", msg_val);
                    {
                        let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                        let idx = errors.len() as u32;
                        errors.push(crate::ErrorEntry {
                            name: "TypeError".to_string(),
                            message: msg,
                            value: error_obj,
                        });
                        return value::encode_handle(value::TAG_EXCEPTION, idx);
                    }
                }
                *v = val;
                *init = true;
                return val;
            }
        }
        if rec.is_strict {
            let msg = format!("assignment to undeclared variable `{}`", name_str);
            let msg_val = store_runtime_string(caller, msg.clone());
            let error_obj = create_error_object(caller, "ReferenceError", msg_val);
            {
                let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                let idx = errors.len() as u32;
                errors.push(crate::ErrorEntry {
                    name: "ReferenceError".to_string(),
                    message: msg,
                    value: error_obj,
                });
                return value::encode_handle(value::TAG_EXCEPTION, idx);
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

fn decode_scope_record_meta_bool(val: i64) -> bool {
    if value::is_bool(val) {
        value::decode_bool(val)
    } else if value::is_f64(val) {
        value::decode_f64(val) != 0.0
    } else {
        val != 0
    }
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
            0 => rec.is_strict = decode_scope_record_meta_bool(val),
            1 => rec.has_arguments_binding = decode_scope_record_meta_bool(val),
            2 => rec.home_object = Some(val),
            3 => {
                if !value::is_undefined(val) {
                    rec.new_target = Some(val);
                }
            }
            _ => debug_assert!(false, "unknown scope record meta key: {}", tag),
        }
    }
    0i64
}

pub(crate) fn scope_record_destroy(mut caller: Caller<'_, RuntimeState>, record: i64) {
    let handle = value::decode_scope_record_handle(record);
    caller.data_mut().scope_records.remove(&handle);
}
