use super::*;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, Instruction, MODULE_ENTRY_IR_NAME, Program,
    Terminator,
};

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
        return lower_module(module, false);
    }

    // 多模块编译路径
    // 早错误：对每个模块运行私有名静态校验（与单模块路径一致）。
    for (_, module_ast) in &modules {
        lowerer_classes_ts::validate_private_names(module_ast)?;
    }

    let mut lowerer = setup_multi_module_lowerer(
        import_map,
        dynamic_import_targets,
        export_names,
        dynamic_import_specifiers,
    )?;

    predeclare_module_exports(&mut lowerer, &modules)?;
    process_import_aliases(&mut lowerer, &modules)?;

    let has_tla = modules.iter().any(|(_, m)| has_top_level_await(m));
    let entry = init_entry_block(&mut lowerer, has_tla, &modules)?;

    lowerer.emit_hoisted_var_initializers(entry);
    emit_global_constants(&mut lowerer, entry);
    create_namespace_objects(&mut lowerer, entry);

    let flow = lower_module_bodies(&mut lowerer, &modules)?;
    fill_namespace_properties(&mut lowerer, flow)?;

    finalize_multi_module(&mut lowerer, flow, has_tla)?;

    Ok(lowerer.module)
}

/// 设置多模块 lowerer 的初始状态
fn setup_multi_module_lowerer(
    import_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    dynamic_import_targets: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    export_names: &std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    dynamic_import_specifiers: &std::collections::HashMap<
        wjsm_ir::ModuleId,
        Vec<(String, wjsm_ir::ModuleId)>,
    >,
) -> Result<Lowerer, LoweringError> {
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
    Ok(lowerer)
}

/// 预扫描：为所有模块的变量声明创建作用域条目，并声明 default export 变量
fn predeclare_module_exports(
    lowerer: &mut Lowerer,
    modules: &[(wjsm_ir::ModuleId, swc_ast::Module)],
) -> Result<(), LoweringError> {
    for (module_id, module_ast) in modules {
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
    Ok(())
}

/// 处理 import 声明：为别名导入和默认导入建立映射
fn process_import_aliases(
    lowerer: &mut Lowerer,
    modules: &[(wjsm_ir::ModuleId, swc_ast::Module)],
) -> Result<(), LoweringError> {
    for (module_id, module_ast) in modules {
        let bindings = lowerer.import_bindings.get(module_id);
        let Some(bindings) = bindings else { continue };
        for binding in bindings {
            for (local_name, imported_name) in &binding.names {
                if imported_name == "*" {
                    // 命名空间导入（import * as ns from '...'）暂不支持
                    return Err(LoweringError::Diagnostic(Diagnostic::new(
                        0,
                        0,
                        "namespace import (import * as ...) is not yet supported".to_string(),
                    )));
                }
                if imported_name == "default" {
                    if let Some(source_ir_name) = lowerer
                        .export_map
                        .get(&(binding.source_module, "default".to_string()))
                        && local_name != "default"
                    {
                        lowerer
                            .import_aliases
                            .insert(local_name.clone(), source_ir_name.clone());
                    }
                    continue;
                }
                if local_name != imported_name
                    && let Ok(scope_id) = lowerer.scopes.resolve_scope_id(imported_name)
                {
                    let source_ir_name = format!("${scope_id}.{imported_name}");
                    lowerer
                        .import_aliases
                        .insert(local_name.clone(), source_ir_name);
                }
            }
        }
        let _ = module_ast;
    }
    Ok(())
}

/// 初始化入口块（支持 TLA）
fn init_entry_block(
    lowerer: &mut Lowerer,
    has_tla: bool,
    modules: &[(wjsm_ir::ModuleId, swc_ast::Module)],
) -> Result<BasicBlockId, LoweringError> {
    if has_tla {
        // 取第一个模块的 span 用于错误报告
        let first_span = modules
            .first()
            .map(|(_, m)| m.span)
            .unwrap_or(swc_core::common::DUMMY_SP);
        lowerer.init_async_main_context(first_span)
    } else {
        Ok(BasicBlockId(0))
    }
}

/// 初始化全局内置变量（undefined, NaN, Infinity）
fn emit_global_constants(lowerer: &mut Lowerer, entry: BasicBlockId) {
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
}

/// 为动态 import 的目标模块创建并注册命名空间对象
fn create_namespace_objects(lowerer: &mut Lowerer, entry: BasicBlockId) {
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

/// 处理每个模块的 body（语句、导出声明、默认导出等）
fn lower_module_bodies(
    lowerer: &mut Lowerer,
    modules: &[(wjsm_ir::ModuleId, swc_ast::Module)],
) -> Result<StmtFlow, LoweringError> {
    let entry_block = lowerer.current_function.last_block_id();
    let mut flow = StmtFlow::Open(entry_block);
    for (module_id, module_ast) in modules {
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
                    flow = lower_module_decl(lowerer, decl, flow)?;
                }
            }
        }
    }
    Ok(flow)
}

