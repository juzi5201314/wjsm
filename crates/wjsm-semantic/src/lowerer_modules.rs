use super::*;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, Instruction, MODULE_ENTRY_IR_NAME, Program,
    Terminator,
};

#[derive(Debug, Clone)]
pub struct ModuleLoweringInput {
    pub id: wjsm_ir::ModuleId,
    pub ast: swc_ast::Module,
    pub metadata: ModuleMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetadata {
    pub filename: String,
    pub dirname: String,
    pub url: String,
    pub kind: ModuleKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Esm,
    CommonJs,
}

/// 将多个模块编译为单一的 IR Program（模块 bundling）
///
/// # 参数
/// - `modules`: 模块列表，包含 ModuleId、AST 与编译期路径元数据
/// - `import_map`: 导入映射，module_id → ImportBinding 列表
/// - `dynamic_import_targets`: 动态 import() 目标映射，module_id → 被动态 import 的目标模块 ID 列表
/// - `export_names`: 导出名称映射，module_id → 导出名集合
/// - `dynamic_import_specifiers`: 动态 import() specifier 映射，module_id → [(specifier, 目标 ModuleId)]
pub fn lower_modules(
    modules: Vec<ModuleLoweringInput>,
    import_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    dynamic_import_targets: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    export_names: &std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    dynamic_import_specifiers: &std::collections::HashMap<
        wjsm_ir::ModuleId,
        Vec<(String, wjsm_ir::ModuleId)>,
    >,
    re_export_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>,
) -> Result<Program, LoweringError> {
    // 多模块编译路径
    // 早错误：对每个模块运行私有名静态校验（与单模块路径一致）。
    for module in &modules {
        lowerer_classes_ts::validate_private_names(&module.ast)?;
    }

    let module_metadata = modules
        .iter()
        .map(|module| (module.id, module.metadata.clone()))
        .collect();
    let mut lowerer = setup_multi_module_lowerer(
        module_metadata,
        import_map,
        dynamic_import_targets,
        export_names,
        dynamic_import_specifiers,
        re_export_map,
    )?;

    predeclare_module_exports(&mut lowerer, &modules)?;

    let has_tla = modules
        .iter()
        .any(|module| has_top_level_await(&module.ast));
    let entry = init_entry_block(&mut lowerer, has_tla, &modules)?;

    lowerer.emit_hoisted_var_initializers(entry);
    emit_global_constants(&mut lowerer, entry);
    create_namespace_objects(&mut lowerer, entry);

    apply_re_export_map(&mut lowerer)?;
    let _flow = process_import_aliases(&mut lowerer, &modules, StmtFlow::Open(entry))?;

    let flow = lower_module_bodies(&mut lowerer, &modules)?;

    finalize_multi_module(&mut lowerer, flow, has_tla)?;

    Ok(lowerer.module)
}

/// 设置多模块 lowerer 的初始状态
fn setup_multi_module_lowerer(
    module_metadata: std::collections::HashMap<wjsm_ir::ModuleId, ModuleMetadata>,
    import_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    dynamic_import_targets: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    export_names: &std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    dynamic_import_specifiers: &std::collections::HashMap<
        wjsm_ir::ModuleId,
        Vec<(String, wjsm_ir::ModuleId)>,
    >,
    re_export_map: &std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>,
) -> Result<Lowerer, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.import_bindings = import_map.clone();
    lowerer.dynamic_import_targets = dynamic_import_targets.clone();
    lowerer.module_export_names = export_names.clone();
    lowerer.re_export_map = re_export_map.clone();
    lowerer.module_metadata = module_metadata;

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

fn predeclare_cjs_path_bindings(
    lowerer: &mut Lowerer,
    _module_id: wjsm_ir::ModuleId,
) -> Result<(), LoweringError> {
    for name in ["__filename", "__dirname"] {
        lowerer
            .scopes
            .declare(name, VarKind::Const, true)
            .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
    }
    Ok(())
}

/// 预扫描：为所有模块的变量声明创建作用域条目，并声明 default export 变量
fn predeclare_module_exports(
    lowerer: &mut Lowerer,
    modules: &[ModuleLoweringInput],
) -> Result<(), LoweringError> {
    for module in modules {
        let module_id = module.id;
        let module_ast = &module.ast;
        lowerer.current_module_id = Some(module_id);
        // 为每个模块的顶层声明创建独立的块级作用域（#43）：
        // 避免两个模块同名的顶层 let/const 落入同一根作用域导致 "cannot redeclare"。
        // 使用 Block 而非 Function 作用域——模块体最终全部降级进同一个 $module_main 函数，
        // 若用 Function 作用域会让 binding_owner_function_scope ≠ current_function_scope_id，
        // 破坏捕获/共享 env 的路由判断（live binding 依赖该机制）。
        lowerer.scopes.push_scope(ScopeKind::Block);
        let module_scope = lowerer.scopes.current_scope_id();
        lowerer.module_scopes.insert(module_id, module_scope);
        predeclare_cjs_path_bindings(lowerer, module_id)?;
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
                        .insert((module_id, "default".to_string()), ir_name);
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
                        .insert((module_id, "default".to_string()), ir_name);
                }
                swc_ast::ModuleItem::ModuleDecl(swc_ast::ModuleDecl::ExportDecl(export_decl)) => {
                    let names = decl_exported_names(&export_decl.decl);
                    for name in names {
                        // 用 resolve_scope_id 而非 lookup：const 在预声明阶段处于 TDZ（未初始化），
                        // lookup 会失败；此处只需作用域 id 以登记 export_map（#44）。
                        if let Ok(scope_id) = lowerer.scopes.resolve_scope_id(&name) {
                            let ir_name = format!("${scope_id}.{name}");
                            lowerer.export_map.insert((module_id, name), ir_name);
                        }
                    }
                }
                swc_ast::ModuleItem::ModuleDecl(swc_ast::ModuleDecl::ExportNamed(named))
                    if named.src.is_none() =>
                {
                    lower_export_named(lowerer, named);
                }
                _ => {}
            }
        }
        // 退回根作用域，准备处理下一个模块。
        lowerer.scopes.pop_scope();
    }
    Ok(())
}

