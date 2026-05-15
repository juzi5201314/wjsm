use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp,
    ValueId,
};
use crate::scope_tree::{ScopeKind, VarKind, LexicalMode, ScopeTree};
use crate::cfg_builder::{FunctionBuilder, LabelContext, LabelKind, FinallyContext, TryContext, StmtFlow};
use crate::builtins::*;
use crate::eval_helpers::*;
use crate::kind_strings::*;
use crate::{LoweringError, Diagnostic};
use super::lowerer::{Lowerer, ActiveUsingVar, AsyncContextState, HoistedVar, CapturedBinding, EVAL_SCOPE_ENV_PARAM, WK_SYMBOL_ITERATOR, WK_SYMBOL_SPECIES, WK_SYMBOL_TO_STRING_TAG, WK_SYMBOL_ASYNC_ITERATOR, WK_SYMBOL_HAS_INSTANCE, WK_SYMBOL_TO_PRIMITIVE, WK_SYMBOL_DISPOSE, WK_SYMBOL_MATCH, WK_SYMBOL_ASYNC_DISPOSE};

fn has_top_level_await(module: &swc_ast::Module) -> bool {
    fn expr_has_await(expr: &swc_ast::Expr) -> bool {
        match expr {
            swc_ast::Expr::Await(_) => true,
            // 边界：不递归进入函数/类体
            swc_ast::Expr::Fn(_) | swc_ast::Expr::Arrow(_) | swc_ast::Expr::Class(_) => false,
            // 递归检查子表达式
            swc_ast::Expr::Array(a) => a
                .elems
                .iter()
                .any(|e| e.as_ref().map_or(false, |e| expr_has_await(&e.expr))),
            swc_ast::Expr::Object(o) => o.props.iter().any(|p| match p {
                swc_ast::PropOrSpread::Spread(s) => expr_has_await(&s.expr),
                swc_ast::PropOrSpread::Prop(p) => match &**p {
                    swc_ast::Prop::KeyValue(kv) => expr_has_await(&kv.value),
                    swc_ast::Prop::Assign(a) => expr_has_await(&a.value),
                    _ => false,
                },
            }),
            swc_ast::Expr::Unary(u) => expr_has_await(&u.arg),
            swc_ast::Expr::Update(u) => expr_has_await(&u.arg),
            swc_ast::Expr::Bin(b) => expr_has_await(&b.left) || expr_has_await(&b.right),
            swc_ast::Expr::Assign(a) => expr_has_await(&a.right),
            swc_ast::Expr::Member(m) => {
                expr_has_await(&m.obj)
                    || matches!(&m.prop, swc_ast::MemberProp::Computed(c) if expr_has_await(&c.expr))
            }
            swc_ast::Expr::Cond(c) => {
                expr_has_await(&c.test) || expr_has_await(&c.cons) || expr_has_await(&c.alt)
            }
            swc_ast::Expr::Call(c) => {
                (match &c.callee {
                    swc_ast::Callee::Expr(e) => expr_has_await(e),
                    _ => false,
                }) || c.args.iter().any(|a| expr_has_await(&a.expr))
            }
            swc_ast::Expr::New(n) => {
                expr_has_await(&n.callee)
                    || n.args
                        .as_ref()
                        .map_or(false, |a| a.iter().any(|a| expr_has_await(&a.expr)))
            }
            swc_ast::Expr::Seq(s) => s.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::Tpl(t) => t.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::TaggedTpl(t) => {
                expr_has_await(&t.tag) || t.tpl.exprs.iter().any(|e| expr_has_await(e))
            }
            swc_ast::Expr::Yield(y) => y.arg.as_ref().map_or(false, |a| expr_has_await(a)),
            swc_ast::Expr::Paren(p) => expr_has_await(&p.expr),
            _ => false,
        }
    }

    fn decl_has_await(decl: &swc_ast::Decl) -> bool {
        match decl {
            swc_ast::Decl::Var(v) => v
                .decls
                .iter()
                .any(|d| d.init.as_ref().map_or(false, |i| expr_has_await(i))),
            swc_ast::Decl::Fn(_) | swc_ast::Decl::Class(_) => false,
            _ => false,
        }
    }

    fn stmt_has_await(stmt: &swc_ast::Stmt) -> bool {
        match stmt {
            swc_ast::Stmt::Expr(e) => expr_has_await(&e.expr),
            swc_ast::Stmt::Decl(d) => decl_has_await(d),
            swc_ast::Stmt::Block(b) => b.stmts.iter().any(stmt_has_await),
            swc_ast::Stmt::If(i) => {
                expr_has_await(&i.test)
                    || stmt_has_await(&i.cons)
                    || i.alt.as_ref().map_or(false, |a| stmt_has_await(a))
            }
            swc_ast::Stmt::While(w) => expr_has_await(&w.test) || stmt_has_await(&w.body),
            swc_ast::Stmt::DoWhile(d) => expr_has_await(&d.test) || stmt_has_await(&d.body),
            swc_ast::Stmt::For(f) => {
                f.init.as_ref().map_or(false, |init| match init {
                    swc_ast::VarDeclOrExpr::VarDecl(v) => v
                        .decls
                        .iter()
                        .any(|d| d.init.as_ref().map_or(false, |i| expr_has_await(i))),
                    swc_ast::VarDeclOrExpr::Expr(e) => expr_has_await(e),
                }) || f.test.as_ref().map_or(false, |t| expr_has_await(t))
                    || f.update.as_ref().map_or(false, |u| expr_has_await(u))
                    || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::ForIn(f) => expr_has_await(&f.right) || stmt_has_await(&f.body),
            swc_ast::Stmt::ForOf(f) => {
                f.is_await || expr_has_await(&f.right) || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::Return(r) => r.arg.as_ref().map_or(false, |a| expr_has_await(a)),
            swc_ast::Stmt::Throw(t) => expr_has_await(&t.arg),
            swc_ast::Stmt::Try(t) => {
                t.block.stmts.iter().any(stmt_has_await)
                    || t.handler
                        .as_ref()
                        .map_or(false, |h| h.body.stmts.iter().any(stmt_has_await))
                    || t.finalizer
                        .as_ref()
                        .map_or(false, |f| f.stmts.iter().any(stmt_has_await))
            }
            swc_ast::Stmt::Switch(s) => {
                expr_has_await(&s.discriminant)
                    || s.cases.iter().any(|c| {
                        c.test.as_ref().map_or(false, |t| expr_has_await(t))
                            || c.cons.iter().any(stmt_has_await)
                    })
            }
            swc_ast::Stmt::Labeled(l) => stmt_has_await(&l.body),
            swc_ast::Stmt::With(w) => expr_has_await(&w.obj) || stmt_has_await(&w.body),
            _ => false,
        }
    }

    for item in &module.body {
        match item {
            swc_ast::ModuleItem::Stmt(stmt) => {
                if stmt_has_await(stmt) {
                    return true;
                }
            }
            swc_ast::ModuleItem::ModuleDecl(decl) => match decl {
                swc_ast::ModuleDecl::ExportDecl(e) => {
                    if decl_has_await(&e.decl) {
                        return true;
                    }
                }
                swc_ast::ModuleDecl::ExportDefaultExpr(e) => {
                    if expr_has_await(&e.expr) {
                        return true;
                    }
                }
                _ => {}
            },
        }
    }
    false
}

pub fn lower_module(module: swc_ast::Module) -> Result<Program, LoweringError> {
    Lowerer::new().lower_module(&module)
}

pub fn lower_eval_module(module: swc_ast::Module) -> Result<Program, LoweringError> {
    lower_eval_module_with_scope(module, false, false)
}

pub fn lower_eval_module_with_scope(
    module: swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.eval_mode = true;
    lowerer.eval_has_scope_bridge = has_scope_bridge;
    lowerer.eval_var_writes_to_scope = var_writes_to_scope;
    lowerer.lower_module(&module)
}

/// 将多个模块编译为单一的 IR Program（模块 bundling）
///
/// # 参数
/// - `modules`: 模块列表，每个元素是 (ModuleId, AST)
/// - `import_map`: 导入映射，module_id → ImportBinding 列表
/// - `dynamic_import_targets`: 动态 import() 目标映射，module_id → 被动态 import 的目标模块 ID 列表
/// - `export_names`: 导出名称映射，module_id → 导出名集合
/// - `dynamic_import_specifiers`: 动态 import() specifier 映射，module_id → [(specifier, 目标 ModuleId)]
pub fn lower_modules(
    modules: Vec<(wjsm_ir::ModuleId, swc_ast::Module)>,
    import_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    dynamic_import_targets: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    export_names: &std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    dynamic_import_specifiers: &std::collections::HashMap<
        wjsm_ir::ModuleId,
        Vec<(String, wjsm_ir::ModuleId)>,
    >,
) -> Result<Program, LoweringError> {
    // 如果只有一个模块且没有 import，使用单模块编译路径
    if modules.len() == 1 && import_map.is_empty() {
        let (_, module) = modules.into_iter().next().unwrap();
        return lower_module(module);
    }

    // 多模块编译路径
    let mut lowerer = Lowerer::new();
    lowerer.import_bindings = import_map.clone();
    lowerer.dynamic_import_targets = dynamic_import_targets.clone();
    lowerer.module_export_names = export_names.clone();

    // 收集需要构建命名空间对象的模块
    for targets in dynamic_import_targets.values() {
        for &target_id in targets {
            lowerer.dynamic_import_namespace_modules.insert(target_id);
        }
    }

    // 构建 specifier → ModuleId 映射（从动态 import specifier 列表构建，而非 import_map）
    for (module_id, spec_list) in dynamic_import_specifiers.iter() {
        for (specifier, target_id) in spec_list {
            lowerer
                .dynamic_import_specifier_map
                .insert((*module_id, specifier.clone()), *target_id);
        }
    }

    lowerer.shared_env_stack.push(None);

    // 预扫描：为所有模块的变量声明创建作用域条目
    // 这样可以确保跨模块的 import 绑定能够找到目标变量
    for (module_id, module_ast) in &modules {
        lowerer.current_module_id = Some(*module_id);
        lowerer.predeclare_stmts(&module_ast.body)?;
        for item in &module_ast.body {
            match item {
                swc_ast::ModuleItem::ModuleDecl(swc_ast::ModuleDecl::ExportDefaultExpr(_)) => {
                    let default_var = format!("_default_export_mod{}", module_id.0);
                    let scope_id = lowerer
                        .scopes
                        .declare(&default_var, VarKind::Const, true)
                        .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
                    let ir_name = format!("${scope_id}.{default_var}");
                    lowerer
                        .export_map
                        .insert((*module_id, "default".to_string()), ir_name);
                }
                swc_ast::ModuleItem::ModuleDecl(swc_ast::ModuleDecl::ExportDefaultDecl(_)) => {
                    let default_var = format!("_default_export_mod{}", module_id.0);
                    let scope_id = lowerer
                        .scopes
                        .declare(&default_var, VarKind::Const, true)
                        .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
                    let ir_name = format!("${scope_id}.{default_var}");
                    lowerer
                        .export_map
                        .insert((*module_id, "default".to_string()), ir_name);
                }
                _ => {}
            }
        }
    }

    // 处理 import 声明：为别名导入和默认导入建立映射
    for (module_id, module_ast) in &modules {
        let bindings = lowerer.import_bindings.get(module_id);
        let Some(bindings) = bindings else { continue };
        for binding in bindings {
            for (local_name, imported_name) in &binding.names {
                if imported_name == "*" {
                    // 命名空间导入（import * as ns from '...'）暂不支持
                    return Err(LoweringError::Diagnostic(Diagnostic::new(
                        0,
                        0,
                        format!("namespace import (import * as ...) is not yet supported"),
                    )));
                }
                if imported_name == "default" {
                    if let Some(source_ir_name) = lowerer
                        .export_map
                        .get(&(binding.source_module, "default".to_string()))
                    {
                        if local_name != "default" {
                            lowerer
                                .import_aliases
                                .insert(local_name.clone(), source_ir_name.clone());
                        }
                    }
                    continue;
                }
                if local_name != imported_name {
                    if let Ok(scope_id) = lowerer.scopes.resolve_scope_id(imported_name) {
                        let source_ir_name = format!("${scope_id}.{imported_name}");
                        lowerer
                            .import_aliases
                            .insert(local_name.clone(), source_ir_name);
                    }
                }
            }
        }
        let _ = module_ast;
    }

    // 初始化全局内置变量（undefined, NaN, Infinity）
    // 这些变量在顶层作用域中，不需要模块前缀
    let has_tla = modules.iter().any(|(_, m)| has_top_level_await(m));
    let entry = if has_tla {
        // 取第一个模块的 span 用于错误报告
        let first_span = modules
            .first()
            .map(|(_, m)| m.span)
            .unwrap_or(swc_core::common::DUMMY_SP);
        lowerer.init_async_main_context(first_span)?
    } else {
        BasicBlockId(0)
    };

    // 初始化提升的 var 变量为 undefined
    lowerer.emit_hoisted_var_initializers(entry);

    // undefined
    let undef_const = lowerer.module.add_constant(Constant::Undefined);
    let undef_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        entry,
        Instruction::Const {
            dest: undef_val,
            constant: undef_const,
        },
    );
    lowerer.current_function.append_instruction(
        entry,
        Instruction::StoreVar {
            name: "$0.undefined".to_string(),
            value: undef_val,
        },
    );
    // NaN
    let nan_const = lowerer.module.add_constant(Constant::Number(f64::NAN));
    let nan_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        entry,
        Instruction::Const {
            dest: nan_val,
            constant: nan_const,
        },
    );
    lowerer.current_function.append_instruction(
        entry,
        Instruction::StoreVar {
            name: "$0.NaN".to_string(),
            value: nan_val,
        },
    );
    // Infinity
    let inf_const = lowerer.module.add_constant(Constant::Number(f64::INFINITY));
    let inf_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        entry,
        Instruction::Const {
            dest: inf_val,
            constant: inf_const,
        },
    );
    lowerer.current_function.append_instruction(
        entry,
        Instruction::StoreVar {
            name: "$0.Infinity".to_string(),
            value: inf_val,
        },
    );

    // ── 为动态 import 的目标模块创建并注册命名空间对象 ──────────────────────
    // 必须在模块体执行前注册，否则 import() 在模块体中调用时找不到命名空间
    // 属性在模块体执行后填充（此时导出变量才有值）
    {
        let mut namespace_modules: Vec<_> = lowerer
            .dynamic_import_namespace_modules
            .iter()
            .copied()
            .collect();
        namespace_modules.sort_by_key(|id| id.0);
        for target_module_id in &namespace_modules {
            let export_names_set = lowerer.module_export_names.get(target_module_id).cloned();
            let capacity = export_names_set.as_ref().map_or(0, |s| s.len()) + 1;

            // 创建空命名空间对象
            let ns_obj = lowerer.alloc_value();
            lowerer.current_function.append_instruction(
                entry,
                Instruction::NewObject {
                    dest: ns_obj,
                    capacity: capacity as u32,
                },
            );

            // 注册到运行时缓存
            let module_id_const = lowerer
                .module
                .add_constant(Constant::ModuleId(*target_module_id));
            let module_id_val = lowerer.alloc_value();
            lowerer.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: module_id_val,
                    constant: module_id_const,
                },
            );
            lowerer.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::RegisterModuleNamespace,
                    args: vec![module_id_val, ns_obj],
                },
            );

            // 记录 ValueId 供后续属性填充使用
            lowerer
                .dynamic_import_namespace_objects
                .insert(*target_module_id, ns_obj);
        }
    }

    // 处理每个模块的 body
    let mut flow = StmtFlow::Open(entry);
    for (module_id, module_ast) in &modules {
        lowerer.current_module_id = Some(*module_id);
        for item in &module_ast.body {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    flow = lowerer.lower_stmt(stmt, flow)?;
                }
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    match decl {
                        // export const/let/var/function/class → 将内层声明作为普通语句处理
                        swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                            flow = lowerer
                                .lower_stmt(&swc_ast::Stmt::Decl(export_decl.decl.clone()), flow)?;
                            // 将导出名注册到 export_map
                            let current_mid =
                                lowerer.current_module_id.unwrap_or(wjsm_ir::ModuleId(0));
                            let names = decl_exported_names(&export_decl.decl);
                            for name in names {
                                if let Ok((scope_id, _)) = lowerer.scopes.lookup(&name) {
                                    let ir_name = format!("${scope_id}.{name}");
                                    lowerer.export_map.insert((current_mid, name), ir_name);
                                }
                            }
                        }
                        // export default expr → 计算表达式并存储到 _default_export_mod{id} 变量
                        swc_ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                            let outer_block = lowerer.ensure_open(flow)?;
                            let value_val = lowerer.lower_expr(&default_expr.expr, outer_block)?;
                            let outer_block = lowerer.ensure_open(flow)?;
                            if let Some(current_mid) = lowerer.current_module_id {
                                let default_var = format!("_default_export_mod{}", current_mid.0);
                                if let Some(ir_name) = lowerer
                                    .export_map
                                    .get(&(current_mid, "default".to_string()))
                                {
                                    lowerer.current_function.append_instruction(
                                        outer_block,
                                        Instruction::StoreVar {
                                            name: ir_name.clone(),
                                            value: value_val,
                                        },
                                    );
                                } else {
                                    let (scope_id, _) = lowerer
                                        .scopes
                                        .lookup(&default_var)
                                        .map_err(|msg| lowerer.error(default_expr.span, msg))?;
                                    let ir_name = format!("${scope_id}.{default_var}");
                                    lowerer.current_function.append_instruction(
                                        outer_block,
                                        Instruction::StoreVar {
                                            name: ir_name,
                                            value: value_val,
                                        },
                                    );
                                }
                            }
                            flow = StmtFlow::Open(outer_block);
                        }
                        // export default function/class → 将声明作为普通语句处理并存储到变量
                        swc_ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                            flow = match &default_decl.decl {
                                swc_ast::DefaultDecl::Fn(fn_expr) => {
                                    let name = fn_expr.ident.as_ref().map_or_else(
                                        || {
                                            format!(
                                                "_default_export_mod{}",
                                                lowerer.current_module_id.map_or(0, |m| m.0)
                                            )
                                        },
                                        |ident| ident.sym.to_string(),
                                    );
                                    let outer_block = lowerer.ensure_open(flow)?;
                                    let fn_val = lowerer.lower_fn_expr(
                                        &swc_ast::FnExpr {
                                            ident: Some(swc_ast::Ident::new(
                                                name.clone().into(),
                                                default_decl.span,
                                                swc_core::common::SyntaxContext::default(),
                                            )),
                                            function: fn_expr.function.clone(),
                                        },
                                        outer_block,
                                    )?;
                                    let outer_block = lowerer.ensure_open(flow)?;
                                    if let Some(current_mid) = lowerer.current_module_id {
                                        if let Some(ir_name) = lowerer
                                            .export_map
                                            .get(&(current_mid, "default".to_string()))
                                        {
                                            lowerer.current_function.append_instruction(
                                                outer_block,
                                                Instruction::StoreVar {
                                                    name: ir_name.clone(),
                                                    value: fn_val,
                                                },
                                            );
                                        }
                                    }
                                    StmtFlow::Open(outer_block)
                                }
                                swc_ast::DefaultDecl::Class(class_expr) => {
                                    let outer_block = lowerer.ensure_open(flow)?;
                                    let class_val = lowerer.lower_class_expr(
                                        &swc_ast::ClassExpr {
                                            ident: class_expr.ident.clone(),
                                            class: class_expr.class.clone(),
                                        },
                                        outer_block,
                                    )?;
                                    let outer_block = lowerer.ensure_open(flow)?;
                                    if let Some(current_mid) = lowerer.current_module_id {
                                        if let Some(ir_name) = lowerer
                                            .export_map
                                            .get(&(current_mid, "default".to_string()))
                                        {
                                            lowerer.current_function.append_instruction(
                                                outer_block,
                                                Instruction::StoreVar {
                                                    name: ir_name.clone(),
                                                    value: class_val,
                                                },
                                            );
                                        }
                                    }
                                    StmtFlow::Open(outer_block)
                                }
                                _ => flow,
                            };
                        }
                        // import 声明 → 单模块模式下跳过
                        swc_ast::ModuleDecl::Import(_) => {
                            // 暂时跳过 import（依赖已由 bundler 预处理）
                        }
                        // export { x } / export { x as y } → 将导出名注册到 export_map
                        swc_ast::ModuleDecl::ExportNamed(named_export) => {
                            let current_mid =
                                lowerer.current_module_id.unwrap_or(wjsm_ir::ModuleId(0));
                            if named_export.src.is_none() {
                                // 本地导出：export { x } / export { x as y }
                                for spec in &named_export.specifiers {
                                    if let swc_ast::ExportSpecifier::Named(named) = spec {
                                        let local_name = match &named.orig {
                                            swc_ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            swc_ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        let exported_name = named.exported.as_ref().map_or_else(
                                            || local_name.clone(),
                                            |e| match e {
                                                swc_ast::ModuleExportName::Ident(ident) => {
                                                    ident.sym.to_string()
                                                }
                                                swc_ast::ModuleExportName::Str(s) => {
                                                    s.value.to_string_lossy().into_owned()
                                                }
                                            },
                                        );
                                        if let Ok((scope_id, _)) =
                                            lowerer.scopes.lookup(&local_name)
                                        {
                                            let ir_name = format!("${scope_id}.{local_name}");
                                            lowerer
                                                .export_map
                                                .insert((current_mid, exported_name), ir_name);
                                        }
                                    }
                                }
                            }
                            // re-export (export { x } from './foo') 暂不支持，需要跨模块绑定查找
                        }
                        // export * from → 暂时跳过
                        _ => {
                            // 暂不处理 re-exports
                        }
                    }
                }
            }
        }
    }

    // ── 为动态 import 的命名空间对象填充属性 ────────────────────────────────
    // 命名空间对象已在模块体执行前创建并注册，此处仅设置属性值
    // （模块体执行后，导出变量才被赋值）
    //
    // TODO: 当前实现为一次性快照语义（SetProp 后不再更新），不符合 ES Module live binding 规范。
    // 根据规范，命名空间属性必须是 live binding：ns.x 应反映导出变量的最新值。
    // 完整修复需要 IR 层支持 getter 或在 StoreVar 时同步更新命名空间属性。
    // 这属于较大特性，需要 IR 层变更后才能实现。
    if let StmtFlow::Open(ns_block) = flow {
        let mut namespace_modules: Vec<_> = lowerer
            .dynamic_import_namespace_objects
            .keys()
            .copied()
            .collect();
        namespace_modules.sort_by_key(|id| id.0);
        for target_module_id in namespace_modules {
            let ns_obj = lowerer.dynamic_import_namespace_objects[&target_module_id];
            let export_names_set = lowerer.module_export_names.get(&target_module_id).cloned();

            // 为每个导出设置属性
            if let Some(names) = export_names_set {
                let mut sorted_names: Vec<_> = names.iter().collect();
                sorted_names.sort();
                for export_name in sorted_names {
                    if let Some(ir_name) = lowerer
                        .export_map
                        .get(&(target_module_id, export_name.clone()))
                        .cloned()
                    {
                        let value_val = lowerer.alloc_value();
                        lowerer.current_function.append_instruction(
                            ns_block,
                            Instruction::LoadVar {
                                dest: value_val,
                                name: ir_name,
                            },
                        );
                        let key_const = lowerer
                            .module
                            .add_constant(Constant::String(export_name.clone()));
                        let key_val = lowerer.alloc_value();
                        lowerer.current_function.append_instruction(
                            ns_block,
                            Instruction::Const {
                                dest: key_val,
                                constant: key_const,
                            },
                        );
                        lowerer.current_function.append_instruction(
                            ns_block,
                            Instruction::SetProp {
                                object: ns_obj,
                                key: key_val,
                                value: value_val,
                            },
                        );
                    }
                }
            }

            // 设置 Symbol.toStringTag = "Module"
            let tag_key = lowerer
                .module
                .add_constant(Constant::String("Symbol.toStringTag".to_string()));
            let tag_key_val = lowerer.alloc_value();
            lowerer.current_function.append_instruction(
                ns_block,
                Instruction::Const {
                    dest: tag_key_val,
                    constant: tag_key,
                },
            );
            let tag_value = lowerer
                .module
                .add_constant(Constant::String("Module".to_string()));
            let tag_value_val = lowerer.alloc_value();
            lowerer.current_function.append_instruction(
                ns_block,
                Instruction::Const {
                    dest: tag_value_val,
                    constant: tag_value,
                },
            );
            lowerer.current_function.append_instruction(
                ns_block,
                Instruction::SetProp {
                    object: ns_obj,
                    key: tag_key_val,
                    value: tag_value_val,
                },
            );
        }
    }

    // 完成：构建 main 函数
    match flow {
        StmtFlow::Open(block) => {
            if has_tla {
                // TLA：resolve promise 然后 return
                let undef_const = lowerer.module.add_constant(Constant::Undefined);
                let undef_val = lowerer.alloc_value();
                lowerer.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_const,
                    },
                );
                let promise_val = lowerer.alloc_value();
                lowerer.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: promise_val,
                        name: format!("${}.$promise", lowerer.async_promise_scope_id),
                    },
                );
                lowerer.current_function.append_instruction(
                    block,
                    Instruction::PromiseResolve {
                        promise: promise_val,
                        value: undef_val,
                    },
                );
                lowerer
                    .current_function
                    .set_terminator(block, Terminator::Return { value: None });
            } else {
                lowerer
                    .current_function
                    .set_terminator(block, Terminator::Return { value: None });
            }
        }
        StmtFlow::Terminated => {}
    }

    if has_tla {
        lowerer.finalize_async_main()?;
    } else {
        let has_eval = lowerer.current_function.has_eval();
        let blocks = lowerer.current_function.into_blocks();
        let mut function = Function::new("main", BasicBlockId(0));
        function.set_has_eval(has_eval);
        for block in blocks {
            function.push_block(block);
        }
        lowerer.module.push_function(function);
    }

    Ok(lowerer.module)
}


