use super::*;

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
    if let Some(env) = scope_env
        && (code.contains("var arguments") || code.contains("function arguments"))
    {
        let has_arguments = resolve_handle(caller, env)
            .and_then(|ptr| read_object_property_by_name(caller, ptr, "arguments"))
            .is_some();

        if !has_arguments {
            let msg = "SyntaxError: declaring 'arguments' in eval code is invalid";
            set_runtime_error(caller.data(), msg.to_string());
            return value::encode_undefined();
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
