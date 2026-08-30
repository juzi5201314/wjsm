use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use super::*;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, FunctionId, ImportBinding, Instruction,
    MODULE_ENTRY_IR_NAME, ModuleId, Program, ReExportBinding, Terminator,
};

/// 多模块 lowering 的链接图：import / 动态 import / 导出 / 重导出。
///
/// 这五张表总是一起传入 [`lower_modules`] 及其变体，收成一个借用视图避免
/// 位置参数错位，并消掉 `too_many_arguments`。
#[derive(Clone, Copy, Debug)]
pub struct ModuleLinking<'a> {
    pub import_map: &'a HashMap<ModuleId, Vec<ImportBinding>>,
    pub dynamic_import_targets: &'a HashMap<ModuleId, Vec<ModuleId>>,
    pub export_names: &'a HashMap<ModuleId, BTreeSet<String>>,
    pub dynamic_import_specifiers: &'a HashMap<ModuleId, Vec<(String, ModuleId)>>,
    pub re_export_map: &'a HashMap<ModuleId, Vec<ReExportBinding>>,
}

impl ModuleLinking<'static> {
    /// 空链接图：单模块、无 import/export 边的测试与 agent 脚本路径。
    pub fn empty() -> Self {
        static IMPORT_MAP: LazyLock<HashMap<ModuleId, Vec<ImportBinding>>> =
            LazyLock::new(HashMap::new);
        static DYNAMIC_IMPORT_TARGETS: LazyLock<HashMap<ModuleId, Vec<ModuleId>>> =
            LazyLock::new(HashMap::new);
        static EXPORT_NAMES: LazyLock<HashMap<ModuleId, BTreeSet<String>>> =
            LazyLock::new(HashMap::new);
        static DYNAMIC_IMPORT_SPECIFIERS: LazyLock<HashMap<ModuleId, Vec<(String, ModuleId)>>> =
            LazyLock::new(HashMap::new);
        static RE_EXPORT_MAP: LazyLock<HashMap<ModuleId, Vec<ReExportBinding>>> =
            LazyLock::new(HashMap::new);
        Self {
            import_map: &IMPORT_MAP,
            dynamic_import_targets: &DYNAMIC_IMPORT_TARGETS,
            export_names: &EXPORT_NAMES,
            dynamic_import_specifiers: &DYNAMIC_IMPORT_SPECIFIERS,
            re_export_map: &RE_EXPORT_MAP,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleLoweringInput {
    pub id: wjsm_ir::ModuleId,
    pub ast: swc_ast::Module,
    pub metadata: ModuleMetadata,
    /// 源码文本（可选）：`emit_debug_checks` 时用于行/列映射。
    pub source: Option<std::sync::Arc<str>>,
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

/// 多模块 lowering 的布局元数据（builtin 段缓存 / hydration 用）。
///
/// 这是 lowerer 内部状态（`Lowerer::export_map` / `Lowerer::module_scopes` 与
/// `ScopeTree` 作用域总数）的最终快照；`lower_modules_with_debug` 只返回 `Program`，
/// 不包含这些布局信息。builtin 段缓存需要它们重建 [`BuiltinSegment`]。
#[derive(Debug, Clone)]
pub struct LoweringMetadata {
    /// 模块导出名 → IR 变量名（`export_map` 最终快照）。
    pub export_map: std::collections::HashMap<(ModuleId, String), String>,
    /// 每模块顶层作用域 id（`module_scopes` 最终快照）。
    pub module_scopes: std::collections::HashMap<ModuleId, usize>,
    /// 本段 lower 结束时 ScopeTree 总作用域数（含 root）；供用户段做 scope 基址。
    pub scope_count: usize,
    /// 本段入口已创建并注册命名空间对象的模块集合（`namespace_object_modules`
    /// 最终快照，升序确定）。hydration 时用户段对这些模块取回同一对象而非重建。
    pub namespace_modules: std::collections::BTreeSet<ModuleId>,
}

/// builtin 段（hydration 输入；由独立 lowerer 缓存为完整 Program 的 builtin 依赖闭包
/// 及其布局元数据）。用户 lowerer 以它为种子启动：预装函数/常量（段函数在前）、
/// 预置占位作用域（scope 基址）、注入 export_map/module_export_names/module_scopes，
/// 最后在用户 `$module_main` 入口块首条发射对段入口的调用。
#[derive(Debug, Clone)]
pub struct BuiltinSegment {
    /// builtin 段完整 Program（函数+常量），入口函数名为 `$builtin_main`。
    pub program: Program,
    /// builtin 段 lower 时的总 scope 数（含 root）→ 用户 lowerer 的 scope 基址。
    pub scope_count: u32,
    /// 段内入口函数（= 段内最后一个函数，由 finalize 最后 push 的 `$builtin_main`）。
    pub entry_function_id: FunctionId,
    /// 模块导出名 → IR 变量名。
    pub export_map: std::collections::HashMap<(ModuleId, String), String>,
    /// 每模块导出名集合。
    pub module_export_names:
        std::collections::HashMap<ModuleId, std::collections::BTreeSet<String>>,
    /// builtin 每模块顶层 scope id（值均 < `scope_count`）。
    pub module_scopes: std::collections::HashMap<ModuleId, usize>,
    /// 段入口 `$builtin_main` 已创建并注册命名空间对象的模块集合（builtin 之间
    /// `import * as` / 段内动态 import 目标）。用户段对这些模块经
    /// GetModuleNamespace 取回同一对象。
    pub namespace_modules: std::collections::BTreeSet<ModuleId>,
}

/// 将多个模块编译为单一的 IR Program（模块 bundling）
///
/// # 参数
/// - `modules`: 模块列表，包含 ModuleId、AST 与编译期路径元数据
/// - `linking`: import / 动态 import / 导出 / 重导出链接图
pub fn lower_modules(
    modules: Vec<ModuleLoweringInput>,
    linking: ModuleLinking<'_>,
) -> Result<Program, LoweringError> {
    lower_modules_with_debug(modules, linking, false)
}

/// 多模块 lowering；`emit_debug_checks` 为 true 时在各模块 body 语句入口插桩。
pub fn lower_modules_with_debug(
    modules: Vec<ModuleLoweringInput>,
    linking: ModuleLinking<'_>,
    emit_debug_checks: bool,
) -> Result<Program, LoweringError> {
    lower_modules_with_debug_meta(modules, linking, emit_debug_checks).map(|(program, _)| program)
}

/// 与 [`lower_modules_with_debug`] 相同，但额外返回 [`LoweringMetadata`]
/// （export_map / module_scopes / scope_count 最终快照）。
///
/// builtin 段缓存用：缓存的段程序需要这些布局信息才能在用户 lowerer 中重建
/// [`BuiltinSegment`]（scope 基址 + 导出解析）。
pub fn lower_modules_with_debug_meta(
    modules: Vec<ModuleLoweringInput>,
    linking: ModuleLinking<'_>,
    emit_debug_checks: bool,
) -> Result<(Program, LoweringMetadata), LoweringError> {
    // 多模块编译路径
    // 早错误：对每个模块运行私有名静态校验（与单模块路径一致）。
    for module in &modules {
        lowerer_classes_ts::validate_private_names(&module.ast)?;
    }

    let module_metadata = modules
        .iter()
        .map(|module| (module.id, module.metadata.clone()))
        .collect();
    let mut lowerer = setup_multi_module_lowerer(module_metadata, linking, 0)?;
    lowerer.emit_debug_checks = emit_debug_checks;

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

    let body_flow = StmtFlow::Open(lowerer.current_function.last_block_id());
    let body_flow = install_all_namespace_getters(&mut lowerer, body_flow)?;
    let flow = lower_module_bodies(&mut lowerer, &modules, body_flow)?;

    finalize_multi_module(&mut lowerer, flow, has_tla)?;

    let metadata = LoweringMetadata {
        export_map: lowerer.export_map.clone(),
        module_scopes: lowerer.module_scopes.clone(),
        scope_count: lowerer.scopes.scope_count(),
        namespace_modules: lowerer.namespace_object_modules.iter().copied().collect(),
    };
    Ok((lowerer.module, metadata))
}

/// 以 builtin 段为种子 lower 用户模块（hydration 入口）。
///
/// 与 [`lower_modules_with_debug_meta`] 流程一致，但：
/// - ScopeTree 以 `builtin.scope_count` 为基址（占位作用域 id 0..scope_count，
///   `push_scope` 从 scope_count 继续），保证 builtin 段 IR 变量名
///   `${scope_id}.{name}` 在合并程序中依然成立；
/// - builtin 段 Program 预装（段函数在前、用户函数在后）+ export_map /
///   module_export_names / module_scopes 注入；
/// - **用户 `$module_main` 入口块头部插入对 `$builtin_main` 的 Call**（builtin
///   顶层先于所有用户模块初始化执行）。`$builtin_main` 函数体保留在段函数里，
///   不再 inline。runtime 用共享 `variables` 表让 `$N.x` 跨 image 可见。
///   `$builtin_main` 顶层未捕获 throw 经这次 Call 的异常路径从 `$module_main` 传出。
///
/// `modules` 只含用户模块；`builtin` 段必须无 TLA（builtin 段构建时保证）。
/// 生产路径遇用户 TLA 会回退整包 lower，不会走进本函数。
pub fn lower_modules_with_builtin_seed(
    modules: Vec<ModuleLoweringInput>,
    linking: ModuleLinking<'_>,
    builtin: BuiltinSegment,
    emit_debug_checks: bool,
) -> Result<Program, LoweringError> {
    // 早错误：对每个模块运行私有名静态校验（与多模块路径一致）。
    for module in &modules {
        lowerer_classes_ts::validate_private_names(&module.ast)?;
    }

    let module_metadata = modules
        .iter()
        .map(|module| (module.id, module.metadata.clone()))
        .collect();
    let mut lowerer = setup_multi_module_lowerer(
        module_metadata,
        linking,
        usize::try_from(builtin.scope_count).expect("u32 scope_count 总能转为 usize"),
    )?;
    lowerer.emit_debug_checks = emit_debug_checks;
    lowerer.hydrate_builtin_segment(&builtin);

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

    // 统一序幕安装（用户 + builtin 来源）：builtin 段模块不在用户 `modules`
    // 列表里，但其命名空间来源已登记进 namespace_object_modules；运行时顺序
    // 上入口块头部的 `$builtin_main` 前缀调用先初始化 builtin 导出绑定，
    // 随后才执行这里发射的 getter 安装代码。
    let body_flow = StmtFlow::Open(lowerer.current_function.last_block_id());
    let body_flow = install_all_namespace_getters(&mut lowerer, body_flow)?;

    let flow = lower_module_bodies(&mut lowerer, &modules, body_flow)?;

    // builtin 顶层先执行：用户入口块头部 Call `$builtin_main`。
    emit_builtin_entry_call(&mut lowerer, &builtin);

    finalize_multi_module(&mut lowerer, flow, has_tla)?;

    Ok(lowerer.module)
}

/// 在用户 `$module_main`（或 TLA 的 async body entry）入口块头部插入
/// `Const FunctionRef($builtin_main)` + `Const Undefined` + `Call`。
///
/// 终止器不动。Call 的异常语义走现有 lowering：顶层未捕获 throw 变成这次
/// Call 的异常返回，再从 `$module_main` 传出。
fn emit_builtin_entry_call(lowerer: &mut Lowerer, builtin: &BuiltinSegment) {
    let entry_block = lowerer.async_main_body_entry.unwrap_or(BasicBlockId(0));
    let entry_block_idx = usize::try_from(entry_block.0).expect("BasicBlockId 索引在 usize 内");
    let callee = lowerer.alloc_value();
    let this_val = lowerer.alloc_value();
    let dest = lowerer.alloc_value();
    let fn_const = lowerer
        .module
        .add_constant(Constant::FunctionRef(builtin.entry_function_id));
    let undef_const = lowerer.module.add_constant(Constant::Undefined);
    let original =
        std::mem::take(lowerer.current_function.blocks[entry_block_idx].instructions_mut());
    let prefix = [
        Instruction::Const {
            dest: callee,
            constant: fn_const,
        },
        Instruction::Const {
            dest: this_val,
            constant: undef_const,
        },
        Instruction::Call {
            dest: Some(dest),
            callee,
            this_val,
            args: Vec::new(),
            callsite: None,
        },
    ];
    let instructions = lowerer.current_function.blocks[entry_block_idx].instructions_mut();
    instructions.extend(prefix);
    instructions.extend(original);
}

/// 设置多模块 lowerer 的初始状态
fn setup_multi_module_lowerer(
    module_metadata: HashMap<ModuleId, ModuleMetadata>,
    linking: ModuleLinking<'_>,
    base_scope_count: usize,
) -> Result<Lowerer, LoweringError> {
    let mut lowerer = Lowerer::with_base_scope_count(base_scope_count);
    lowerer.import_bindings = linking.import_map.clone();
    lowerer.dynamic_import_targets = linking.dynamic_import_targets.clone();
    lowerer.module_export_names = linking.export_names.clone();
    lowerer.re_export_map = linking.re_export_map.clone();
    lowerer.module_metadata = module_metadata;

    // 收集需要构建命名空间对象的模块：动态 import 目标 ∪ 静态 `import * as`
    // 来源。二者共享同一 canonical 对象（§10.4.6.12 GetModuleNamespace 缓存），
    // 静态命名空间局部与 import() 结果必须是同一对象身份。
    for targets in linking.dynamic_import_targets.values() {
        for &target_id in targets {
            lowerer.namespace_object_modules.insert(target_id);
        }
    }
    for bindings in linking.import_map.values() {
        for binding in bindings {
            if binding.names.iter().any(|(_, imported)| imported == "*") {
                lowerer
                    .namespace_object_modules
                    .insert(binding.source_module);
            }
        }
    }

    // 构建 specifier → ModuleId 映射（从动态 import specifier 列表构建，而非 import_map）
    for (module_id, spec_list) in linking.dynamic_import_specifiers.iter() {
        for (specifier, target_id) in spec_list {
            lowerer
                .dynamic_import_specifier_map
                .insert((*module_id, specifier.clone()), *target_id);
        }
    }

    lowerer.shared_env_stack.push(None);
    Ok(lowerer)
}

fn predeclare_cjs_host_bindings(
    lowerer: &mut Lowerer,
    module_id: wjsm_ir::ModuleId,
) -> Result<(), LoweringError> {
    if lowerer.module_metadata.get(&module_id).map(|m| m.kind) != Some(ModuleKind::CommonJs) {
        return Ok(());
    }
    for name in ["require", "module", "exports", "__filename", "__dirname"] {
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
        // 每个模块拥有独立的词法环境与 var 环境。模块体虽然共同降级进
        // `$module_main`，顶层 var/函数声明仍必须按 ECMAScript Module Record 隔离；
        // `ScopeKind::Module` 只改变 var hoist 边界，不改变闭包所属的 IR 函数。
        lowerer.scopes.push_scope(ScopeKind::Module);
        let module_scope = lowerer.scopes.current_scope_id();
        lowerer.module_scopes.insert(module_id, module_scope);
        predeclare_cjs_host_bindings(lowerer, module_id)?;
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
                    // 复用来源模块的 canonical 命名空间对象（入口块已创建并
                    // 注册）：跨模块同源 `import * as` 与动态 import() 必须是
                    // 同一对象身份（§10.4.6.12 GetModuleNamespace 缓存）。
                    let ns_obj = *lowerer
                        .namespace_objects
                        .get(&binding.source_module)
                        .expect("命名空间来源模块已在 setup 阶段登记并于入口块创建对象");
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

/// 为需要命名空间对象的模块（动态 import 目标 ∪ 静态 `import * as` 来源）
/// 各创建一个 canonical 命名空间对象并注册到运行时缓存。运行时缓存注册使
/// 同一模块的后续 `import()`（含 runtime 路径命中同一 RuntimeModuleKey）
/// 返回同一对象，与静态命名空间局部保持对象身份一致。
fn create_namespace_objects(lowerer: &mut Lowerer, entry: BasicBlockId) {
    let mut namespace_modules: Vec<_> = lowerer.namespace_object_modules.iter().copied().collect();
    namespace_modules.sort_by_key(|id| id.0);
    for target_module_id in &namespace_modules {
        // builtin 段（hydration 种子）已在 `$builtin_main` 创建并注册该模块的
        // canonical 对象；入口块头部的段入口前缀调用先执行，这里按 ModuleId
        // 取回同一对象（§10.4.6.12 单一身份），不重建、不重复注册。
        if lowerer.builtin_namespace_modules.contains(target_module_id) {
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
            let ns_obj = lowerer.alloc_value();
            lowerer.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(ns_obj),
                    builtin: Builtin::GetModuleNamespace,
                    args: vec![module_id_val],
                },
            );
            lowerer.namespace_objects.insert(*target_module_id, ns_obj);
            continue;
        }
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
        lowerer.namespace_objects.insert(*target_module_id, ns_obj);
    }
}

fn emit_cjs_host_bindings(
    lowerer: &mut Lowerer,
    module_id: wjsm_ir::ModuleId,
    block: BasicBlockId,
) -> Result<(), LoweringError> {
    let Some(metadata) = lowerer.module_metadata.get(&module_id).cloned() else {
        return Ok(());
    };
    if metadata.kind != ModuleKind::CommonJs {
        return Ok(());
    }
    let Some(&module_scope) = lowerer.module_scopes.get(&module_id) else {
        return Ok(());
    };

    let filename = emit_cjs_string_constant(lowerer, block, metadata.filename);
    let dirname = emit_cjs_string_constant(lowerer, block, metadata.dirname);
    emit_cjs_module_binding(lowerer, block, module_scope, "__filename", filename);
    emit_cjs_module_binding(lowerer, block, module_scope, "__dirname", dirname);

    let exports_obj = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::NewObject {
            dest: exports_obj,
            capacity: 0,
        },
    );
    emit_cjs_module_binding(lowerer, block, module_scope, "exports", exports_obj);

    let module_obj = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::NewObject {
            dest: module_obj,
            capacity: 4,
        },
    );
    emit_cjs_property(lowerer, block, module_obj, "id", filename);
    emit_cjs_property(lowerer, block, module_obj, "filename", filename);
    emit_cjs_property(lowerer, block, module_obj, "exports", exports_obj);
    let loaded_val = emit_cjs_bool_constant(lowerer, block, true);
    emit_cjs_property(lowerer, block, module_obj, "loaded", loaded_val);
    emit_cjs_module_binding(lowerer, block, module_scope, "module", module_obj);

    let require_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: Some(require_val),
            builtin: Builtin::CjsCreateRequire,
            args: vec![filename],
        },
    );
    emit_cjs_module_binding(lowerer, block, module_scope, "require", require_val);
    lowerer.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::CjsRegisterModule,
            args: vec![filename, module_obj, exports_obj],
        },
    );
    Ok(())
}