/// 根据 `re_export_map` 将重导出写入 `export_map`（在模块体执行之前，与本地 export 预注册配合）。
fn apply_re_export_map(lowerer: &mut Lowerer) -> Result<(), LoweringError> {
    let re_export_map = lowerer.re_export_map.clone();
    for (module_id, bindings) in re_export_map {
        for binding in bindings {
            if binding.local_name.is_none() && binding.exported_name.is_none() {
                let source_mid = binding.source_module;
                let keys: Vec<(wjsm_ir::ModuleId, String)> = lowerer
                    .export_map
                    .keys()
                    .filter(|(mid, _)| *mid == source_mid)
                    .cloned()
                    .collect();
                for (src_mid, export_name) in keys {
                    if export_name == "default" {
                        continue;
                    }
                    if let Some(ir_name) = lowerer.export_map.get(&(src_mid, export_name.clone())) {
                        lowerer
                            .export_map
                            .insert((module_id, export_name), ir_name.clone());
                    }
                }
            } else if let (Some(_local), Some(exported), Some(ir_name)) = (
                binding.local_name.as_ref(),
                binding.exported_name.as_ref(),
                binding
                    .local_name
                    .as_ref()
                    .and_then(|local| resolve_export_ir(lowerer, binding.source_module, local)),
            ) {
                lowerer
                    .export_map
                    .insert((module_id, exported.clone()), ir_name);
            }
        }
    }
    Ok(())
}

/// 解析模块导出名对应的 IR 变量（含 `export_map` 与重导出链）。
fn resolve_export_ir(
    lowerer: &Lowerer,
    module_id: wjsm_ir::ModuleId,
    export_name: &str,
) -> Option<String> {
    if let Some(ir) = lowerer
        .export_map
        .get(&(module_id, export_name.to_string()))
        .cloned()
    {
        return Some(ir);
    }
    if let Some(bindings) = lowerer.re_export_map.get(&module_id) {
        for binding in bindings {
            if let Some(local) = binding.local_name.as_ref() {
                let exported = binding.exported_name.as_deref().unwrap_or(local.as_str());
                if exported == export_name {
                    return resolve_export_ir(lowerer, binding.source_module, local);
                }
            }
        }
    }
    if let Ok(scope_id) = lowerer.scopes.resolve_scope_id(export_name) {
        return Some(format!("${scope_id}.{export_name}"));
    }
    None
}