/// 处理单个模块声明（export decl / export default / import / export named）
fn lower_module_decl(
    lowerer: &mut Lowerer,
    decl: &swc_ast::ModuleDecl,
    flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    match decl {
        // export const/let/var/function/class → 将内层声明作为普通语句处理
        swc_ast::ModuleDecl::ExportDecl(export_decl) => {
            let flow = lowerer.lower_stmt(
                &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                flow,
            )?;
            // 将导出名注册到 export_map
            let current_mid = lowerer.current_module_id.unwrap_or(wjsm_ir::ModuleId(0));
            let names = decl_exported_names(&export_decl.decl);
            for name in names {
                if let Ok((scope_id, _)) = lowerer.scopes.lookup(&name) {
                    let ir_name = format!("${scope_id}.{name}");
                    lowerer.export_map.insert((current_mid, name), ir_name);
                }
            }
            Ok(flow)
        }
        // export default expr → 计算表达式并存储到 _default_export_mod{id} 变量
        swc_ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
            lower_export_default_expr(lowerer, default_expr, flow)
        }
        // export default function/class → 将声明作为普通语句处理并存储到变量
        swc_ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
            lower_export_default_decl(lowerer, default_decl, flow)
        }
        // import 声明 → 单模块模式下跳过
        swc_ast::ModuleDecl::Import(_) => {
            // 暂时跳过 import（依赖已由 bundler 预处理）
            Ok(flow)
        }
        // export { x } / export { x as y } → 将导出名注册到 export_map
        swc_ast::ModuleDecl::ExportNamed(named_export) => {
            lower_export_named(lowerer, named_export);
            Ok(flow)
        }
        // export * from → 暂时跳过
        _ => {
            // 暂不处理 re-exports
            Ok(flow)
        }
    }
}

/// 处理 `export default <expr>`
fn lower_export_default_expr(
    lowerer: &mut Lowerer,
    default_expr: &swc_ast::ExportDefaultExpr,
    flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
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
    Ok(StmtFlow::Open(outer_block))
}

/// 处理 `export default function/class`
fn lower_export_default_decl(
    lowerer: &mut Lowerer,
    default_decl: &swc_ast::ExportDefaultDecl,
    flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    match &default_decl.decl {
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
            if let Some(current_mid) = lowerer.current_module_id
                && let Some(ir_name) = lowerer
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
            Ok(StmtFlow::Open(outer_block))
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
            if let Some(current_mid) = lowerer.current_module_id
                && let Some(ir_name) = lowerer
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
            Ok(StmtFlow::Open(outer_block))
        }
        _ => Ok(flow),
    }
}

/// 处理 `export { x }` / `export { x as y }`
fn lower_export_named(
    lowerer: &mut Lowerer,
    named_export: &swc_ast::NamedExport,
) {
    let current_mid = lowerer.current_module_id.unwrap_or(wjsm_ir::ModuleId(0));
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
                if let Ok((scope_id, _)) = lowerer.scopes.lookup(&local_name) {
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

/// 为动态 import 的命名空间对象填充属性
///
/// TODO: 当前实现为一次性快照语义（SetProp 后不再更新），不符合 ES Module live binding 规范。
/// 根据规范，命名空间属性必须是 live binding：ns.x 应反映导出变量的最新值。
/// 完整修复需要 IR 层支持 getter 或在 StoreVar 时同步更新命名空间属性。
/// 这属于较大特性，需要 IR 层变更后才能实现。
fn fill_namespace_properties(
    lowerer: &mut Lowerer,
    flow: StmtFlow,
) -> Result<(), LoweringError> {
    let StmtFlow::Open(ns_block) = flow else {
        return Ok(());
    };
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
    Ok(())
}

/// 完成 main 函数构建（处理 TLA 或普通返回）
fn finalize_multi_module(
    lowerer: &mut Lowerer,
    flow: StmtFlow,
    has_tla: bool,
) -> Result<(), LoweringError> {
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
        let known_callees = lowerer.current_function.take_known_callee_vars();
        let blocks = lowerer.current_function.take_blocks();
        let mut function = Function::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0));
        function.set_has_eval(has_eval);
        for (ir_name, fn_id) in known_callees {
            function.record_known_callee(ir_name, fn_id);
        }
        for block in blocks {
            function.push_block(block);
        }
        lowerer.module.push_function(function);
    }

    Ok(())
}