fn emit_cjs_string_constant(
    lowerer: &mut Lowerer,
    block: BasicBlockId,
    value: String,
) -> wjsm_ir::ValueId {
    let string_const = lowerer.module.add_constant(Constant::String(value));
    let string_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: string_val,
            constant: string_const,
        },
    );
    string_val
}

fn emit_cjs_bool_constant(
    lowerer: &mut Lowerer,
    block: BasicBlockId,
    value: bool,
) -> wjsm_ir::ValueId {
    let bool_const = lowerer.module.add_constant(Constant::Bool(value));
    let bool_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: bool_val,
            constant: bool_const,
        },
    );
    bool_val
}

fn emit_cjs_property(
    lowerer: &mut Lowerer,
    block: BasicBlockId,
    object: wjsm_ir::ValueId,
    key: &str,
    value: wjsm_ir::ValueId,
) {
    let key_val = emit_cjs_string_constant(lowerer, block, key.to_string());
    lowerer.emit_set_prop(block, object, key_val, value);
}

fn emit_cjs_module_binding(
    lowerer: &mut Lowerer,
    block: BasicBlockId,
    module_scope: usize,
    name: &str,
    value: wjsm_ir::ValueId,
) {
    lowerer.current_function.append_instruction(
        block,
        Instruction::StoreVar {
            name: format!("${module_scope}.{name}"),
            value,
        },
    );
}