/// 处理 import 声明：绑定别名、默认导入与命名空间导入。
fn process_import_aliases(
    lowerer: &mut Lowerer,
    modules: &[ModuleLoweringInput],
    mut flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    for module in modules {
        let module_id = module.id;
        // 进入导入方模块自己的作用域（#43/#44）：命名空间 local 与别名都属于该模块，
        // 不能落入根作用域，否则跨模块同名 import 会互相覆盖。
        let Some(&module_scope) = lowerer.module_scopes.get(&module_id) else {
            continue;
        };
        lowerer.scopes.enter_scope(module_scope);
        let bindings: Vec<_> = lowerer
            .import_bindings
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        for binding in bindings {
            for (local_name, imported_name) in &binding.names {
                if imported_name == "*" {
                    lowerer
                        .scopes
                        .declare(local_name, VarKind::Const, true)
                        .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
                    let block = lowerer.ensure_open(flow)?;
                    let export_names_set = lowerer
                        .module_export_names
                        .get(&binding.source_module)
                        .cloned();
                    let capacity = export_names_set.as_ref().map_or(0, |s| s.len()) + 1;
                    let ns_obj = lowerer.alloc_value();
                    lowerer.current_function.append_instruction(
                        block,
                        Instruction::NewObject {
                            dest: ns_obj,
                            capacity: capacity as u32,
                        },
                    );
                    let (scope_id, _) = lowerer
                        .scopes
                        .lookup(local_name)
                        .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
                    let ir_name = format!("${scope_id}.{local_name}");
                    lowerer.current_function.append_instruction(
                        block,
                        Instruction::StoreVar {
                            name: ir_name,
                            value: ns_obj,
                        },
                    );
                    lowerer
                        .static_namespace_import_objects
                        .insert((module_id, local_name.clone()), ns_obj);
                    lowerer.static_namespace_import_sources.push((
                        module_id,
                        local_name.clone(),
                        binding.source_module,
                    ));
                    flow = StmtFlow::Open(block);
                    continue;
                }
                if imported_name == "default" {
                    if let Some(source_ir_name) =
                        resolve_export_ir(lowerer, binding.source_module, "default")
                    {
                        lowerer
                            .import_aliases
                            .insert((module_id, local_name.clone()), source_ir_name);
                    }
                    continue;
                }
                if let Some(source_ir_name) =
                    resolve_export_ir(lowerer, binding.source_module, imported_name)
                {
                    lowerer
                        .import_aliases
                        .insert((module_id, local_name.clone()), source_ir_name);
                }
            }
        }
        lowerer.scopes.pop_scope();
    }
    Ok(flow)
}