impl Lowerer {
    pub(crate) fn lower_module(mut self, module: &swc_ast::Module) -> Result<Program, LoweringError> {
        // main 函数也需要 shared_env_stack 条目（顶层闭包需要在 main 中创建 env 对象）
        self.shared_env_stack.push(None);
        self.strict_mode = module_has_use_strict_directive(module);
        // Pre-scan: hoist variable declarations so let/const are in TDZ.
        self.predeclare_stmts(&module.body)?;

        let has_tla = has_top_level_await(module);
        let entry = if has_tla {
            self.init_async_main_context(module.span)?
        } else {
            BasicBlockId(0)
        };
        self.emit_hoisted_var_initializers(entry);

        // 初始化全局内置变量：undefined, NaN, Infinity
        // undefined
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.undefined".to_string(),
                value: undef_val,
            },
        );
        // NaN
        let nan_const = self.module.add_constant(Constant::Number(f64::NAN));
        let nan_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: nan_val,
                constant: nan_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.NaN".to_string(),
                value: nan_val,
            },
        );
        // Infinity
        let inf_const = self.module.add_constant(Constant::Number(f64::INFINITY));
        let inf_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: inf_val,
                constant: inf_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.Infinity".to_string(),
                value: inf_val,
            },
        );

        let mut flow = StmtFlow::Open(entry);

        for item in &module.body {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    flow = self.lower_stmt(stmt, flow)?;
                }
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    match decl {
                        // export const/let/var/function/class → 将内层声明作为普通语句处理
                        swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                            flow = self
                                .lower_stmt(&swc_ast::Stmt::Decl(export_decl.decl.clone()), flow)?;
                        }
                        // export default expr → 将表达式作为普通语句处理
                        swc_ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                            let expr_stmt = swc_ast::ExprStmt {
                                span: default_expr.span,
                                expr: default_expr.expr.clone(),
                            };
                            flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                        }
                        // export default function/class → 作为声明处理
                        swc_ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                            match &default_decl.decl {
                                swc_ast::DefaultDecl::Fn(fn_expr) => {
                                    if let Some(ident) = &fn_expr.ident {
                                        // export default function foo() {} → 作为命名函数声明处理
                                        let decl = swc_ast::Decl::Fn(swc_ast::FnDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            function: fn_expr.function.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    } else {
                                        // 匿名默认导出函数 — 作为表达式语句求值
                                        let expr_stmt = swc_ast::ExprStmt {
                                            span: default_decl.span,
                                            expr: Box::new(swc_ast::Expr::Fn(fn_expr.clone())),
                                        };
                                        flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                                    }
                                }
                                swc_ast::DefaultDecl::Class(class_expr) => {
                                    if let Some(ident) = &class_expr.ident {
                                        // export default class Foo {} → 作为命名类声明处理
                                        let decl = swc_ast::Decl::Class(swc_ast::ClassDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            class: class_expr.class.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    }
                                    // 匿名默认导出类 — 跳过（无法作为表达式求值）
                                }
                                swc_ast::DefaultDecl::TsInterfaceDecl(_) => {
                                    // TypeScript 接口声明，跳过
                                }
                            }
                        }
                        // import 声明 → 单模块模式下跳过
                        swc_ast::ModuleDecl::Import(_) => {
                            // 单模块模式，跳过 import
                        }
                        // export * from / export { ... } → 暂时跳过
                        _ => {
                            // 暂不处理 re-exports
                        }
                    }
                }
            }
        }

        // If the last block is still open and hasn't been terminated, finalize it.
        match flow {
            StmtFlow::Open(block) => {
                if has_tla {
                    // TLA：resolve promise 然后 return
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let undef_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: undef_val,
                            constant: undef_const,
                        },
                    );
                    let promise_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: promise_val,
                            name: format!("${}.$promise", self.async_promise_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::PromiseResolve {
                            promise: promise_val,
                            value: undef_val,
                        },
                    );
                    self.current_function
                        .set_terminator(block, Terminator::Return { value: None });
                } else {
                    // 非 TLA：检查 unreachable 并设置 Return
                    let is_unreachable = self
                        .current_function
                        .block(block)
                        .map_or(false, |b| matches!(b.terminator(), Terminator::Unreachable));
                    if self.eval_mode {
                        let return_value = if let Some(value) = self.eval_completion {
                            value
                        } else {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let undef_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: undef_val,
                                    constant: undef_const,
                                },
                            );
                            undef_val
                        };
                        self.current_function.set_terminator(
                            block,
                            Terminator::Return {
                                value: Some(return_value),
                            },
                        );
                    } else if is_unreachable {
                        self.current_function
                            .set_terminator(block, Terminator::Return { value: None });
                    }
                }
            }
            StmtFlow::Terminated => {}
        }

        if has_tla {
            self.finalize_async_main()?;
        } else {
            let has_eval = self.current_function.has_eval();
            let blocks = self.current_function.into_blocks();
            let mut function = Function::new("main", BasicBlockId(0));
            function.set_has_eval(has_eval);
            if self.eval_mode {
                function.set_params(vec![EVAL_SCOPE_ENV_PARAM.to_string()]);
            }
            for block in blocks {
                function.push_block(block);
            }
            self.module.push_function(function);
        }
        Ok(self.module)
    }

}