/// 处理每个模块的 body（语句、导出声明、默认导出等）
fn lower_module_bodies(
    lowerer: &mut Lowerer,
    modules: &[ModuleLoweringInput],
    mut flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    for module in modules {
        let module_id = module.id;
        let module_ast = &module.ast;
        lowerer.current_module_id = Some(module_id);
        // 每模块源码：供 DebugCheck 行/列解析。
        lowerer.diagnostic_source = module.source.clone();
        lowerer.diagnostic_filename = module.metadata.filename.clone();
        // 进入该模块的顶层作用域（#43）：模块体中的标识符解析必须命中模块自己的作用域，
        // 而非根作用域，否则同名顶层变量会跨模块互相解析错位。
        if let Some(&module_scope) = lowerer.module_scopes.get(&module_id) {
            lowerer.scopes.enter_scope(module_scope);
        }
        if let StmtFlow::Open(block) = flow {
            emit_cjs_host_bindings(lowerer, module_id, block)?;
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
    // NamedEvaluation（§16.2.3.7）：`export default <匿名函数定义>` 命名为 "default"。
    if Lowerer::is_anonymous_fn_definition(&default_expr.expr) {
        lowerer.named_eval_hint = Some("default".to_string());
    }
    let value_val = lowerer.lower_expr(&default_expr.expr, outer_block)?;
    let mut outer_block = lowerer.resolve_store_block(outer_block);
    if let Some(current_mid) = lowerer.current_module_id {
        let default_var = format!("_default_export_mod{}", current_mid.0);
        let ir_name = if let Some(ir_name) = lowerer
            .export_map
            .get(&(current_mid, "default".to_string()))
        {
            ir_name.clone()
        } else {
            let (scope_id, _) = lowerer
                .scopes
                .lookup(&default_var)
                .map_err(|msg| lowerer.error(default_expr.span, msg))?;
            format!("${scope_id}.{default_var}")
        };
        // 经 store_binding_value 收口：default 绑定已被命名空间 getter 捕获
        // 进共享 env（序幕安装快照 TDZ 哨兵）时，此处必须同步 env 值。
        let binding = parse_ir_name_to_binding(&ir_name);
        outer_block = lowerer.store_binding_value(
            outer_block,
            &binding,
            value_val,
            default_expr.span,
            true,
        )?;
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
            let outer_block = lowerer.ensure_open(flow)?;
            // NamedEvaluation（§16.2.3.7）：匿名 `export default function` 的
            // `name` 为 "default"；命名形态取自身 ident（保留自引用绑定）。
            if fn_expr.ident.is_none() {
                lowerer.named_eval_hint = Some("default".to_string());
            }
            let fn_val = lowerer.lower_fn_expr(
                &swc_ast::FnExpr {
                    ident: fn_expr.ident.clone(),
                    function: fn_expr.function.clone(),
                },
                outer_block,
            )?;
            let mut outer_block = lowerer.ensure_open(flow)?;
            if let Some(current_mid) = lowerer.current_module_id
                && let Some(ir_name) = lowerer
                    .export_map
                    .get(&(current_mid, "default".to_string()))
                    .cloned()
            {
                // 经 store_binding_value 收口以同步命名空间共享 env。
                let binding = parse_ir_name_to_binding(&ir_name);
                outer_block = lowerer.store_binding_value(
                    outer_block,
                    &binding,
                    fn_val,
                    default_decl.span(),
                    true,
                )?;
            }
            Ok(StmtFlow::Open(outer_block))
        }
        swc_ast::DefaultDecl::Class(class_expr) => {
            let outer_block = lowerer.ensure_open(flow)?;
            // NamedEvaluation（§16.2.3.7）：匿名 `export default class` 的
            // 构造器 `name` 为 "default"。
            if class_expr.ident.is_none() {
                lowerer.named_eval_hint = Some("default".to_string());
            }
            let class_val = lowerer.lower_class_expr(
                &swc_ast::ClassExpr {
                    ident: class_expr.ident.clone(),
                    class: class_expr.class.clone(),
                },
                outer_block,
            )?;
            // 类求值可能推进 block（计算键异常分叉等）：消费延续块，
            // 后续导出存储不得落回已终止的入口块。
            let mut outer_block = lowerer.resolve_store_block(outer_block);
            if let Some(current_mid) = lowerer.current_module_id
                && let Some(ir_name) = lowerer
                    .export_map
                    .get(&(current_mid, "default".to_string()))
                    .cloned()
            {
                // 经 store_binding_value 收口以同步命名空间共享 env。
                let binding = parse_ir_name_to_binding(&ir_name);
                outer_block = lowerer.store_binding_value(
                    outer_block,
                    &binding,
                    class_val,
                    default_decl.span(),
                    true,
                )?;
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
    /// 在模块体之前安装：`ensure_shared_env` 对尚未初始化的绑定快照 TDZ 哨兵
    /// （getter 读到哨兵按 §10.4.6.8 抛 ReferenceError），声明执行时经
    /// `store_binding_value` 同步共享 env，安装后不会重复快照。
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
        // load_captured_binding 走 record_capture + GetProp 路径），
        // 经 TDZ 检查后返回。§10.4.6.8 [[Get]] 对未初始化的导出绑定
        // （循环导入下声明尚未执行）须抛 ReferenceError 而非泄漏哨兵值。
        let entry = BasicBlockId(0);
        let value_val = self.load_captured_binding(entry, binding)?;
        let ret_block = self.resolve_store_block(entry);
        let (checked_val, ret_block) = self.emit_tdz_check(ret_block, value_val, &binding.name)?;
        self.current_function.set_terminator(
            ret_block,
            Terminator::Return {
                value: Some(checked_val),
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
        self.finalize_function_captures(&mut ir_function, &captured);
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

/// 在全部模块体之前（入口序幕）为每个 canonical 命名空间对象安装 live
/// binding getter 并收口 exotic 身份。
///
/// ECMAScript 在 Link 阶段（任何模块求值之前）即物化命名空间对象的全部
/// 属性；循环导入下先执行的模块读取后执行模块的命名空间时，键集合、
/// 描述符与 exotic 行为必须已就绪，未初始化的 let/const 导出经 getter 的
/// TDZ 检查抛 ReferenceError（§10.4.6.8）。绑定快照由 `ensure_shared_env`
/// 处理：TDZ 绑定写入未初始化哨兵，声明执行时 `store_binding_value` 同步
/// 共享 env，live binding 语义不变。按 module_id 升序安装保证确定性输出。
fn install_all_namespace_getters(
    lowerer: &mut Lowerer,
    mut flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    let mut source_module_ids: Vec<ModuleId> = lowerer.namespace_objects.keys().copied().collect();
    source_module_ids.sort_by_key(|id| id.0);
    for module_id in source_module_ids {
        // builtin 段取回的 canonical 对象已由 `$builtin_main` 安装 getter 并
        // 收口为 exotic（不可扩展），跳过重复安装。
        if lowerer.builtin_namespace_modules.contains(&module_id) {
            continue;
        }
        // 进入来源模块作用域：resolve_export_ir 的作用域回退解析须命中该
        // 模块自己的顶层绑定（builtin 段模块的占位作用域同样已登记）。
        let module_scope = lowerer.module_scopes.get(&module_id).copied();
        if let Some(scope) = module_scope {
            lowerer.scopes.enter_scope(scope);
        }
        let result = install_live_namespace_getters_for_source(lowerer, module_id, flow);
        if module_scope.is_some() {
            lowerer.scopes.pop_scope();
        }
        flow = result?;
    }
    Ok(flow)
}

/// 为来源模块 `source_module_id` 的 canonical 命名空间对象安装 live binding
/// getter（#45），并收口为 Module Namespace Exotic Object（§10.4.6）。
///
/// 在入口序幕（任何模块体之前）调用，对应规范 Link 阶段的命名空间物化。
/// 静态 `import * as` 与动态 `import()` 共享同一对象，安装只发生一次。
///
/// 每个导出名安装一个 getter 访问器，getter 读取来源模块的导出绑定（经由捕获/共享 env
/// 机制返回最新值），从而满足 ECMAScript §10.4.6 命名空间对象的 live binding 语义。
/// 全部导出与 @@toStringTag 安装完成后发射 FinalizeModuleNamespace：
/// [[Prototype]] 置 null、不可扩展、宿主登记 exotic 身份。
fn install_live_namespace_getters_for_source(
    lowerer: &mut Lowerer,
    source_module_id: wjsm_ir::ModuleId,
    flow: StmtFlow,
) -> Result<StmtFlow, LoweringError> {
    let StmtFlow::Open(mut block) = flow else {
        return Ok(flow);
    };

    let Some(&ns_obj) = lowerer.namespace_objects.get(&source_module_id) else {
        return Ok(StmtFlow::Open(block));
    };

    // 解析本模块全部导出名 → 来源 IR 变量名（按名排序，保证确定性输出）。
    let mut exports: Vec<(String, String)> = Vec::new();
    if let Some(names) = lowerer.module_export_names.get(&source_module_id).cloned() {
        for export_name in &names {
            if let Some(ir_name) = resolve_export_ir(lowerer, source_module_id, export_name) {
                exports.push((export_name.clone(), ir_name));
            }
        }
    }

    for (export_name, source_ir_name) in &exports {
        block = lowerer.install_namespace_getter(ns_obj, export_name, source_ir_name, block)?;
    }
    set_namespace_string_tag(lowerer, ns_obj, block);
    lowerer.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::FinalizeModuleNamespace,
            args: vec![ns_obj],
        },
    );
    Ok(StmtFlow::Open(block))
}

/// 为命名空间对象定义 `@@toStringTag = "Module"`（ECMAScript §10.4.6.2）。
///
/// 必须用 well-known symbol 真键与全 false 特性的数据描述符：字符串键
/// `"Symbol.toStringTag"` 会被 `Object.keys` 枚举出来，且
/// `Object.prototype.toString` 只读真符号键（否则呈现 `[object Object]`）。
fn set_namespace_string_tag(lowerer: &mut Lowerer, ns_obj: wjsm_ir::ValueId, block: BasicBlockId) {
    let index_const = lowerer.module.add_constant(Constant::Number(f64::from(
        wjsm_ir::wk_symbol::TO_STRING_TAG,
    )));
    let index_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: index_val,
            constant: index_const,
        },
    );
    let tag_symbol = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: Some(tag_symbol),
            builtin: Builtin::SymbolWellKnown,
            args: vec![index_val],
        },
    );
    // 数据描述符 { value: "Module" }：writable/enumerable/configurable 缺省 false，
    // 与 §10.4.6.2 的属性特性一致。
    let descriptor = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::NewObject {
            dest: descriptor,
            capacity: 1,
        },
    );
    let value_key = lowerer
        .module
        .add_constant(Constant::String("value".to_string()));
    let value_key_val = lowerer.alloc_value();
    lowerer.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: value_key_val,
            constant: value_key,
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
    lowerer.emit_set_prop(block, descriptor, value_key_val, tag_value_val);
    lowerer.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::DefineProperty,
            args: vec![ns_obj, tag_symbol, descriptor],
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