/// 初始化入口块（支持 TLA）
fn init_entry_block(
    lowerer: &mut Lowerer,
    has_tla: bool,
    modules: &[ModuleLoweringInput],
) -> Result<BasicBlockId, LoweringError> {
    if has_tla {
        // 取第一个模块的 span 用于错误报告
        let first_span = modules
            .first()
            .map(|module| module.ast.span)
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

    // 创建全局对象，用于 bundled module 中的 builtin global 解析。
    let global_obj = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        entry,
        Instruction::CallBuiltin {
            dest: Some(global_obj),
            builtin: Builtin::CreateGlobalObject,
            args: vec![],
        },
    );
    lowerer.current_function.append_instruction(
        entry,
        Instruction::StoreVar {
            name: "$0.$global".to_string(),
            value: global_obj,
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

fn emit_cjs_path_bindings(
    lowerer: &mut Lowerer,
    module_id: wjsm_ir::ModuleId,
    block: BasicBlockId,
) -> Result<(), LoweringError> {
    let Some(metadata) = lowerer.module_metadata.get(&module_id).cloned() else {
        return Ok(());
    };
    let Some(&module_scope) = lowerer.module_scopes.get(&module_id) else {
        return Ok(());
    };

    emit_module_path_binding(
        lowerer,
        block,
        module_scope,
        "__filename",
        metadata.filename,
    );
    emit_module_path_binding(lowerer, block, module_scope, "__dirname", metadata.dirname);
    Ok(())
}

fn emit_module_path_binding(
    lowerer: &mut Lowerer,
    block: BasicBlockId,
    module_scope: usize,
    name: &str,
    value: String,
) {
    let string_const = lowerer.module.add_constant(Constant::String(value));
    let string_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: string_val,
            constant: string_const,
        },
    );
    lowerer.current_function.append_instruction(
        block,
        Instruction::StoreVar {
            name: format!("${module_scope}.{name}"),
            value: string_val,
        },
    );
}

/// 处理每个模块的 body（语句、导出声明、默认导出等）
fn lower_module_bodies(
    lowerer: &mut Lowerer,
    modules: &[ModuleLoweringInput],
) -> Result<StmtFlow, LoweringError> {
    let entry_block = lowerer.current_function.last_block_id();
    let mut flow = StmtFlow::Open(entry_block);
    for module in modules {
        let module_id = module.id;
        let module_ast = &module.ast;
        lowerer.current_module_id = Some(module_id);
        // 进入该模块的顶层作用域（#43）：模块体中的标识符解析必须命中模块自己的作用域，
        // 而非根作用域，否则同名顶层变量会跨模块互相解析错位。
        if let Some(&module_scope) = lowerer.module_scopes.get(&module_id) {
            lowerer.scopes.enter_scope(module_scope);
        }
        if let StmtFlow::Open(block) = flow {
            emit_cjs_path_bindings(lowerer, module_id, block)?;
        }
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
        // 模块体执行完毕后，为以本模块为来源的命名空间对象安装 live binding getter（#45）。
        // 拓扑序保证来源模块先于导入方降级，此时本模块的导出绑定与捕获闭包均已就绪。
        flow = install_live_namespace_getters_for_source(lowerer, module_id, flow)?;
        lowerer.scopes.pop_scope();
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
            let flow = lowerer.lower_stmt(&swc_ast::Stmt::Decl(export_decl.decl.clone()), flow)?;
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
fn lower_export_named(lowerer: &mut Lowerer, named_export: &swc_ast::NamedExport) {
    let current_mid = lowerer.current_module_id.unwrap_or(wjsm_ir::ModuleId(0));
    if named_export.src.is_none() {
        // 本地导出：export { x } / export { x as y }
        for spec in &named_export.specifiers {
            if let swc_ast::ExportSpecifier::Named(named) = spec {
                let local_name = match &named.orig {
                    swc_ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
                    swc_ast::ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                };
                let exported_name = named.exported.as_ref().map_or_else(
                    || local_name.clone(),
                    |e| match e {
                        swc_ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                    },
                );
                // resolve_scope_id 而非 lookup：预声明阶段 local 可能处于 TDZ（const），
                // 此处只需作用域 id 登记 export_map（#44）。
                if let Ok(scope_id) = lowerer.scopes.resolve_scope_id(&local_name) {
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

impl Lowerer {
    /// 为命名空间对象 `ns_obj` 的导出 `export_name` 安装一个 live binding getter（#45）。
    ///
    /// getter 是一个捕获来源模块导出绑定的闭包：每次读取 `ns.export_name` 时通过
    /// 闭包 env 读取该绑定的最新值，从而满足 ECMAScript §10.4.6 模块命名空间对象的
    /// live binding 语义（导出变量被改写后 `ns.x` 反映新值）。
    ///
    /// 必须在来源模块体降级完成后调用：此时导出绑定与其捕获闭包（若存在）均已物化，
    /// `ensure_shared_env` 不会重复快照。
    fn install_namespace_getter(
        &mut self,
        ns_obj: wjsm_ir::ValueId,
        export_name: &str,
        source_ir_name: &str,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // 将 `${scope_id}.{name}` 形式的 IR 变量名解析回 CapturedBinding，
        // 以便 getter 通过既有的捕获/共享 env 机制读取 live 值。
        let binding = parse_ir_name_to_binding(source_ir_name);
        let getter_fn_id = self.build_namespace_getter_fn(&binding)?;

        // 在外层（$module_main）发射 getter 闭包：捕获来源绑定。
        let func_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(getter_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );
        let mut current_block = block;
        let env_val =
            self.ensure_shared_env(current_block, std::slice::from_ref(&binding), DUMMY_SP)?;
        current_block = self.resolve_store_block(current_block);
        let getter_val = self.alloc_value();
        self.current_function.append_instruction(
            current_block,
            Instruction::CallBuiltin {
                dest: Some(getter_val),
                builtin: Builtin::CreateClosure,
                args: vec![func_ref_val, env_val],
            },
        );

        // 构建访问器 descriptor { get: closure, enumerable: true, configurable: false }
        // 并通过 DefineProperty 安装到命名空间对象上。
        let desc = self.build_descriptor("get", getter_val, true, false, current_block)?;
        let key_const = self
            .module
            .add_constant(Constant::String(export_name.to_string()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            current_block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        self.current_function.append_instruction(
            current_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::DefineProperty,
                args: vec![ns_obj, key_val, desc],
            },
        );
        Ok(current_block)
    }

    /// 构建命名空间 live binding getter 的 IR 函数：函数体读取捕获绑定并返回其值。
    /// 返回 FunctionId（getter 通过 CreateClosure 绑定来源 env）。
    fn build_namespace_getter_fn(
        &mut self,
        binding: &CapturedBinding,
    ) -> Result<wjsm_ir::FunctionId, LoweringError> {
        let fn_name = format!("$ns_getter.{}", binding.env_key());
        self.push_function_context(&fn_name, BasicBlockId(0));
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        // 函数体：通过 env 读取来源绑定（getter 不属于绑定所有者函数，
        // load_captured_binding 走 record_capture + GetProp 路径），然后返回。
        let entry = BasicBlockId(0);
        let value_val = self.load_captured_binding(entry, binding)?;
        let ret_block = self.resolve_store_block(entry);
        self.current_function.set_terminator(
            ret_block,
            Terminator::Return {
                value: Some(value_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&fn_name, BasicBlockId(0));
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let fn_id = self.module.push_function(ir_function);
        self.pop_function_context();
        Ok(fn_id)
    }
}

/// 将 `${scope_id}.{name}` 形式的 IR 变量名解析回 `CapturedBinding`。
/// 顶层导出绑定恒为该形式（见 export_map 写入处）。
pub(crate) fn parse_ir_name_to_binding(ir_name: &str) -> CapturedBinding {
    if let Some(rest) = ir_name.strip_prefix('$')
        && let Some((scope_str, name)) = rest.split_once('.')
        && let Ok(scope_id) = scope_str.parse::<usize>()
    {
        return CapturedBinding::new(name.to_string(), scope_id);
    }
    // 理论不可达：导出绑定恒为 `${scope_id}.{name}`。
    CapturedBinding::new(ir_name.to_string(), 0)
}

/// 为某来源模块 `source_module_id` 关联的所有命名空间对象安装 live binding getter（#45）。
///
/// 在该模块体降级完成后调用（拓扑序保证来源先于导入方）。覆盖两类命名空间对象：
/// - 静态 `import * as ns`：以本模块为来源的全部 `ns` 局部对象；
/// - 动态 `import()`：以本模块为目标的命名空间对象。
///
/// 每个导出名安装一个 getter 访问器，getter 读取来源模块的导出绑定（经由捕获/共享 env
/// 机制返回最新值），从而满足 ECMAScript §10.4.6 命名空间对象的 live binding 语义。
fn install_live_namespace_getters_for_source(
    lowerer: &mut Lowerer,
    source_module_id: wjsm_ir::ModuleId,
    flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    let StmtFlow::Open(mut block) = flow else {
        return Ok(flow);
    };

    // 收集本模块作为来源的全部命名空间对象（静态 import * as + 动态 import()）。
    let mut targets: Vec<wjsm_ir::ValueId> = Vec::new();
    for (importer_mid, local, src_mid) in &lowerer.static_namespace_import_sources {
        if *src_mid == source_module_id
            && let Some(&ns_obj) = lowerer
                .static_namespace_import_objects
                .get(&(*importer_mid, local.clone()))
        {
            targets.push(ns_obj);
        }
    }
    if let Some(&ns_obj) = lowerer
        .dynamic_import_namespace_objects
        .get(&source_module_id)
    {
        targets.push(ns_obj);
    }
    if targets.is_empty() {
        return Ok(StmtFlow::Open(block));
    }

    // 解析本模块全部导出名 → 来源 IR 变量名（按名排序，保证确定性输出）。
    let mut exports: Vec<(String, String)> = Vec::new();
    if let Some(names) = lowerer.module_export_names.get(&source_module_id).cloned() {
        for export_name in &names {
            if let Some(ir_name) = resolve_export_ir(lowerer, source_module_id, export_name) {
                exports.push((export_name.clone(), ir_name));
            }
        }
    }

    for ns_obj in targets {
        for (export_name, source_ir_name) in &exports {
            block = lowerer.install_namespace_getter(ns_obj, export_name, source_ir_name, block)?;
        }
        set_namespace_string_tag(lowerer, ns_obj, block);
    }
    Ok(StmtFlow::Open(block))
}

/// 为命名空间对象设置 `Symbol.toStringTag = "Module"`（ECMAScript §10.4.6.2）。
fn set_namespace_string_tag(lowerer: &mut Lowerer, ns_obj: wjsm_ir::ValueId, block: BasicBlockId) {
    let tag_key = lowerer
        .module
        .add_constant(Constant::String("Symbol.toStringTag".to_string()));
    let tag_key_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
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
        block,
        Instruction::Const {
            dest: tag_value_val,
            constant: tag_value,
        },
    );
    lowerer.current_function.append_instruction(
        block,
        Instruction::SetProp {
            object: ns_obj,
            key: tag_key_val,
            value: tag_value_val,
        },
    );
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
