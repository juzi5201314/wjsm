use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp,
    ValueId,
};

const EVAL_SCOPE_ENV_PARAM: &str = "$eval_env";

const WK_SYMBOL_ITERATOR: u32 = 0;
const WK_SYMBOL_SPECIES: u32 = 1;
const WK_SYMBOL_TO_STRING_TAG: u32 = 2;
const WK_SYMBOL_ASYNC_ITERATOR: u32 = 3;
const WK_SYMBOL_HAS_INSTANCE: u32 = 4;
const WK_SYMBOL_TO_PRIMITIVE: u32 = 5;
const WK_SYMBOL_DISPOSE: u32 = 6;
const WK_SYMBOL_MATCH: u32 = 7;
const WK_SYMBOL_ASYNC_DISPOSE: u32 = 8;

// ── Scope tree ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Block,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Var,
    Let,
    Const,
}

/// 控制预扫描时是否包含 let/const 声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexicalMode {
    /// 包含 let/const 声明（顶层作用域预扫描）。
    Include,
    /// 排除 let/const 声明（块级作用域嵌套扫描）。
    Exclude,
}

#[derive(Debug, Clone)]
struct VarInfo {
    kind: VarKind,
    /// `false` = in TDZ (declared lexically but not yet initialised).
    /// `true`  = initialised and ready for use.
    initialised: bool,
}

struct Scope {
    parent: Option<usize>,
    kind: ScopeKind,
    id: usize,
    variables: std::collections::HashMap<String, VarInfo>,
}

struct ScopeTree {
    arenas: Vec<Scope>,
    current: usize,
}

impl ScopeTree {
    fn new() -> Self {
        let root = Scope {
            parent: None,
            kind: ScopeKind::Function,
            id: 0,
            variables: std::collections::HashMap::new(),
        };
        let mut arenas = Vec::new();
        arenas.push(root);
        Self { arenas, current: 0 }
    }

    /// Push a new child scope and enter it.
    fn push_scope(&mut self, kind: ScopeKind) {
        let idx = self.arenas.len();
        let scope = Scope {
            parent: Some(self.current),
            kind,
            id: idx,
            variables: std::collections::HashMap::new(),
        };
        self.arenas.push(scope);
        self.current = idx;
    }

    /// 获取当前 scope 的 id。
    fn current_scope_id(&self) -> usize {
        self.current
    }

    /// 返回指定 scope 所属的最近函数 scope。
    fn function_scope_for_scope(&self, mut scope_id: usize) -> usize {
        loop {
            let scope = &self.arenas[scope_id];
            if matches!(scope.kind, ScopeKind::Function) {
                return scope_id;
            }
            scope_id = scope
                .parent
                .expect("non-root scope must have a parent function scope");
        }
    }

    /// Pop the current scope, returning to its parent.
    fn pop_scope(&mut self) {
        self.current = self.arenas[self.current]
            .parent
            .expect("cannot pop root scope");
    }

    /// Declare a variable in the appropriate scope.
    ///
    /// - `let` / `const` → current (innermost) scope.
    /// - `var`          → nearest enclosing *function* scope.
    ///
    /// Returns `Err(message)` on redeclaration conflict (let/const in same scope).
    fn declare(&mut self, name: &str, kind: VarKind, initialised: bool) -> Result<usize, String> {
        let target_idx = match kind {
            VarKind::Var => self.nearest_function_scope()?,
            VarKind::Let | VarKind::Const => self.current,
        };

        let scope = &mut self.arenas[target_idx];

        // var redeclaration in the same scope is allowed (JS semantics).
        // let / const redeclaration in the same scope is an error.
        if let Some(existing) = scope.variables.get(name) {
            match (existing.kind, kind) {
                (VarKind::Var, VarKind::Var) => return Ok(scope.id),
                _ => {
                    return Err(format!(
                        "cannot redeclare identifier `{name}` in the same scope"
                    ));
                }
            }
        }

        scope
            .variables
            .insert(name.to_string(), VarInfo { kind, initialised });
        Ok(scope.id)
    }

    /// Mark a variable as initialised (exit TDZ).
    fn mark_initialised(&mut self, name: &str) -> Result<(), String> {
        let mut cursor = self.current;
        loop {
            let scope = &mut self.arenas[cursor];
            if let Some(info) = scope.variables.get_mut(name) {
                info.initialised = true;
                return Ok(());
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// Look up a variable by name. Returns `(scope_id, VarKind)` if found.
    fn lookup(&self, name: &str) -> Result<(usize, VarKind), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if !info.initialised {
                    return Err(format!("cannot access `{name}` before initialisation"));
                }
                return Ok((scope.id, info.kind));
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// 返回当前词法环境可见的绑定；同名绑定只保留最近的一层。
    fn visible_bindings(&self) -> Vec<(usize, String, VarKind)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &self.arenas[scope_id];
            let mut names: Vec<_> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if seen.insert(name.clone()) {
                    if let Some(info) = scope.variables.get(&name) {
                        if info.initialised {
                            result.push((scope.id, name, info.kind));
                        }
                    }
                }
            }
            cursor = scope.parent;
        }
        result
    }

    /// Resolve a variable's scope id without checking TDZ.
    fn resolve_scope_id(&self, name: &str) -> Result<usize, String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if scope.variables.contains_key(name) {
                return Ok(scope.id);
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// 赋值时查找变量：组合 `check_mutable` 和 `lookup`，
    /// 一次 scope chain 遍历完成 const 检查 + TDZ 检查。
    ///
    /// # 性能优化
    /// `lower_assign` 原本先调 `check_mutable` 再调 `lookup`，
    /// 分别遍历 scope chain 各一次。合并为一次遍历减少冗余的 HashMap 查找，
    /// 在深层嵌套作用域中有最多约 50% 的查找节省。
    fn lookup_for_assign(&self, name: &str) -> Result<(usize, VarKind), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if matches!(info.kind, VarKind::Const) {
                    return Err(format!(
                        "cannot reassign a const-declared variable `{name}`"
                    ));
                }
                if !info.initialised {
                    return Err(format!("cannot access `{name}` before initialisation"));
                }
                return Ok((scope.id, info.kind));
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// Check that this variable is not `const` before reassignment.
    /// 注意：`lower_assign` 现在使用 `lookup_for_assign` 在一次遍历中同时完成
    /// const 检查和 scope 解析，此方法保留以供未来使用。
    #[allow(dead_code)]
    fn check_mutable(&self, name: &str) -> Result<(), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if matches!(info.kind, VarKind::Const) {
                    return Err(format!(
                        "cannot reassign a const-declared variable `{name}`"
                    ));
                }
                return Ok(());
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    fn nearest_function_scope(&self) -> Result<usize, String> {
        let mut cursor = self.current;
        loop {
            if matches!(self.arenas[cursor].kind, ScopeKind::Function) {
                return Ok(cursor);
            }
            cursor = self.arenas[cursor]
                .parent
                .ok_or_else(|| "root must be function scope".to_string())?;
        }
    }
}

// ── CFG Builder ─────────────────────────────────────────────────────────

/// Internal helper that encapsulates CFG construction for one function.
struct FunctionBuilder {
    _name: String,
    _entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
    has_eval: bool,
}

impl FunctionBuilder {
    fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            _name: name.into(),
            _entry: entry,
            blocks: vec![BasicBlock::new(entry)],
            has_eval: false,
        }
    }

    fn mark_has_eval(&mut self) {
        self.has_eval = true;
    }

    fn has_eval(&self) -> bool {
        self.has_eval
    }

    fn new_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::new(id));
        id
    }

    fn append_instruction(&mut self, block: BasicBlockId, instruction: Instruction) {
        if let Some(b) = self.block_mut(block) {
            b.push_instruction(instruction);
        }
    }

    fn set_terminator(&mut self, block: BasicBlockId, terminator: Terminator) {
        if let Some(b) = self.block_mut(block) {
            b.set_terminator(terminator);
        }
    }

    /// O(1) 通过 id 获取 block 可变引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 new_block 保证），
    /// 使用直接索引访问而非 iter_mut().find()，将 O(n) 降为 O(1)。
    fn block_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// O(1) 通过 id 获取 block 引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    fn block(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    /// Ensure control flow from `from` reaches `target`.
    ///
    /// - If `from` is `Terminated`: no-op, returns `Terminated`.
    /// - If `from` is `Open(block)` and block has Unreachable terminator: set Jump { target }.
    /// - Returns `StmtFlow::Open(target)` so caller can continue writing to target.
    fn ensure_jump_or_terminated(&mut self, from: StmtFlow, target: BasicBlockId) -> StmtFlow {
        match from {
            StmtFlow::Terminated => StmtFlow::Terminated,
            StmtFlow::Open(block) => {
                let is_unreachable = self
                    .block(block)
                    .map_or(false, |b| matches!(b.terminator(), Terminator::Unreachable));
                if is_unreachable {
                    self.set_terminator(block, Terminator::Jump { target });
                }
                StmtFlow::Open(target)
            }
        }
    }

    #[allow(dead_code)]
    fn finish(self) -> Function {
        let entry = self._entry;
        let mut function = Function::new(self._name, entry);
        function.set_has_eval(self.has_eval);
        function
    }

    fn into_blocks(mut self) -> Vec<BasicBlock> {
        std::mem::take(&mut self.blocks)
    }
}

// ── Label & Finally tracking ────────────────────────────────────────────

#[derive(Debug, Clone)]
struct LabelContext {
    label: Option<String>,
    kind: LabelKind,
    break_target: BasicBlockId,
    continue_target: Option<BasicBlockId>,
    iterator_to_close: Option<ValueId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelKind {
    Loop,
    Switch,
    Block,
}

#[derive(Debug, Clone)]
struct FinallyContext {
    _finally_block: BasicBlockId,
    _after_finally_block: BasicBlockId,
}

#[derive(Debug, Clone)]
struct TryContext {
    catch_entry: Option<BasicBlockId>,
    exception_var: String,
    label_depth: usize,
}

/// The flow state after lowering a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StmtFlow {
    /// Control flow continues in the given basic block.
    Open(BasicBlockId),
    /// The statement terminated control flow (return, throw, break, continue, unreachable).
    Terminated,
}

// ── Public API ──────────────────────────────────────────────────────────

/// 检测模块体是否包含 top-level `await`（不递归进入函数/类体边界）
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

// ── Lowerer ─────────────────────────────────────────────────────────────

struct Lowerer {
    module: Module,
    next_value: u32,
    scopes: ScopeTree,
    hoisted_vars: Vec<HoistedVar>,
    /// 用于 O(1) 重复检测的 HashSet。
    hoisted_vars_set: std::collections::HashSet<(usize, String)>,
    current_function: FunctionBuilder,
    label_stack: Vec<LabelContext>,
    finally_stack: Vec<FinallyContext>,
    try_contexts: Vec<TryContext>,
    next_temp: u32,
    pending_loop_label: Option<String>,
    active_finalizers: Vec<swc_ast::BlockStmt>,
    /// 匿名类 / 匿名函数计数器
    anon_counter: u32,
    // ── Function context stack ────────────────────────────────────────────
    function_stack: Vec<FunctionBuilder>,
    function_hoisted_stack: Vec<(Vec<HoistedVar>, std::collections::HashSet<(usize, String)>)>,
    function_next_value_stack: Vec<u32>,
    function_next_temp_stack: Vec<u32>,
    async_context_stack: Vec<AsyncContextState>,
    function_try_contexts_stack: Vec<Vec<TryContext>>,
    function_finally_stack_stack: Vec<Vec<FinallyContext>>,
    function_label_stack_stack: Vec<Vec<LabelContext>>,
    function_active_finalizers_stack: Vec<Vec<swc_ast::BlockStmt>>,
    function_pending_loop_label_stack: Vec<Option<String>>,
    // ── 闭包捕获相关 ──────────────────────────────────────────────────
    /// 每层函数的捕获绑定列表，push_function_context 时压入空 Vec。
    captured_names_stack: Vec<Vec<CapturedBinding>>,
    /// 每层函数的 function scope id，用于判断变量是否逃逸。
    function_scope_id_stack: Vec<usize>,
    /// 追踪当前是否在箭头函数中（箭头函数的 this 需要词法捕获）
    is_arrow_fn_stack: Vec<bool>,
    /// 每层函数的共享 env 对象 (ValueId) + 已注册的捕获绑定集合。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，确保可变捕获变量的修改对所有闭包可见。
    shared_env_stack: Vec<Option<(ValueId, std::collections::HashSet<CapturedBinding>)>>,
    // ── 模块系统相关 ────────────────────────────────────────────────────────
    /// 当前正在编译的模块 ID（用于多模块编译）
    current_module_id: Option<wjsm_ir::ModuleId>,
    /// 导入映射：module_id → ImportBinding 列表
    import_bindings: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    /// 导出映射：.0 = 模块 ID, .1 = 导出名 → 变量名
    export_map: std::collections::HashMap<(wjsm_ir::ModuleId, String), String>,
    /// 导入别名映射：local_name → source_ir_name
    /// 用于 `import { x as y }` 和 `import x from './dep'` 等场景
    import_aliases: std::collections::HashMap<String, String>,
    /// 动态 import() 目标映射：module_id → 被动态 import 的目标模块 ID 列表
    dynamic_import_targets: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    /// 动态 import specifier → ModuleId 映射：(当前模块 ID, specifier) → 目标 ModuleId
    dynamic_import_specifier_map:
        std::collections::HashMap<(wjsm_ir::ModuleId, String), wjsm_ir::ModuleId>,
    /// 需要构建命名空间对象的模块集合
    dynamic_import_namespace_modules: std::collections::HashSet<wjsm_ir::ModuleId>,
    /// 命名空间对象的 ValueId：ModuleId → ValueId（在模块体执行前创建，模块体执行后填充属性）
    dynamic_import_namespace_objects:
        std::collections::HashMap<wjsm_ir::ModuleId, wjsm_ir::ValueId>,
    module_export_names:
        std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    is_async_fn: bool,
    is_async_generator_fn: bool,
    async_state_counter: u32,
    captured_var_slots: std::collections::HashMap<String, u32>,
    async_next_continuation_slot: u32,
    async_resume_blocks: Vec<(u32, BasicBlockId)>,
    async_promise_scope_id: usize,
    async_dispatch_block: Option<BasicBlockId>,
    async_main_body_entry: Option<BasicBlockId>,
    async_main_param_ir_names: Vec<String>,
    async_env_scope_id: usize,
    async_state_scope_id: usize,
    async_resume_val_scope_id: usize,
    async_is_rejected_scope_id: usize,
    async_generator_scope_id: usize,
    async_closure_env_ir_name: Option<String>,
    strict_mode: bool,
    eval_mode: bool,
    eval_has_scope_bridge: bool,
    eval_var_writes_to_scope: bool,
    eval_completion: Option<ValueId>,
    /// 当前作用域中活跃的 using 变量（用于作用域退出时自动 dispose）
    active_using_vars: Vec<ActiveUsingVar>,
}

/// 追踪当前作用域中的 using 变量，用于在作用域退出时自动 dispose。
#[derive(Debug, Clone)]
struct ActiveUsingVar {
    ir_name: String,
    is_async: bool,
}

#[derive(Clone)]
struct AsyncContextState {
    is_async_fn: bool,
    is_async_generator_fn: bool,
    async_state_counter: u32,
    captured_var_slots: std::collections::HashMap<String, u32>,
    async_next_continuation_slot: u32,
    async_resume_blocks: Vec<(u32, BasicBlockId)>,
    async_promise_scope_id: usize,
    async_dispatch_block: Option<BasicBlockId>,
    async_env_scope_id: usize,
    async_state_scope_id: usize,
    async_resume_val_scope_id: usize,
    async_is_rejected_scope_id: usize,
    async_generator_scope_id: usize,
    async_closure_env_ir_name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct HoistedVar {
    scope_id: usize,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CapturedBinding {
    name: String,
    scope_id: Option<usize>,
}

impl CapturedBinding {
    fn new(name: impl Into<String>, scope_id: usize) -> Self {
        Self {
            name: name.into(),
            scope_id: Some(scope_id),
        }
    }

    fn lexical_this() -> Self {
        Self {
            name: "$this".to_string(),
            scope_id: None,
        }
    }

    fn env_key(&self) -> String {
        match self.scope_id {
            Some(scope_id) => format!("${scope_id}.{}", self.name),
            None => self.name.clone(),
        }
    }

    fn display_name(&self) -> String {
        self.env_key()
    }

    fn var_ir_name(&self) -> String {
        match self.scope_id {
            Some(scope_id) => format!("${scope_id}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

impl Lowerer {
    fn new() -> Self {
        let mut scopes = ScopeTree::new();
        // 预注册 ECMAScript 全局内置标识符
        let _ = scopes.declare("undefined", VarKind::Var, true);
        let _ = scopes.declare("NaN", VarKind::Var, true);
        let _ = scopes.declare("Infinity", VarKind::Var, true);
        let _ = scopes.declare("Symbol", VarKind::Var, true);

        Self {
            module: Module::new(),
            next_value: 0,
            scopes,
            hoisted_vars: Vec::new(),
            hoisted_vars_set: std::collections::HashSet::new(),
            current_function: FunctionBuilder::new("main", BasicBlockId(0)),
            label_stack: Vec::new(),
            finally_stack: Vec::new(),
            try_contexts: Vec::new(),
            next_temp: 0,
            pending_loop_label: None,
            active_finalizers: Vec::new(),
            anon_counter: 0,
            function_stack: Vec::new(),
            function_hoisted_stack: Vec::new(),
            function_next_value_stack: Vec::new(),
            function_next_temp_stack: Vec::new(),
            async_context_stack: Vec::new(),
            function_try_contexts_stack: Vec::new(),
            function_finally_stack_stack: Vec::new(),
            function_label_stack_stack: Vec::new(),
            function_active_finalizers_stack: Vec::new(),
            function_pending_loop_label_stack: Vec::new(),
            captured_names_stack: Vec::new(),
            function_scope_id_stack: Vec::new(),
            is_arrow_fn_stack: Vec::new(),
            shared_env_stack: Vec::new(),
            current_module_id: None,
            import_bindings: std::collections::HashMap::new(),
            export_map: std::collections::HashMap::new(),
            import_aliases: std::collections::HashMap::new(),
            dynamic_import_targets: std::collections::HashMap::new(),
            dynamic_import_namespace_modules: std::collections::HashSet::new(),
            dynamic_import_namespace_objects: std::collections::HashMap::new(),
            dynamic_import_specifier_map: std::collections::HashMap::new(),
            module_export_names: std::collections::HashMap::new(),
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_main_body_entry: None,
            async_main_param_ir_names: Vec::new(),
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
            strict_mode: false,
            eval_mode: false,
            eval_has_scope_bridge: false,
            eval_var_writes_to_scope: false,
            active_using_vars: Vec::new(),
            eval_completion: None,
        }
    }

    fn capture_async_context(&self) -> AsyncContextState {
        AsyncContextState {
            is_async_fn: self.is_async_fn,
            is_async_generator_fn: self.is_async_generator_fn,
            async_state_counter: self.async_state_counter,
            captured_var_slots: self.captured_var_slots.clone(),
            async_next_continuation_slot: self.async_next_continuation_slot,
            async_resume_blocks: self.async_resume_blocks.clone(),
            async_promise_scope_id: self.async_promise_scope_id,
            async_dispatch_block: self.async_dispatch_block,
            async_env_scope_id: self.async_env_scope_id,
            async_state_scope_id: self.async_state_scope_id,
            async_resume_val_scope_id: self.async_resume_val_scope_id,
            async_is_rejected_scope_id: self.async_is_rejected_scope_id,
            async_generator_scope_id: self.async_generator_scope_id,
            async_closure_env_ir_name: self.async_closure_env_ir_name.clone(),
        }
    }

    fn restore_async_context(&mut self, context: AsyncContextState) {
        self.is_async_fn = context.is_async_fn;
        self.is_async_generator_fn = context.is_async_generator_fn;
        self.async_state_counter = context.async_state_counter;
        self.captured_var_slots = context.captured_var_slots;
        self.async_next_continuation_slot = context.async_next_continuation_slot;
        self.async_resume_blocks = context.async_resume_blocks;
        self.async_promise_scope_id = context.async_promise_scope_id;
        self.async_dispatch_block = context.async_dispatch_block;
        self.async_env_scope_id = context.async_env_scope_id;
        self.async_state_scope_id = context.async_state_scope_id;
        self.async_resume_val_scope_id = context.async_resume_val_scope_id;
        self.async_is_rejected_scope_id = context.async_is_rejected_scope_id;
        self.async_generator_scope_id = context.async_generator_scope_id;
        self.async_closure_env_ir_name = context.async_closure_env_ir_name;
    }

    fn reset_async_context(&mut self) {
        self.restore_async_context(AsyncContextState {
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
        });
    }

    fn push_function_context(&mut self, name: impl Into<String>, entry: BasicBlockId) {
        self.async_context_stack.push(self.capture_async_context());
        self.function_stack.push(std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new(name, entry),
        ));
        // 压入函数作用域到现有作用域树，而非创建新树
        self.scopes.push_scope(ScopeKind::Function);
        // 记录当前函数的 scope id（用于逃逸分析）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false); // 默认非箭头函数，箭头函数会单独设置
        self.shared_env_stack.push(None); // 新函数上下文，尚无共享 env 对象
        self.function_hoisted_stack.push((
            std::mem::take(&mut self.hoisted_vars),
            std::mem::take(&mut self.hoisted_vars_set),
        ));
        self.function_next_value_stack.push(self.next_value);
        self.function_next_temp_stack.push(self.next_temp);
        self.next_value = 0;
        self.next_temp = 0;
        self.function_try_contexts_stack.push(std::mem::take(&mut self.try_contexts));
        self.function_finally_stack_stack.push(std::mem::take(&mut self.finally_stack));
        self.function_label_stack_stack.push(std::mem::take(&mut self.label_stack));
        self.function_active_finalizers_stack.push(std::mem::take(&mut self.active_finalizers));
        self.function_pending_loop_label_stack.push(self.pending_loop_label.take());
        self.reset_async_context();
    }

    fn pop_function_context(&mut self) {
        self.current_function = self.function_stack.pop().expect("function stack underflow");
        // 弹出函数作用域，回到外层作用域
        self.scopes.pop_scope();
        self.function_scope_id_stack.pop();
        self.captured_names_stack.pop();
        self.is_arrow_fn_stack.pop();
        self.shared_env_stack.pop();
        let (vars, set) = self
            .function_hoisted_stack
            .pop()
            .expect("hoisted stack underflow");
        self.hoisted_vars = vars;
        self.hoisted_vars_set = set;
        self.next_value = self
            .function_next_value_stack
            .pop()
            .expect("next value stack underflow");
        self.next_temp = self
            .function_next_temp_stack
            .pop()
            .expect("next temp stack underflow");
        self.try_contexts = self
            .function_try_contexts_stack
            .pop()
            .expect("try contexts stack underflow");
        self.finally_stack = self
            .function_finally_stack_stack
            .pop()
            .expect("finally stack stack underflow");
        self.label_stack = self
            .function_label_stack_stack
            .pop()
            .expect("label stack stack underflow");
        self.active_finalizers = self
            .function_active_finalizers_stack
            .pop()
            .expect("active finalizers stack underflow");
        self.pending_loop_label = self
            .function_pending_loop_label_stack
            .pop()
            .expect("pending loop label stack underflow");
        let async_context = self
            .async_context_stack
            .pop()
            .expect("async context stack underflow");
        self.restore_async_context(async_context);
    }

    fn current_function_scope_id(&self) -> usize {
        self.function_scope_id_stack.last().copied().unwrap_or(0)
    }

    fn binding_owner_function_scope(&self, binding: &CapturedBinding) -> usize {
        binding
            .scope_id
            .map(|scope_id| self.scopes.function_scope_for_scope(scope_id))
            .unwrap_or_else(|| self.current_function_scope_id())
    }

    fn binding_belongs_to_current_function(&self, binding: &CapturedBinding) -> bool {
        self.binding_owner_function_scope(binding) == self.current_function_scope_id()
    }

    fn record_capture(&mut self, binding: CapturedBinding) {
        if let Some(captured) = self.captured_names_stack.last_mut() {
            if !captured.contains(&binding) {
                captured.push(binding);
            }
        }
    }

    fn captured_display_names(captured: &[CapturedBinding]) -> Vec<String> {
        captured.iter().map(CapturedBinding::display_name).collect()
    }

    fn is_shared_binding(&self, binding: &CapturedBinding) -> bool {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref())
            .map_or(false, |(_, names)| names.contains(binding))
    }

    fn shared_env_value(&self) -> Option<ValueId> {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref().map(|(value, _)| *value))
    }

    fn append_env_key_const(&mut self, block: BasicBlockId, binding: &CapturedBinding) -> ValueId {
        let key_const = self
            .module
            .add_constant(Constant::String(binding.env_key()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        key_val
    }

    fn load_captured_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let env_val = if self.binding_belongs_to_current_function(binding) {
            self.shared_env_value()
                .expect("shared binding must have a materialized env")
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(block)
        };
        let key_val = self.append_env_key_const(block, binding);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object: env_val,
                key: key_val,
            },
        );
        Ok(dest)
    }

    fn lower_module(mut self, module: &swc_ast::Module) -> Result<Program, LoweringError> {
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

        // Math constants
        let math_constants: [(&str, f64); 8] = [
            ("$0.Math.E", std::f64::consts::E),
            ("$0.Math.LN10", std::f64::consts::LN_10),
            ("$0.Math.LN2", std::f64::consts::LN_2),
            ("$0.Math.LOG10E", std::f64::consts::LOG10_E),
            ("$0.Math.LOG2E", std::f64::consts::LOG2_E),
            ("$0.Math.PI", std::f64::consts::PI),
            ("$0.Math.SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("$0.Math.SQRT2", std::f64::consts::SQRT_2),
        ];
        for (name, val) in math_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

        // Number constants
        let number_constants: [(&str, f64); 8] = [
            ("$0.Number.EPSILON", f64::EPSILON),
            ("$0.Number.MAX_VALUE", f64::MAX),
            ("$0.Number.MIN_VALUE", f64::MIN_POSITIVE),
            ("$0.Number.MAX_SAFE_INTEGER", (1i64 << 53) as f64 - 1.0),
            ("$0.Number.MIN_SAFE_INTEGER", -((1i64 << 53) as f64 - 1.0)),
            ("$0.Number.NaN", f64::NAN),
            ("$0.Number.NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("$0.Number.POSITIVE_INFINITY", f64::INFINITY),
        ];
        for (name, val) in number_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

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

    fn lower_stmt(
        &mut self,
        stmt: &swc_ast::Stmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => self.lower_expr_stmt(expr_stmt, flow),
            swc_ast::Stmt::Decl(decl) => match decl {
                swc_ast::Decl::Fn(fn_decl) => self.lower_fn_decl(fn_decl, flow),
                swc_ast::Decl::Var(var_decl) => self.lower_var_decl(var_decl, flow),
                swc_ast::Decl::Class(class_decl) => self.lower_class_decl(class_decl, flow),
                swc_ast::Decl::TsInterface(_) => self.lower_empty(flow),
                swc_ast::Decl::TsTypeAlias(_) => self.lower_empty(flow),
                swc_ast::Decl::TsEnum(ts_enum) => self.lower_ts_enum(ts_enum, flow),
                swc_ast::Decl::TsModule(ts_module) => self.lower_ts_module(ts_module, flow),
                swc_ast::Decl::Using(using_decl) => self.lower_using_decl(using_decl, flow),
                #[allow(unreachable_patterns)]
                _ => Err(self.error(
                    stmt.span(),
                    format!("unsupported declaration kind `{}`", decl_kind(decl)),
                )),
            },
            swc_ast::Stmt::Block(block_stmt) => self.lower_block_stmt(block_stmt, flow),
            swc_ast::Stmt::If(if_stmt) => self.lower_if(if_stmt, flow),
            swc_ast::Stmt::While(while_stmt) => self.lower_while(while_stmt, flow),
            swc_ast::Stmt::DoWhile(do_while_stmt) => self.lower_do_while(do_while_stmt, flow),
            swc_ast::Stmt::For(for_stmt) => self.lower_for(for_stmt, flow),
            swc_ast::Stmt::ForIn(for_in) => self.lower_for_in(for_in, flow),
            swc_ast::Stmt::ForOf(for_of) => self.lower_for_of(for_of, flow),
            swc_ast::Stmt::Break(break_stmt) => self.lower_break(break_stmt, flow),
            swc_ast::Stmt::Continue(continue_stmt) => self.lower_continue(continue_stmt, flow),
            swc_ast::Stmt::Return(return_stmt) => self.lower_return(return_stmt, flow),
            swc_ast::Stmt::Labeled(labeled) => self.lower_labeled(labeled, flow),
            swc_ast::Stmt::Switch(switch_stmt) => self.lower_switch(switch_stmt, flow),
            swc_ast::Stmt::Throw(throw_stmt) => self.lower_throw(throw_stmt, flow),
            swc_ast::Stmt::Try(try_stmt) => self.lower_try(try_stmt, flow),
            swc_ast::Stmt::Empty(_) => self.lower_empty(flow),
            swc_ast::Stmt::Debugger(_) => self.lower_debugger(flow),
            swc_ast::Stmt::With(with_stmt) => self.lower_with(with_stmt, flow),
        }
    }

    // ── Expression statements ───────────────────────────────────────────────

    fn lower_expr_stmt(
        &mut self,
        expr_stmt: &swc_ast::ExprStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        if self.eval_mode {
            let value = self.lower_expr(&expr_stmt.expr, block)?;
            self.eval_completion = Some(value);
            return Ok(StmtFlow::Open(self.resolve_store_block(block)));
        }

        let result_block = match expr_stmt.expr.as_ref() {
            swc_ast::Expr::Call(call) => self.lower_call(call, block)?,
            expr => {
                let _value = self.lower_expr(expr, block)?;
                self.resolve_store_block(block)
            }
        };
        Ok(StmtFlow::Open(result_block))
    }

    fn lower_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let _ = self.lower_call_expr(call, block)?;
        Ok(self.resolve_store_block(block))
    }

    // ── Blocks ──────────────────────────────────────────────────────────────

    fn lower_block_stmt(
        &mut self,
        block_stmt: &swc_ast::BlockStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let prev_using_count = self.active_using_vars.len();
        self.scopes.push_scope(ScopeKind::Block);
        self.predeclare_block_stmts(&block_stmt.stmts)?;

        let mut flow = flow;
        for stmt in &block_stmt.stmts {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            flow = self.lower_stmt(stmt, flow)?;
        }

        // 在块退出时，对块内声明的 using 变量执行 dispose
        let new_using_count = self.active_using_vars.len();
        if new_using_count > prev_using_count {
            match flow {
                StmtFlow::Open(block) => {
                    let merged = self.emit_using_disposal(block);
                    self.active_using_vars.truncate(prev_using_count);
                    flow = StmtFlow::Open(merged);
                }
                StmtFlow::Terminated => {
                    // 块因 return/throw/break/continue 终止，
                    // using 变量的 dispose 由外层 finally 或运行时异常处理负责
                    self.active_using_vars.truncate(prev_using_count);
                }
            }
        }

        self.scopes.pop_scope();
        Ok(flow)
    }

    // ── if / else ───────────────────────────────────────────────────────────

    fn lower_if(
        &mut self,
        if_stmt: &swc_ast::IfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let cond = self.lower_expr(&if_stmt.test, block)?;
        let then_block = self.current_function.new_block();
        let else_or_merge = self.current_function.new_block();

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: cond,
                true_block: then_block,
                false_block: else_or_merge,
            },
        );

        let incoming_eval_completion = self.eval_completion;

        // lower 'then' branch
        let then_flow = self.lower_stmt(&if_stmt.cons, StmtFlow::Open(then_block))?;
        let then_eval_completion = self.eval_completion;

        let has_else = if let Some(alt) = &if_stmt.alt {
            self.eval_completion = incoming_eval_completion;
            // 'else' uses else_or_merge as its entry
            let else_flow = self.lower_stmt(alt, StmtFlow::Open(else_or_merge))?;
            let else_eval_completion = self.eval_completion;
            match (then_flow, else_flow) {
                (StmtFlow::Terminated, StmtFlow::Terminated) => StmtFlow::Terminated,
                _ => {
                    // Create a merge block only if at least one path doesn't terminate
                    let merge = self.current_function.new_block();
                    let after_then = self
                        .current_function
                        .ensure_jump_or_terminated(then_flow, merge);
                    let after_else = self
                        .current_function
                        .ensure_jump_or_terminated(else_flow, merge);
                    self.merge_eval_completion_after_if(
                        merge,
                        then_flow,
                        then_eval_completion,
                        after_then,
                        else_flow,
                        else_eval_completion,
                        after_else,
                    );
                    after_then
                }
            }
        } else {
            // No else: else_or_merge is the merge block (empty)
            // 即使 then 分支终止（break/return/continue），else 路径仍然可达
            let merge = else_or_merge;
            let _after_then = self
                .current_function
                .ensure_jump_or_terminated(then_flow, merge);
            if self.eval_mode {
                self.eval_completion = incoming_eval_completion.or(then_eval_completion);
            }
            StmtFlow::Open(merge)
        };

        Ok(has_else)
    }

    fn merge_eval_completion_after_if(
        &mut self,
        merge: BasicBlockId,
        then_flow: StmtFlow,
        then_eval_completion: Option<ValueId>,
        _after_then: StmtFlow,
        else_flow: StmtFlow,
        else_eval_completion: Option<ValueId>,
        _after_else: StmtFlow,
    ) {
        if !self.eval_mode {
            return;
        }

        let (StmtFlow::Open(then_block), Some(then_value)) = (then_flow, then_eval_completion)
        else {
            self.eval_completion = then_eval_completion.or(else_eval_completion);
            return;
        };
        let (StmtFlow::Open(else_block), Some(else_value)) = (else_flow, else_eval_completion)
        else {
            self.eval_completion = then_eval_completion.or(else_eval_completion);
            return;
        };

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: then_block,
                        value: then_value,
                    },
                    PhiSource {
                        predecessor: else_block,
                        value: else_value,
                    },
                ],
            },
        );
        self.eval_completion = Some(result);
    }

    // ── while ───────────────────────────────────────────────────────────────

    fn lower_while(
        &mut self,
        while_stmt: &swc_ast::WhileStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let header = self.current_function.new_block();
        let body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });
        let true_val = self.alloc_value();
        let true_const = self.module.add_constant(Constant::Bool(true));
        self.current_function.append_instruction(
            header,
            Instruction::Const {
                dest: true_val,
                constant: true_const,
            },
        );

        let cond = self.lower_expr(&while_stmt.test, header)?;
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                true_block: body,
                false_block: exit,
            },
        );

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(header),
            iterator_to_close: None,
        });

        let body_flow = self.lower_stmt(&while_stmt.body, StmtFlow::Open(body))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, header);

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── do...while ──────────────────────────────────────────────────────────

    fn lower_do_while(
        &mut self,
        do_while: &swc_ast::DoWhileStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let body = self.current_function.new_block();
        let condition = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: body });

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(condition),
            iterator_to_close: None,
        });

        let body_flow = self.lower_stmt(&do_while.body, StmtFlow::Open(body))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, condition);

        let cond = self.lower_expr(&do_while.test, condition)?;
        self.current_function.set_terminator(
            condition,
            Terminator::Branch {
                condition: cond,
                true_block: body,
                false_block: exit,
            },
        );

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for ─────────────────────────────────────────────────────────────────

    fn lower_for(
        &mut self,
        for_stmt: &swc_ast::ForStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        // init
        if let Some(init) = &for_stmt.init {
            match init {
                swc_ast::VarDeclOrExpr::VarDecl(var_decl) => {
                    self.lower_var_decl(var_decl, StmtFlow::Open(block))?;
                }
                swc_ast::VarDeclOrExpr::Expr(expr) => {
                    let _ = self.lower_expr(expr, block)?;
                }
            }
        }

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let update = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(update),
            iterator_to_close: None,
        });

        // condition
        if let Some(test) = &for_stmt.test {
            let cond = self.lower_expr(test, header)?;
            self.current_function.set_terminator(
                header,
                Terminator::Branch {
                    condition: cond,
                    true_block: body_block,
                    false_block: exit,
                },
            );
        } else {
            // no condition → always true
            let true_val = self.load_bool_constant(true, header);
            self.current_function.set_terminator(
                header,
                Terminator::Branch {
                    condition: true_val,
                    true_block: body_block,
                    false_block: exit,
                },
            );
        }

        // body
        let body_flow = self.lower_stmt(&for_stmt.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, update);

        // update
        if let Some(update_expr) = &for_stmt.update {
            let _ = self.lower_expr(update_expr, update)?;
        }
        self.current_function
            .set_terminator(update, Terminator::Jump { target: header });

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for...in ────────────────────────────────────────────────────────────

    fn lower_for_in(
        &mut self,
        for_in: &swc_ast::ForInStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let rhs = self.lower_expr(&for_in.right, block)?;

        // Create enumerator from object
        let enum_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(enum_handle),
                builtin: Builtin::EnumeratorFrom,
                args: vec![rhs],
            },
        );

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let next = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: check enumerator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::EnumeratorDone,
                args: vec![enum_handle],
            },
        );
        // 反转 done 条件：backend 假设 loop condition "true = continue",
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(next),
            iterator_to_close: None,
        });

        // body: get key, assign lhs
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::CallBuiltin {
                dest: Some(key_val),
                builtin: Builtin::EnumeratorKey,
                args: vec![enum_handle],
            },
        );

        self.lower_for_in_of_lhs(&for_in.left, key_val, body_block)?;

        let body_flow = self.lower_stmt(&for_in.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next);

        // next
        self.current_function.append_instruction(
            next,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::EnumeratorNext,
                args: vec![enum_handle],
            },
        );
        self.current_function
            .set_terminator(next, Terminator::Jump { target: header });

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for...of ────────────────────────────────────────────────────────────

    fn lower_for_of(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if for_of.is_await {
            return self.lower_for_await_of(for_of, flow);
        }
        let block = self.ensure_open(flow)?;

        let iterable = self.lower_expr(&for_of.right, block)?;

        // Create iterator from iterable
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
            },
        );

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let next_block = self.current_function.new_block();
        let close = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: check iterator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );
        // 反转 done 条件：backend 假设 loop condition "true = continue",
        // 但 done_val 是 "true = done = exit"。使用 Not 反转。
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        // Register label context: break → close (which then jumps to exit)
        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: close,
            continue_target: Some(next_block),
            iterator_to_close: Some(iter_handle),
        });

        // body: get value, assign lhs
        let value_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::CallBuiltin {
                dest: Some(value_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );

        self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next_block);

        self.label_stack.pop();

        // close block: iterator clean-up on break
        self.current_function.append_instruction(
            close,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(close, Terminator::Jump { target: exit });

        // next: advance iterator
        self.current_function.append_instruction(
            next_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(next_block, Terminator::Jump { target: header });

        Ok(StmtFlow::Open(exit))
    }

    fn lower_for_await_of(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if !self.is_async_fn {
            return Err(self.error(
                for_of.span(),
                "for await...of is only valid in async functions",
            ));
        }

        let block = self.ensure_open(flow)?;

        let iterable = self.lower_expr(&for_of.right, block)?;

        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
            },
        );
        let iter_binding = format!("$for_await_iter.{}", self.next_temp);
        self.next_temp += 1;
        let iter_scope_id = self
            .scopes
            .declare(&iter_binding, VarKind::Let, true)
            .map_err(|msg| self.error(for_of.span(), msg))?;
        let iter_ir_name = format!("${iter_scope_id}.{iter_binding}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: iter_ir_name.clone(),
                value: iter_handle,
            },
        );

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let close = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        let iter_for_next = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::LoadVar {
                dest: iter_for_next,
                name: iter_ir_name.clone(),
            },
        );
        let next_call_result = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(next_call_result),
                builtin: Builtin::IteratorNext,
                args: vec![iter_for_next],
            },
        );

        let next_result = self.alloc_value();
        {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                header,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                header,
                Instruction::CallBuiltin {
                    dest: Some(next_result),
                    builtin: Builtin::PromiseResolveStatic,
                    args: vec![undef_val, next_call_result],
                },
            );
        }

        let next_state = self.async_state_counter;
        self.async_state_counter += 1;

        let resume_block = self.current_function.new_block();
        self.async_resume_blocks.push((next_state, resume_block));
        let saved_bindings = self.async_visible_binding_names();
        self.emit_save_async_bindings(header, &saved_bindings);

        self.current_function.append_instruction(
            header,
            Instruction::Suspend {
                promise: next_result,
                state: next_state,
            },
        );

        let continue_after_await = self.current_function.new_block();
        self.current_function.set_terminator(
            header,
            Terminator::Jump {
                target: continue_after_await,
            },
        );

        self.emit_restore_async_bindings(resume_block, &saved_bindings);
        let resume_val = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: resume_val,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let is_rejected = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: is_rejected,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );
        let throw_block = self.current_function.new_block();
        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_rejected,
                true_block: throw_block,
                false_block: continue_after_await,
            },
        );
        self.emit_throw_value(throw_block, resume_val)?;

        let awaited_result = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::LoadVar {
                dest: awaited_result,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let done_key_const = self
            .module
            .add_constant(Constant::String("done".to_string()));
        let done_key = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::Const {
                dest: done_key,
                constant: done_key_const,
            },
        );
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::GetProp {
                dest: done_val,
                object: awaited_result,
                key: done_key,
            },
        );
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            continue_after_await,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        let iter_for_body_close = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::LoadVar {
                dest: iter_for_body_close,
                name: iter_ir_name.clone(),
            },
        );
        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: close,
            continue_target: Some(header),
            iterator_to_close: Some(iter_for_body_close),
        });

        let awaited_result_for_value = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::LoadVar {
                dest: awaited_result_for_value,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let value_key_const = self
            .module
            .add_constant(Constant::String("value".to_string()));
        let value_key = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::Const {
                dest: value_key,
                constant: value_key_const,
            },
        );
        let value_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::GetProp {
                dest: value_val,
                object: awaited_result_for_value,
                key: value_key,
            },
        );

        self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, header);

        self.label_stack.pop();

        let iter_for_close = self.alloc_value();
        self.current_function.append_instruction(
            close,
            Instruction::LoadVar {
                dest: iter_for_close,
                name: iter_ir_name,
            },
        );
        self.current_function.append_instruction(
            close,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_for_close],
            },
        );
        self.current_function
            .set_terminator(close, Terminator::Jump { target: exit });

        Ok(StmtFlow::Open(exit))
    }

    /// Lower the LHS of a for...in or for...of loop.
    /// Supports: simple identifier, or var declaration with single binding identifier.
    fn lower_for_in_of_lhs(
        &mut self,
        left: &swc_ast::ForHead,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        match left {
            swc_ast::ForHead::Pat(pat) => {
                match &**pat {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        let (scope_id, _) = self
                            .scopes
                            .lookup(&name)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.current_function.append_instruction(
                            block,
                            Instruction::StoreVar {
                                name: ir_name,
                                value,
                            },
                        );
                        Ok(())
                    }
                    swc_ast::Pat::Object(_) | swc_ast::Pat::Array(_) | swc_ast::Pat::Assign(_) => {
                        self.lower_destructure_pattern(pat, value, block, VarKind::Let)
                    }
                    _ => Err(self.error(
                        pat.span(),
                        "destructuring patterns in for...in/for...of are not yet supported",
                    )),
                }
            }
            swc_ast::ForHead::VarDecl(var_decl) => {
                let kind = match var_decl.kind {
                    swc_ast::VarDeclKind::Var => VarKind::Var,
                    swc_ast::VarDeclKind::Let => VarKind::Let,
                    swc_ast::VarDeclKind::Const => VarKind::Const,
                };
                for declarator in &var_decl.decls {
                    match &declarator.name {
                        swc_ast::Pat::Ident(binding) => {
                            let name = binding.id.sym.to_string();
                            let scope_id = self
                                .scopes
                                .resolve_scope_id(&name)
                                .map_err(|msg| self.error(var_decl.span, msg))?;
                            self.scopes
                                .mark_initialised(&name)
                                .map_err(|msg| self.error(var_decl.span, msg))?;
                            let ir_name = format!("${scope_id}.{name}");
                            self.current_function.append_instruction(
                                block,
                                Instruction::StoreVar {
                                    name: ir_name,
                                    value,
                                },
                            );
                        }
                        _ => {
                            self.lower_destructure_pattern(&declarator.name, value, block, kind)?;
                        }
                    }
                }
                Ok(())
            }
            swc_ast::ForHead::UsingDecl(_) => Err(self.error(
                DUMMY_SP,
                "using declarations in for...in/for...of are not yet supported",
            )),
        }
    }

    // ── break / continue ────────────────────────────────────────────────────

    fn lower_break(
        &mut self,
        break_stmt: &swc_ast::BreakStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let target_index = if let Some(label) = &break_stmt.label {
            self.find_label_context_index(&label.sym.to_string(), Some(label.span))?
        } else {
            self.find_nearest_break_context_index(break_stmt.span())?
        };
        let target = self.label_stack[target_index].break_target;
        let iterator_cleanups = self.iterator_cleanups_crossing(target_index);

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
                self.emit_iterator_closes(after_finally, &iterator_cleanups);
                self.current_function
                    .set_terminator(after_finally, Terminator::Jump { target });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    fn lower_continue(
        &mut self,
        continue_stmt: &swc_ast::ContinueStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let target_index = if let Some(label) = &continue_stmt.label {
            let index = self.find_label_context_index(&label.sym.to_string(), Some(label.span))?;
            if self.label_stack[index].continue_target.is_none() {
                return Err(self.error(
                    continue_stmt.span(),
                    format!("cannot continue to non-loop label `{}`", label.sym),
                ));
            }
            index
        } else {
            self.find_nearest_continue_context_index(continue_stmt.span())?
        };
        let target = self.label_stack[target_index]
            .continue_target
            .expect("continue target checked above");
        let iterator_cleanups = self.iterator_cleanups_crossing(target_index);

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
                self.emit_iterator_closes(after_finally, &iterator_cleanups);
                self.current_function
                    .set_terminator(after_finally, Terminator::Jump { target });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    fn find_nearest_break_context_index(&self, span: Span) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if matches!(ctx.kind, LabelKind::Loop | LabelKind::Switch | LabelKind::Block) {
                return Ok(index);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0,
            span.hi.0,
            "break outside of loop or switch",
        )))
    }

    fn find_nearest_continue_context_index(&self, span: Span) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if ctx.continue_target.is_some() {
                return Ok(index);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0,
            span.hi.0,
            "continue outside of loop",
        )))
    }

    fn find_label_context_index(
        &self,
        name: &str,
        error_span: Option<Span>,
    ) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if ctx.label.as_deref() == Some(name) {
                return Ok(index);
            }
        }
        let (start, end) = match error_span {
            Some(span) => (span.lo.0, span.hi.0),
            None => (0, 0),
        };
        Err(LoweringError::Diagnostic(Diagnostic::new(
            start,
            end,
            format!("unknown label `{name}`"),
        )))
    }

    fn iterator_cleanups_crossing(&self, target_index: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(target_index + 1)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    fn iterator_cleanups_from_depth(&self, depth: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(depth)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    fn active_iterator_cleanups(&self) -> Vec<ValueId> {
        self.iterator_cleanups_from_depth(0)
    }

    fn emit_iterator_closes(&mut self, block: BasicBlockId, iterators: &[ValueId]) {
        for iterator in iterators {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::IteratorClose,
                    args: vec![*iterator],
                },
            );
        }
    }

    // ── labeled ─────────────────────────────────────────────────────────────

    fn lower_labeled(
        &mut self,
        labeled: &swc_ast::LabeledStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let label_name = labeled.label.sym.to_string();

        if self
            .label_stack
            .iter()
            .any(|ctx| ctx.label.as_deref() == Some(label_name.as_str()))
            || self.pending_loop_label.as_deref() == Some(label_name.as_str())
        {
            return Err(self.error(
                labeled.label.span,
                format!("duplicate label `{label_name}`"),
            ));
        }

        let is_loop_body = matches!(
            labeled.body.as_ref(),
            swc_ast::Stmt::While(_)
                | swc_ast::Stmt::DoWhile(_)
                | swc_ast::Stmt::For(_)
                | swc_ast::Stmt::ForIn(_)
                | swc_ast::Stmt::ForOf(_)
        );

        if is_loop_body {
            let previous = self.pending_loop_label.replace(label_name);
            let inner_flow = self.lower_stmt(&labeled.body, StmtFlow::Open(block));
            self.pending_loop_label = previous;
            return inner_flow;
        }

        let exit = self.current_function.new_block();
        self.label_stack.push(LabelContext {
            label: Some(label_name),
            kind: LabelKind::Block,
            break_target: exit,
            continue_target: None,
            iterator_to_close: None,
        });

        let inner_flow = self.lower_stmt(&labeled.body, StmtFlow::Open(block))?;
        let after = self
            .current_function
            .ensure_jump_or_terminated(inner_flow, exit);

        self.label_stack.pop();
        Ok(after)
    }

    // ── return ──────────────────────────────────────────────────────────────

    fn lower_return(
        &mut self,
        return_stmt: &swc_ast::ReturnStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let iterator_cleanups = self.active_iterator_cleanups();

        if self.is_async_fn {
            let value = if let Some(arg) = &return_stmt.arg {
                self.lower_expr(arg, block)?
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

            let return_block = self.resolve_store_block(block);
            match self.lower_pending_finalizers(return_block)? {
                StmtFlow::Open(after_finally) => {
                    self.emit_iterator_closes(after_finally, &iterator_cleanups);
                    if self.is_async_generator_fn {
                        let gen_val = self.alloc_value();
                        self.current_function.append_instruction(
                            after_finally,
                            Instruction::LoadVar {
                                dest: gen_val,
                                name: format!("${}.$generator", self.async_generator_scope_id),
                            },
                        );
                        self.current_function.append_instruction(
                            after_finally,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::AsyncGeneratorReturn,
                                args: vec![gen_val, value],
                            },
                        );
                    } else {
                        let promise_val = self.alloc_value();
                        self.current_function.append_instruction(
                            after_finally,
                            Instruction::LoadVar {
                                dest: promise_val,
                                name: format!("${}.$promise", self.async_promise_scope_id),
                            },
                        );
                        self.current_function.append_instruction(
                            after_finally,
                            Instruction::PromiseResolve {
                                promise: promise_val,
                                value,
                            },
                        );
                    }
                    self.current_function
                        .set_terminator(after_finally, Terminator::Return { value: None });
                }
                StmtFlow::Terminated => {}
            }
            return Ok(StmtFlow::Terminated);
        }

        let value = if let Some(arg) = &return_stmt.arg {
            Some(self.lower_expr(arg, block)?)
        } else {
            None
        };

        let return_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(return_block)? {
            StmtFlow::Open(after_finally) => {
                self.emit_iterator_closes(after_finally, &iterator_cleanups);
                self.current_function
                    .set_terminator(after_finally, Terminator::Return { value });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    // ── switch ──────────────────────────────────────────────────────────────

    fn lower_switch(
        &mut self,
        switch_stmt: &swc_ast::SwitchStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let discr = self.lower_expr(&switch_stmt.discriminant, block)?;

        let exit = self.current_function.new_block();
        // 性能优化：预分配容量避免循环中多次 reallocation
        let case_count = switch_stmt.cases.len();
        let mut cases: Vec<SwitchCaseTarget> = Vec::with_capacity(case_count);
        let mut case_blocks: Vec<BasicBlockId> = Vec::with_capacity(case_count);
        let mut default_pos: Option<usize> = None;

        // Generate a case block for each case
        for case in &switch_stmt.cases {
            if case.test.is_none() {
                // default case — 记录其在 cases 中的位置
                default_pos = Some(case_blocks.len());
            }

            let case_block = self.current_function.new_block();
            case_blocks.push(case_block);

            if let Some(test) = &case.test {
                // Compare discriminant with case value
                let _cond_val = self.lower_binary_op_with_const(test, discr, block)?;
                cases.push(SwitchCaseTarget {
                    constant: self.extract_constant_from_expr(test)?,
                    target: case_block,
                });
            }
        }

        // 设置 switch terminator：default 指向 case_blocks[default_pos]，无 default 则指向 exit
        let default_target = default_pos.map(|p| case_blocks[p]).unwrap_or(exit);

        self.current_function.set_terminator(
            block,
            Terminator::Switch {
                value: discr,
                cases,
                default_block: default_target,
                exit_block: exit,
            },
        );

        // Lower case bodies
        self.label_stack.push(LabelContext {
            label: None,
            kind: LabelKind::Switch,
            break_target: exit,
            continue_target: None,
            iterator_to_close: None,
        });

        for (i, case) in switch_stmt.cases.iter().enumerate() {
            let case_block = case_blocks[i];
            let mut case_flow = StmtFlow::Open(case_block);

            for stmt in &case.cons {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(case_flow, StmtFlow::Terminated) {
                    continue;
                }
                case_flow = self.lower_stmt(stmt, case_flow)?;
            }

            // Fall-through: if not terminated, jump to next case or exit
            let next_target = if i + 1 < case_blocks.len() {
                case_blocks[i + 1]
            } else {
                exit
            };
            let _ = self
                .current_function
                .ensure_jump_or_terminated(case_flow, next_target);
        }

        // NOTE: default case body 已在上面的 case 循环中一并降低，
        // fallthrough 也由循环中的 ensure_jump_or_terminated 处理，无需单独处理。

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    /// Lower a binary comparison with a constant for switch case matching.
    fn lower_binary_op_with_const(
        &mut self,
        _test: &swc_ast::Expr,
        discr: ValueId,
        _block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // For switch cases, the comparison is implicit StrictEq between discr and case value.
        // This will be handled by the Switch terminator at compile time.
        // We just return the discriminant value for now; the backend handles the comparison.
        Ok(discr)
    }

    fn extract_constant_from_expr(
        &mut self,
        expr: &swc_ast::Expr,
    ) -> Result<ConstantId, LoweringError> {
        match expr {
            swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) => {
                Ok(self.module.add_constant(Constant::Number(num.value)))
            }
            swc_ast::Expr::Lit(swc_ast::Lit::Str(s)) => Ok(self
                .module
                .add_constant(Constant::String(s.value.to_string_lossy().into_owned()))),
            swc_ast::Expr::Lit(swc_ast::Lit::Bool(b)) => {
                Ok(self.module.add_constant(Constant::Bool(b.value)))
            }
            swc_ast::Expr::Lit(swc_ast::Lit::Null(_)) => {
                Ok(self.module.add_constant(Constant::Null))
            }
            _ => Err(self.error(expr.span(), "switch case must be a literal")),
        }
    }

    // ── throw ───────────────────────────────────────────────────────────────

    fn emit_async_reject(&mut self, block: BasicBlockId, reason: ValueId) {
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
            Instruction::PromiseReject {
                promise: promise_val,
                reason,
            },
        );
        self.current_function
            .set_terminator(block, Terminator::Return { value: None });
    }

    fn emit_throw_value(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<StmtFlow, LoweringError> {
        if let Some(try_ctx) = self.try_contexts.last() {
            if let Some(catch_entry) = try_ctx.catch_entry {
                let exc_var = try_ctx.exception_var.clone();
                let iterator_cleanups = self.iterator_cleanups_from_depth(try_ctx.label_depth);
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: exc_var,
                        value,
                    },
                );
                self.emit_iterator_closes(block, &iterator_cleanups);
                self.current_function.set_terminator(
                    block,
                    Terminator::Jump {
                        target: catch_entry,
                    },
                );
                return Ok(StmtFlow::Terminated);
            }
        }

        let throw_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(throw_block)? {
            StmtFlow::Open(after_finally) => {
                let iterator_cleanups = self.active_iterator_cleanups();
                self.emit_iterator_closes(after_finally, &iterator_cleanups);
                if self.is_async_generator_fn {
                    let gen_val = self.alloc_value();
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::LoadVar {
                            dest: gen_val,
                            name: format!("${}.$generator", self.async_generator_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::AsyncGeneratorThrow,
                            args: vec![gen_val, value],
                        },
                    );
                    self.current_function
                        .set_terminator(after_finally, Terminator::Return { value: None });
                } else if self.is_async_fn {
                    self.emit_async_reject(after_finally, value);
                } else {
                    self.current_function
                        .set_terminator(after_finally, Terminator::Throw { value });
                }
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    fn lower_throw(
        &mut self,
        throw_stmt: &swc_ast::ThrowStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let value = self.lower_expr(&throw_stmt.arg, block)?;
        self.emit_throw_value(block, value)
    }

    // ── try / catch / finally ───────────────────────────────────────────────

    fn lower_try(
        &mut self,
        try_stmt: &swc_ast::TryStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        // We need to save the current completion state
        // For the initial implementation, we create blocks for the try body,
        // catch body, and finally body, and manage the control flow manually.
        let block = self.ensure_open(flow)?;

        let try_body = self.current_function.new_block();
        let catch_entry = self.current_function.new_block();
        let finally_entry = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: try_body });

        // 推入 try context 以便 lower_throw 能重定向到 catch
        let exc_var = self.alloc_temp_name();
        let has_catch = try_stmt.handler.is_some();
        self.try_contexts.push(TryContext {
            catch_entry: if has_catch { Some(catch_entry) } else { None },
            exception_var: exc_var,
            label_depth: self.label_stack.len(),
        });

        if let Some(finally) = &try_stmt.finalizer {
            self.active_finalizers.push(finally.clone());
        }

        // Lower try body
        let try_flow = self.lower_block_body(&try_stmt.block, StmtFlow::Open(try_body))?;

        // After try body, if not terminated, jump to finally
        if let Some(finally) = &try_stmt.finalizer {
            // There is a finally block
            self.finally_stack.push(FinallyContext {
                _finally_block: finally_entry,
                _after_finally_block: exit,
            });
            let _ = self
                .current_function
                .ensure_jump_or_terminated(try_flow, finally_entry);

            // Lower catch body if present
            if let Some(catch) = &try_stmt.handler {
                eprintln!("DEBUG: catch param = {:?}", catch.param.as_ref().map(|p| std::mem::discriminant(p)));
                // Lower catch clause: bind parameter if present
                self.scopes.push_scope(ScopeKind::Block);
                if let Some(param) = &catch.param {
                    match param {
                        swc_ast::Pat::Ident(binding) => {
                            let name = binding.id.sym.to_string();
                            // 从 try context 获取异常变量名，用 LoadVar 加载
                            let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                            let exc_val = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::LoadVar {
                                    dest: exc_val,
                                    name: exc_var,
                                },
                            );
                            let scope_id = self
                                .scopes
                                .declare(&name, VarKind::Let, true)
                                .map_err(|msg| self.error(param.span(), msg))?;
                            let ir_name = format!("${scope_id}.{name}");
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::StoreVar {
                                    name: ir_name,
                                    value: exc_val,
                                },
                            );
                        }
                        _ => {
                            let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                            let exc_val = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::LoadVar {
                                    dest: exc_val,
                                    name: exc_var,
                                },
                            );
                            let mut names = Vec::new();
                            Self::extract_pat_bindings(&[param.clone()], &mut names);
                            eprintln!("DEBUG: catch destructure names: {:?}", names);
                            for name in &names {
                                eprintln!("DEBUG: declaring {} in scope", name);
                                self.scopes
                                    .declare(name, VarKind::Let, true)
                                    .map_err(|msg| self.error(param.span(), msg))?;
                            }
                            self.lower_destructure_pattern(param, exc_val, catch_entry, VarKind::Let)?;
                        }
                    }
                }

                // Lower catch body
                let catch_body_flow =
                    self.lower_block_body(&catch.body, StmtFlow::Open(catch_entry))?;
                let _ = self
                    .current_function
                    .ensure_jump_or_terminated(catch_body_flow, finally_entry);
                self.scopes.pop_scope();
            } else {
                // No catch: rethrow from catch_entry goes to finally
                let _ = self
                    .current_function
                    .ensure_jump_or_terminated(StmtFlow::Open(catch_entry), finally_entry);
            }
            self.active_finalizers.pop();

            // Lower finally
            let finally_flow = self.lower_block_body(finally, StmtFlow::Open(finally_entry))?;
            let _ = self
                .current_function
                .ensure_jump_or_terminated(finally_flow, exit);

            self.finally_stack.pop();
        } else if let Some(catch) = &try_stmt.handler {
            // try/catch without finally
            self.scopes.push_scope(ScopeKind::Block);
            if let Some(param) = &catch.param {
                match param {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        // 从 try context 获取异常变量名，用 LoadVar 加载
                        let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                        let exc_val = self.alloc_value();
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::LoadVar {
                                dest: exc_val,
                                name: exc_var,
                            },
                        );
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(param.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::StoreVar {
                                name: ir_name,
                                value: exc_val,
                            },
                        );
                    }
                    _ => {}
                }
            }

            let catch_flow = self.lower_block_body(&catch.body, StmtFlow::Open(catch_entry))?;
            let _ = self
                .current_function
                .ensure_jump_or_terminated(catch_flow, exit);
            self.scopes.pop_scope();

            // Set catch entry as the throw target for the try body
            // If try body throws, it jumps to catch_entry
            // Uncaught throw will terminate. For now, try body that throws jumps to catch_entry.
            let _ = self
                .current_function
                .ensure_jump_or_terminated(try_flow, exit);
        }

        self.try_contexts.pop();
        Ok(StmtFlow::Open(exit))
    }

    fn lower_block_body(
        &mut self,
        block_stmt: &swc_ast::BlockStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        self.scopes.push_scope(ScopeKind::Block);
        self.predeclare_block_stmts(&block_stmt.stmts)?;

        let mut flow = flow;
        for stmt in &block_stmt.stmts {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            flow = self.lower_stmt(stmt, flow)?;
        }

        self.scopes.pop_scope();
        Ok(flow)
    }

    // ── Empty / Debugger / With ─────────────────────────────────────────────

    fn lower_empty(&self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
        Ok(flow)
    }

    fn lower_debugger(&mut self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::Debugger,
                args: vec![],
            },
        );
        Ok(StmtFlow::Open(block))
    }

    fn lower_with(
        &self,
        _with_stmt: &swc_ast::WithStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let _block = self.ensure_open(flow)?;
        Err(self.error(
            _with_stmt.span(),
            "with statement is not supported in strict/static scope mode",
        ))
    }

    // ── Variable declarations ───────────────────────────────────────────────

    fn lower_var_decl(
        &mut self,
        var_decl: &swc_ast::VarDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let kind = match var_decl.kind {
            swc_ast::VarDeclKind::Var => VarKind::Var,
            swc_ast::VarDeclKind::Let => VarKind::Let,
            swc_ast::VarDeclKind::Const => VarKind::Const,
        };

        for declarator in &var_decl.decls {
            if let Some(init) = &declarator.init {
                let value = self.lower_expr(init, block)?;
                self.lower_destructure_pattern(&declarator.name, value, block, kind)?;
                // lower_expr may have terminated the block; use resolve_store_block
                let store_block = self.resolve_store_block(block);
                let flow_block = self.resolve_open_after_expr(block, store_block);
                return Ok(StmtFlow::Open(flow_block));
            } else {
                if matches!(kind, VarKind::Const) {
                    return Err(self.error(var_decl.span, "const declarations must be initialised"));
                }
                if matches!(kind, VarKind::Var) {
                    // var without init: already initialised in pre-scan, skip
                    let mut names = Vec::new();
                    Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
                    for name in names {
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(var_decl.span, msg))?;
                    }
                    continue;
                }

                // `let x;`（非解构）或 `let [a, b];` — 初始化为 undefined
                let undef_cid = self.module.add_constant(Constant::Undefined);
                let undef_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_cid,
                    },
                );
                self.lower_destructure_pattern(&declarator.name, undef_val, block, kind)?;
            }
        }

        Ok(StmtFlow::Open(block))
    }

    // ── Destructuring pattern lowering ──────────────────────────────────────

    /// 构建函数参数的 param_ir_names 并声明变量。
    ///   - 简单参数 (x): 直接使用变量名
    ///   - 简单参数 + 默认值 (x = 1): 直接使用变量名
    ///   - 解构参数 ({a}) / 解构+默认值 ([a] = [1]): 使用临时变量名
    fn build_param_ir_names(
        &mut self,
        params: &[swc_ast::Param],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    /// 为箭头函数的参数（Vec<Pat>）构建 param_ir_names.
    fn build_arrow_param_ir_names(
        &mut self,
        params: &[swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    fn build_param_ir_names_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        let mut ir_names: Vec<String> = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        for pat in pats {
            match pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(binding.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{name}"));
                }
                swc_ast::Pat::Assign(assign) => match &*assign.left {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(binding.span(), msg))?;
                        ir_names.push(format!("${scope_id}.{name}"));
                    }
                    _ => {
                        let temp = self.alloc_temp_name();
                        let scope_id = self
                            .scopes
                            .declare(&temp, VarKind::Let, true)
                            .map_err(|msg| self.error(assign.span, msg))?;
                        ir_names.push(format!("${scope_id}.{temp}"));
                        let mut nested = Vec::new();
                        Self::extract_pat_bindings(&[*assign.left.clone()], &mut nested);
                        for n in &nested {
                            self.scopes
                                .declare(n, VarKind::Let, true)
                                .map_err(|msg| self.error(assign.span, msg))?;
                        }
                    }
                },
                swc_ast::Pat::Rest(rest) => {
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[*rest.arg.clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
                _ => {
                    let temp = self.alloc_temp_name();
                    let scope_id = self
                        .scopes
                        .declare(&temp, VarKind::Let, true)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{temp}"));
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[(*pat).clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
            }
        }

        Ok(ir_names)
    }

    /// 在函数体入口生成参数初始化代码（默认值 + 解构）。
    fn emit_param_inits(
        &mut self,
        params: &[swc_ast::Param],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    fn emit_arrow_param_inits(
        &mut self,
        pats: &[swc_ast::Pat],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            pats.iter().collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    fn emit_arguments_init(
        &mut self,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let scope_id = self
            .scopes
            .declare("arguments", VarKind::Let, true)
            .expect("arguments declaration should not fail");
        let ir_name = format!("${scope_id}.arguments");
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CollectRestArgs { dest, skip: 0 },
        );
        let store_block = self.resolve_store_block(block);
        self.current_function.append_instruction(
            store_block,
            Instruction::StoreVar {
                name: ir_name,
                value: dest,
            },
        );
        self.scopes
            .mark_initialised("arguments")
            .expect("arguments init should not fail");
        Ok(self.resolve_store_block(block))
    }

    fn emit_pat_inits_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        param_ir_names: &[String],
        mut block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // param_ir_names[0] = $env, [1] = $this, [2..] = user params (excluding rest)
        let mut ir_name_idx: usize = 2;
        let mut regular_param_count: u32 = 0;
        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(_) = pat {
                break;
            }
            regular_param_count += 1;
        }

        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(rest) = pat {
                let skip = regular_param_count;
                let rest_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CollectRestArgs {
                        dest: rest_val,
                        skip,
                    },
                );
                self.lower_destructure_pattern(&rest.arg, rest_val, block, VarKind::Let)?;
                block = self.resolve_store_block(block);
                break;
            }

            let ir_name = &param_ir_names[ir_name_idx];
            match pat {
                swc_ast::Pat::Ident(_) => {
                    // 简单参数无默认值：值已在 local 中，无需操作
                }
                swc_ast::Pat::Assign(assign) => {
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    let resolved = self.lower_default_value_check(raw, &assign.right, block)?;
                    let store_block = self.resolve_store_block(block);
                    self.current_function.append_instruction(
                        store_block,
                        Instruction::StoreVar {
                            name: ir_name.clone(),
                            value: resolved,
                        },
                    );

                    if !matches!(&*assign.left, swc_ast::Pat::Ident(_)) {
                        let loaded = self.alloc_value();
                        self.current_function.append_instruction(
                            store_block,
                            Instruction::LoadVar {
                                dest: loaded,
                                name: ir_name.clone(),
                            },
                        );
                        self.lower_destructure_pattern(
                            &assign.left,
                            loaded,
                            store_block,
                            VarKind::Let,
                        )?;
                    }
                }
                _ => {
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    self.lower_destructure_pattern(pat, raw, block, VarKind::Let)?;
                }
            }
            ir_name_idx += 1;
            block = self.resolve_open_after_expr(block, self.resolve_store_block(block));
        }
        Ok(block)
    }

    /// 将解构 pattern 降低为一系列 IR 指令（GetProp/GetElem + StoreVar）。
    /// 递归处理嵌套的 Array/Object/Assign pattern。
    fn lower_destructure_pattern(
        &mut self,
        pat: &swc_ast::Pat,
        src_val: ValueId,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        match pat {
            swc_ast::Pat::Ident(binding) => {
                let name = binding.id.sym.to_string();
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(pat.span(), msg))?;
                let ir_name = format!("${scope_id}.{name}");

                if matches!(kind, VarKind::Var) {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                } else {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                }

                let store_block = self.resolve_store_block(block);
                self.current_function.append_instruction(
                    store_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: src_val,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, src_val, store_block);
            }
            swc_ast::Pat::Object(object_pat) => {
                self.lower_object_destructure(object_pat, src_val, block, kind)?;
            }
            swc_ast::Pat::Array(array_pat) => {
                self.lower_array_destructure(array_pat, src_val, block, kind)?;
            }
            swc_ast::Pat::Assign(assign_pat) => {
                let resolved = self.lower_default_value_check(src_val, &assign_pat.right, block)?;
                self.lower_destructure_pattern(&assign_pat.left, resolved, block, kind)?;
            }
            swc_ast::Pat::Rest(_) => {
                return Err(self.error(
                    pat.span(),
                    "rest element must be used as a function parameter or inside array destructuring",
                ));
            }
            swc_ast::Pat::Expr(_) | swc_ast::Pat::Invalid(_) => {}
        }
        Ok(())
    }

    /// 对象解构: `{ prop1, prop2: alias, ...rest }`
    fn lower_object_destructure(
        &mut self,
        object_pat: &swc_ast::ObjectPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        for prop in &object_pat.props {
            match prop {
                swc_ast::ObjectPatProp::KeyValue(kv) => {
                    // { key: pattern } 或 { [computed]: pattern }
                    let key_val = self.lower_prop_name(&kv.key, block)?;
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );
                    self.lower_destructure_pattern(&kv.value, dest, block, kind)?;
                }
                swc_ast::ObjectPatProp::Assign(assign) => {
                    // { key } 等价于 { key: key }
                    let name = assign.key.id.sym.to_string();
                    let key_const = self.module.add_constant(Constant::String(name.clone()));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );

                    // 如果有默认值 { key = default }
                    if let Some(default_expr) = &assign.value {
                        let resolved = self.lower_default_value_check(dest, default_expr, block)?;
                        let scope_id = self
                            .scopes
                            .resolve_scope_id(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        let store_block = self.resolve_store_block(block);
                        self.current_function.append_instruction(
                            store_block,
                            Instruction::StoreVar {
                                name: ir_name,
                                value: resolved,
                            },
                        );
                        self.append_eval_var_leak_if_needed(&name, kind, resolved, store_block);
                    } else {
                        self.lower_destructure_pattern(
                            &swc_ast::Pat::Ident(assign.key.clone()),
                            dest,
                            block,
                            kind,
                        )?;
                    }
                }
                swc_ast::ObjectPatProp::Rest(rest) => {
                    // { ...rest } — 使用 ObjectRest builtin
                    let rest_dest = self.alloc_value();
                    let excluded_cid = self.module.add_constant(Constant::Undefined);
                    let excluded_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: excluded_val,
                            constant: excluded_cid,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(rest_dest),
                            builtin: Builtin::ObjectRest,
                            args: vec![src_val, excluded_val],
                        },
                    );
                    self.lower_destructure_pattern(&rest.arg, rest_dest, block, kind)?;
                }
            }
            // 确保 block 指向当前可用的基本块（可能已被 lower_default_value_check 等终结）
            block = self.resolve_store_block(block);
        }
        Ok(())
    }

    /// 数组解构: `[a, b, ...rest]`
    fn lower_array_destructure(
        &mut self,
        array_pat: &swc_ast::ArrayPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        let mut idx: i32 = 0;
        let mut has_rest = false;

        for (_i, elem) in array_pat.elems.iter().enumerate() {
            let elem = match elem {
                Some(e) => e,
                None => {
                    idx += 1;
                    continue;
                }
            };

            if let swc_ast::Pat::Rest(rest) = elem {
                has_rest = true;
                // [...rest] — 使用 iterator 协议收集剩余元素
                let rest_val =
                    self.lower_array_rest_destructure(src_val, &rest.arg, idx, block, kind)?;
                // rest.arg 可能是 Pat::Ident 或嵌套 pattern
                // 但我们已经在 lower_array_rest_destructure 中处理了
                let _ = rest_val;
            } else {
                // 普通元素 get_elem
                let index_const = self.module.add_constant(Constant::Number(idx as f64));
                let index_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: index_val,
                        constant: index_const,
                    },
                );

                // 如果有默认值，用 IteratorFrom/Next/Value/Done 或简单 undefined 检查
                if let swc_ast::Pat::Assign(assign) = elem {
                    // [a = default] — 需要检查数组对应位置是否为 undefined（不是 nullish）
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: src_val,
                            index: index_val,
                        },
                    );
                    let resolved = self.lower_default_value_check(dest, &assign.right, block)?;
                    self.lower_destructure_pattern(&assign.left, resolved, block, kind)?;
                } else {
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: src_val,
                            index: index_val,
                        },
                    );
                    self.lower_destructure_pattern(elem, dest, block, kind)?;
                }
                idx += 1;
            }
            block = self.resolve_store_block(block);
        }

        if has_rest {
            // 如果使用了 rest，需要关闭 iterator
            // 在此上下文中不需要显式 IteratorClose — 已由 lower_array_rest_destructure 处理
        }

        Ok(())
    }

    /// 数组解构中的 rest 元素: `[...rest]`
    /// 使用 iterator 协议从当前位置收集剩余元素到一个新数组
    fn lower_array_rest_destructure(
        &mut self,
        src_val: ValueId,
        rest_pat: &swc_ast::Pat,
        skip_count: i32,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<ValueId, LoweringError> {
        // 1. 创建迭代器
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![src_val],
            },
        );

        // 2. 跳过已处理的元素
        for _ in 0..skip_count {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::IteratorNext,
                    args: vec![iter_handle],
                },
            );
        }

        // 3. 创建结果数组
        let result_arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: result_arr,
                capacity: 0,
            },
        );

        // 4. 循环收集剩余元素
        let header = self.current_function.new_block();
        let loop_body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: 检查 iterator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: not_done,
                true_block: loop_body,
                false_block: exit,
            },
        );

        // body: 获取值，push 到数组
        let elem_val = self.alloc_value();
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: Some(elem_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ArrayPush,
                args: vec![result_arr, elem_val],
            },
        );
        // 调用 iterator next 前进
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(loop_body, Terminator::Jump { target: header });

        // exit: 关闭迭代器
        self.current_function.append_instruction(
            exit,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_handle],
            },
        );

        // 5. 将结果数组赋值给 rest pattern
        self.lower_destructure_pattern(rest_pat, result_arr, exit, kind)?;

        Ok(result_arr)
    }

    /// 默认值检查: `x = default`
    /// 语义：如果 value === undefined，使用 default 表达式；否则保留原值。
    fn lower_default_value_check(
        &mut self,
        value: ValueId,
        default_expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Compare { op: StrictEq, lhs: value, rhs: Undefined }
        let undef_cid = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_cid,
            },
        );
        let is_undef = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: is_undef,
                op: CompareOp::StrictEq,
                lhs: value,
                rhs: undef_val,
            },
        );

        // Branch
        let then_block = self.current_function.new_block();
        let else_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_undef,
                true_block: then_block,
                false_block: else_block,
            },
        );

        // then_block: 求值默认表达式
        let default_val = self.lower_expr(default_expr, then_block)?;
        self.current_function.set_terminator(
            then_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // else_block: 保留原值
        self.current_function.set_terminator(
            else_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // merge_block: Phi
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: then_block,
                        value: default_val,
                    },
                    PhiSource {
                        predecessor: else_block,
                        value,
                    },
                ],
            },
        );

        Ok(result)
    }

    fn lower_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if fn_decl.function.is_async && fn_decl.function.is_generator {
            return self.lower_async_gen_fn_decl(fn_decl, flow);
        }
        if fn_decl.function.is_async {
            return self.lower_async_fn_decl(fn_decl, flow);
        }
        let name = fn_decl.ident.sym.to_string();
        self.push_function_context(&name, BasicBlockId(0));

        // 声明 $env（闭包环境对象），非闭包时传入 undefined
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        // Register $this so that this-keyword expressions resolve.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let param_ir_names =
            self.build_param_ir_names(&fn_decl.function.params, env_scope_id, this_scope_id)?;

        // Predeclare hoisted vars in the function body.
        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        // Emit parameter initialization (default values + destructuring)
        let body_entry = self.emit_param_inits(&fn_decl.function.params, &param_ir_names, entry)?;

        let body_entry = self.emit_arguments_init(body_entry)?;

        // Lower the function body.
        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        // Add implicit return if the body is still open.
        if let StmtFlow::Open(block) = inner_flow {
            self.current_function
                .set_terminator(block, Terminator::Return { value: None });
        }

        // Finalize the function IR and push it to the module.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        // 设置捕获变量列表（逃逸分析结果）
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for block in blocks {
            ir_function.push_block(block);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore the outer function context.
        self.pop_function_context();

        // 在外层函数中 emit 函数引用（闭包或直接 FunctionRef）
        let outer_block = self.ensure_open(flow)?;
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // 如果有捕获变量，使用共享 env 对象 + CreateClosure
        let callee_val = if captured.is_empty() {
            // 非闭包函数：直接使用 FunctionRef
            func_ref_val
        } else {
            // 闭包函数：获取共享 env 对象（同一外层函数中多个闭包共享）
            let env_val = self.ensure_shared_env(outer_block, &captured, fn_decl.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                outer_block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        let (scope_id, _) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let ir_name = format!("${scope_id}.{name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: callee_val,
            },
        );
        self.append_eval_var_leak_if_needed(&name, VarKind::Var, callee_val, outer_block);

        Ok(StmtFlow::Open(outer_block))
    }

    fn lower_async_gen_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        let async_gen_name = format!("{name}$asyncgen");

        self.push_function_context(&async_gen_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.is_async_generator_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let gen_scope_id = self
            .scopes
            .declare("$generator", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_generator_scope_id = gen_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_decl.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let gen_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(gen_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${gen_scope_id}.$generator"),
                value: gen_from_cont,
            },
        );

        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        for (i, _param) in fn_decl.function.params.iter().enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let param_from_cont = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(param_from_cont),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            let param_ir_name = &user_param_ir_names[2 + i];
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        let after_inits =
            self.emit_param_inits(&fn_decl.function.params, &user_param_ir_names, entry)?;

        let after_inits = self.emit_arguments_init(after_inits)?;

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            let gen_val2 = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: gen_val2,
                    name: format!("${gen_scope_id}.$generator"),
                },
            );
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                b,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::AsyncGeneratorReturn,
                    args: vec![gen_val2, undef_val],
                },
            );
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${state_scope_id}.$state"),
                },
            );
            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases: Vec<SwitchCaseTarget> = Vec::new();
            switch_cases.push(SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            });
            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }
            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);
            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_gen_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_gen_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let wrapper_user_param_ir_names = self.build_param_ir_names(
            &fn_decl.function.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;
        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);
        let wrapper_after_inits = self.emit_param_inits(
            &fn_decl.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        let wrapper_after_inits = self.emit_arguments_init(wrapper_after_inits)?;

        let func_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(async_gen_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );
        let (callee_val, env_val_opt) = if captured.is_empty() {
            (func_ref_val, None)
        } else {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: env_val,
                    name: format!("${wrapper_env_scope_id}.$env"),
                },
            );
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            (closure_val, Some(env_val))
        };

        let count_val_num = 4 + fn_decl.function.params.len();
        let count_const = self
            .module
            .add_constant(Constant::Number(count_val_num as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, undef_val, count_val],
            },
        );
        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(gen_val),
                builtin: Builtin::AsyncGeneratorStart,
                args: vec![cont_val],
            },
        );

        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot2_val, gen_val],
            },
        );

        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_for_slot = if let Some(env_val) = env_val_opt {
            env_val
        } else {
            undef_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot3_val, env_for_slot],
            },
        );

        for (i, _arg) in fn_decl.function.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let save_slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let save_slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: save_slot_val,
                    constant: save_slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, save_slot_val, arg_val],
                },
            );
        }

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(gen_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut wrapper_ir_function = Function::new(&name, BasicBlockId(0));
        wrapper_ir_function.set_has_eval(has_eval);
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);
        self.pop_function_context();

        let outer_block = self.ensure_open(flow)?;
        let wrapper_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(wrapper_fn_id));
        let wrapper_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: wrapper_ref_val,
                constant: wrapper_ref_const,
            },
        );
        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(outer_block, &captured, fn_decl.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                outer_block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![wrapper_ref_val, env_val],
                },
            );
            closure_val
        };
        let (scope_id, _) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let ir_name = format!("${scope_id}.{name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: callee_val,
            },
        );

        Ok(StmtFlow::Open(outer_block))
    }

    fn lower_async_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_decl.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        for (i, _param) in fn_decl.function.params.iter().enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let param_from_cont = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(param_from_cont),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            let param_ir_name = &user_param_ir_names[2 + i];
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        let after_inits =
            self.emit_param_inits(&fn_decl.function.params, &user_param_ir_names, entry)?;

        let after_inits = self.emit_arguments_init(after_inits)?;

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );

        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(block) = inner_flow {
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
                    name: format!("${promise_scope_id}.$promise"),
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
        }

        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${state_scope_id}.$state"),
                },
            );

            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases: Vec<SwitchCaseTarget> = Vec::new();
            let zero_case = SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            };
            switch_cases.push(zero_case);

            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }

            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);

            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        let continuation_slot_count = self.async_next_continuation_slot;

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let wrapper_user_param_ir_names = self.build_param_ir_names(
            &fn_decl.function.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;

        let _wrapper_param_ir_names = vec![
            format!("${wrapper_env_scope_id}.$env"),
            format!("${wrapper_this_scope_id}.$this"),
        ];

        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);

        let wrapper_after_inits = self.emit_param_inits(
            &fn_decl.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        let wrapper_after_inits = self.emit_arguments_init(wrapper_after_inits)?;

        let promise_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::NewPromise { dest: promise_val },
        );

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let (callee_val, env_val_opt) = if captured.is_empty() {
            (func_ref_val, None)
        } else {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: env_val,
                    name: format!("${wrapper_env_scope_id}.$env"),
                },
            );
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            (closure_val, Some(env_val))
        };

        let count_val_num = continuation_slot_count;
        let count_const = self
            .module
            .add_constant(Constant::Number(count_val_num as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, promise_val, count_val],
            },
        );

        let save_slot0_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot0_val,
                constant: save_slot0_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot0_val, promise_val],
            },
        );

        let save_slot1_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot1_val,
                constant: save_slot1_const,
            },
        );
        let env_for_slot = if let Some(ev) = env_val_opt {
            ev
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: ud_val,
                    constant: ud_const,
                },
            );
            ud_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot1_val, env_for_slot],
            },
        );

        for (i, _arg) in fn_decl.function.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let save_slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let save_slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: save_slot_val,
                    constant: save_slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, save_slot_val, arg_val],
                },
            );
        }

        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![callee_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(promise_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut wrapper_ir_function = Function::new(&name, BasicBlockId(0));
        wrapper_ir_function.set_has_eval(has_eval);
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);

        self.pop_function_context();

        let outer_block = self.ensure_open(flow)?;

        let wrapper_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(wrapper_fn_id));
        let wrapper_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: wrapper_ref_val,
                constant: wrapper_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(outer_block, &captured, fn_decl.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                outer_block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![wrapper_ref_val, env_val],
                },
            );
            closure_val
        };

        let (scope_id, _) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let ir_name = format!("${scope_id}.{name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: callee_val,
            },
        );

        Ok(StmtFlow::Open(outer_block))
    }

    /// Lower an anonymous function expression `function(...) { ... }`.
    /// Returns a ValueId for the FunctionRef constant.
    fn lower_fn_expr(
        &mut self,
        fn_expr: &swc_ast::FnExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if fn_expr.function.is_async {
            return self.lower_async_fn_expr(fn_expr, block);
        }
        let name = fn_expr.ident.as_ref().map_or_else(
            || format!("anon_{}", self.module.functions().len()),
            |ident| ident.sym.to_string(),
        );
        self.push_function_context(&name, BasicBlockId(0));

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        // Register $this so that this-keyword expressions resolve.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        // Register the function's own name (named function expression) so it is accessible within the body.
        if let Some(ref ident) = fn_expr.ident {
            let _ = self
                .scopes
                .declare(&ident.sym.to_string(), VarKind::Let, true)
                .map_err(|msg| self.error(fn_expr.span(), msg))?;
        }

        let param_ir_names =
            self.build_param_ir_names(&fn_expr.function.params, env_scope_id, this_scope_id)?;

        // Predeclare hoisted vars in body.
        if let Some(body) = &fn_expr.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let body_entry = self.emit_param_inits(&fn_expr.function.params, &param_ir_names, entry)?;

        let body_entry = self.emit_arguments_init(body_entry)?;

        // Lower body.
        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        // Implicit return undefined.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize IR function and push to module.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore outer context.
        self.pop_function_context();

        // 在外层函数中 emit 函数引用（闭包或直接 FunctionRef）
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // 如果有捕获变量，使用共享 env 对象 + CreateClosure
        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, fn_expr.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    fn lower_async_fn_expr(
        &mut self,
        fn_expr: &swc_ast::FnExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = fn_expr.ident.as_ref().map_or_else(
            || format!("anon_{}", self.module.functions().len()),
            |ident| ident.sym.to_string(),
        );
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        if let Some(ref ident) = fn_expr.ident {
            let _ = self
                .scopes
                .declare(&ident.sym.to_string(), VarKind::Let, true)
                .map_err(|msg| self.error(fn_expr.span(), msg))?;
        }

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_expr.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_expr.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        for (i, _param) in fn_expr.function.params.iter().enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let param_from_cont = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(param_from_cont),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            let param_ir_name = &user_param_ir_names[2 + i];
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        let after_inits =
            self.emit_param_inits(&fn_expr.function.params, &user_param_ir_names, entry)?;

        let after_inits = self.emit_arguments_init(after_inits)?;

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            let promise_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: promise_val,
                    name: format!("${promise_scope_id}.$promise"),
                },
            );
            self.current_function.append_instruction(
                b,
                Instruction::PromiseResolve {
                    promise: promise_val,
                    value: undef_val,
                },
            );
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${state_scope_id}.$state"),
                },
            );
            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases: Vec<SwitchCaseTarget> = Vec::new();
            switch_cases.push(SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            });
            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }
            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);
            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        let continuation_slot_count = self.async_next_continuation_slot;

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        let wrapper_user_param_ir_names = self.build_param_ir_names(
            &fn_expr.function.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;

        let _wrapper_param_ir_names = vec![
            format!("${wrapper_env_scope_id}.$env"),
            format!("${wrapper_this_scope_id}.$this"),
        ];

        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);

        let wrapper_after_inits = self.emit_param_inits(
            &fn_expr.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        let wrapper_after_inits = self.emit_arguments_init(wrapper_after_inits)?;

        let promise_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::NewPromise { dest: promise_val },
        );

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let (callee_val, env_val_opt) = if captured.is_empty() {
            (func_ref_val, None)
        } else {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: env_val,
                    name: format!("${wrapper_env_scope_id}.$env"),
                },
            );
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            (closure_val, Some(env_val))
        };

        let count_val_num = continuation_slot_count;
        let count_const = self
            .module
            .add_constant(Constant::Number(count_val_num as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, promise_val, count_val],
            },
        );

        let save_slot0_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot0_val,
                constant: save_slot0_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot0_val, promise_val],
            },
        );

        let save_slot1_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot1_val,
                constant: save_slot1_const,
            },
        );
        let env_for_slot = if let Some(ev) = env_val_opt {
            ev
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: ud_val,
                    constant: ud_const,
                },
            );
            ud_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot1_val, env_for_slot],
            },
        );

        for (i, _arg) in fn_expr.function.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let save_slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let save_slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: save_slot_val,
                    constant: save_slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, save_slot_val, arg_val],
                },
            );
        }

        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![callee_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(promise_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut wrapper_ir_function = Function::new(&name, BasicBlockId(0));
        wrapper_ir_function.set_has_eval(has_eval);
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);

        self.pop_function_context();

        let wrapper_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(wrapper_fn_id));
        let wrapper_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: wrapper_ref_val,
                constant: wrapper_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, fn_expr.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![wrapper_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    /// Lower an arrow function expression `(params) => expr` or `(params) => { ... }`.
    fn lower_arrow_expr(
        &mut self,
        arrow: &swc_ast::ArrowExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if arrow.is_async {
            return self.lower_async_arrow_expr(arrow, block);
        }
        let name = format!("arrow_{}", self.module.functions().len());
        self.push_function_context(&name, BasicBlockId(0));
        // 标记当前为箭头函数
        *self.is_arrow_fn_stack.last_mut().unwrap() = true;

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        // 箭头函数声明 $this 参数占位（WASM 调用约定需要），但内部 this 通过 env 捕获读取
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let param_ir_names =
            self.build_arrow_param_ir_names(&arrow.params, env_scope_id, this_scope_id)?;

        let entry = BasicBlockId(0);
        let mut inner_flow;

        match arrow.body.as_ref() {
            swc_ast::BlockStmtOrExpr::BlockStmt(block_stmt) => {
                // Predeclare and lower block body.
                self.predeclare_block_stmts(&block_stmt.stmts)?;
                self.emit_hoisted_var_initializers(entry);
                let body_entry =
                    self.emit_arrow_param_inits(&arrow.params, &param_ir_names, entry)?;
                inner_flow = StmtFlow::Open(body_entry);
                for stmt in &block_stmt.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
            swc_ast::BlockStmtOrExpr::Expr(expr) => {
                // Expression body: param inits, lower expr, then return it.
                self.emit_hoisted_var_initializers(entry);
                let body_entry =
                    self.emit_arrow_param_inits(&arrow.params, &param_ir_names, entry)?;
                let val = self.lower_expr(expr, body_entry)?;
                self.current_function
                    .set_terminator(body_entry, Terminator::Return { value: Some(val) });
                inner_flow = StmtFlow::Terminated;
            }
        }

        // Implicit return undefined if no explicit return.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize IR function.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore outer context.
        self.pop_function_context();

        // 在外层函数中 emit 函数引用（闭包或直接 FunctionRef）
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, arrow.span)?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    fn lower_async_arrow_expr(
        &mut self,
        arrow: &swc_ast::ArrowExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = format!("arrow_{}", self.module.functions().len());
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        *self.is_arrow_fn_stack.last_mut().unwrap() = true;

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_arrow_param_ir_names(&arrow.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        for (i, _param) in arrow.params.iter().enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let param_from_cont = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(param_from_cont),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            let param_ir_name = &user_param_ir_names[2 + i];
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        let after_inits =
            self.emit_arrow_param_inits(&arrow.params, &user_param_ir_names, entry)?;

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow;
        match arrow.body.as_ref() {
            swc_ast::BlockStmtOrExpr::BlockStmt(block_stmt) => {
                self.predeclare_block_stmts(&block_stmt.stmts)?;
                inner_flow = StmtFlow::Open(body_entry);
                for stmt in &block_stmt.stmts {
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
            swc_ast::BlockStmtOrExpr::Expr(expr) => {
                let val = self.lower_expr(expr, body_entry)?;
                let promise_val = self.alloc_value();
                self.current_function.append_instruction(
                    body_entry,
                    Instruction::LoadVar {
                        dest: promise_val,
                        name: format!("${promise_scope_id}.$promise"),
                    },
                );
                self.current_function.append_instruction(
                    body_entry,
                    Instruction::PromiseResolve {
                        promise: promise_val,
                        value: val,
                    },
                );
                self.current_function
                    .set_terminator(body_entry, Terminator::Return { value: None });
                inner_flow = StmtFlow::Terminated;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            let promise_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: promise_val,
                    name: format!("${promise_scope_id}.$promise"),
                },
            );
            self.current_function.append_instruction(
                b,
                Instruction::PromiseResolve {
                    promise: promise_val,
                    value: undef_val,
                },
            );
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${state_scope_id}.$state"),
                },
            );
            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases: Vec<SwitchCaseTarget> = Vec::new();
            switch_cases.push(SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            });
            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }
            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);
            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        let continuation_slot_count = self.async_next_continuation_slot;

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let wrapper_user_param_ir_names = self.build_arrow_param_ir_names(
            &arrow.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;

        let _wrapper_param_ir_names = vec![
            format!("${wrapper_env_scope_id}.$env"),
            format!("${wrapper_this_scope_id}.$this"),
        ];

        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);

        let wrapper_after_inits = self.emit_arrow_param_inits(
            &arrow.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        let promise_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::NewPromise { dest: promise_val },
        );

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let (callee_val, env_val_opt) = if captured.is_empty() {
            (func_ref_val, None)
        } else {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: env_val,
                    name: format!("${wrapper_env_scope_id}.$env"),
                },
            );
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            (closure_val, Some(env_val))
        };

        let count_val_num = continuation_slot_count;
        let count_const = self
            .module
            .add_constant(Constant::Number(count_val_num as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, promise_val, count_val],
            },
        );

        let save_slot0_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot0_val,
                constant: save_slot0_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot0_val, promise_val],
            },
        );

        let save_slot1_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot1_val,
                constant: save_slot1_const,
            },
        );
        let env_for_slot = if let Some(ev) = env_val_opt {
            ev
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: ud_val,
                    constant: ud_const,
                },
            );
            ud_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot1_val, env_for_slot],
            },
        );

        for (i, _pat) in arrow.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let save_slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let save_slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: save_slot_val,
                    constant: save_slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, save_slot_val, arg_val],
                },
            );
        }

        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![callee_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(promise_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut wrapper_ir_function = Function::new(&name, BasicBlockId(0));
        wrapper_ir_function.set_has_eval(has_eval);
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);

        self.pop_function_context();

        let wrapper_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(wrapper_fn_id));
        let wrapper_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: wrapper_ref_val,
                constant: wrapper_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, arrow.span)?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![wrapper_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    fn lower_class_decl(
        &mut self,
        class_decl: &swc_ast::ClassDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let class_name = class_decl.ident.sym.to_string();

        let constructor = class_decl
            .class
            .body
            .iter()
            .find_map(|member| match member {
                swc_ast::ClassMember::Constructor(c) => Some(c),
                _ => None,
            });

        let mut private_method_ids: Vec<(String, bool, FunctionId)> = Vec::new();
        for member in &class_decl.class.body {
            if let swc_ast::ClassMember::PrivateMethod(pm) = member {
                let field_name = format!("#{}", pm.key.name);
                let is_static = pm.is_static;
                let fn_name = if is_static {
                    format!("{}.static_{}", class_name, pm.key.name)
                } else {
                    format!("{}.{}", class_name, pm.key.name)
                };

                self.push_function_context(&fn_name, BasicBlockId(0));
                let env_scope_id = self
                    .scopes
                    .declare("$env", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let this_scope_id = self
                    .scopes
                    .declare("$this", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let mut param_ir_names = vec![
                    format!("${env_scope_id}.$env"),
                    format!("${this_scope_id}.$this"),
                ];
                for param in &pm.function.params {
                    if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(pm.span, msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
                if let Some(body) = &pm.function.body {
                    self.predeclare_block_stmts(&body.stmts)?;
                }
                let m_entry = BasicBlockId(0);
                self.emit_hoisted_var_initializers(m_entry);
                let m_entry = self.emit_arguments_init(m_entry)?;
                let mut m_flow = StmtFlow::Open(m_entry);
                if let Some(body) = &pm.function.body {
                    for stmt in &body.stmts {
                        if matches!(m_flow, StmtFlow::Terminated) {
                            continue;
                        }
                        m_flow = self.lower_stmt(stmt, m_flow)?;
                    }
                }
                if let StmtFlow::Open(b) = m_flow {
                    self.current_function
                        .set_terminator(b, Terminator::Return { value: None });
                }
                let m_old_fn = std::mem::replace(
                    &mut self.current_function,
                    FunctionBuilder::new("", BasicBlockId(0)),
                );
                let m_has_eval = m_old_fn.has_eval();
                let m_blocks = m_old_fn.into_blocks();
                let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                m_ir_function.set_has_eval(m_has_eval);
                m_ir_function.set_params(param_ir_names);
                let m_captured = self.captured_names_stack.last().unwrap().clone();
                m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                for b in m_blocks {
                    m_ir_function.push_block(b);
                }
                let m_function_id = self.module.push_function(m_ir_function);
                self.pop_function_context();
                private_method_ids.push((field_name, is_static, m_function_id));
            }
        }

        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        // Register $this as the first param.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_decl.span(), msg))?;

        // Register explicit constructor params.
        let mut param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        if let Some(ctor) = constructor {
            for param in &ctor.params {
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_decl.span(), msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
            }

            // Predeclare hoisted vars in constructor body.
            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        for member in &class_decl.class.body {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![this_val, key_dest, init_val],
                        },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::SetProp { object: this_val, key: key_dest, value: init_val },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if !is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
                );
                self.current_function.append_instruction(
                    field_block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::PrivateSet,
                        args: vec![this_val, key_dest, fn_dest],
                    },
                );
                field_block = self.resolve_store_block(field_block);
            }
        }

        // Lower constructor body.
        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if let Some(_ctor) = constructor {
            inner_flow = StmtFlow::Open(self.emit_arguments_init(
                match inner_flow { StmtFlow::Open(b) => b, _ => entry }
            )?);
        }
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
        }

        // Implicit return if the body is still open.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize the constructor function.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let ctor_captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&ctor_captured));
        for block in blocks {
            ir_function.push_block(block);
        }
        let ctor_function_id = self.module.push_function(ir_function);

        // Restore the outer function context.
        self.pop_function_context();

        let outer_block = self.ensure_open(flow)?;

        // Create constructor FunctionRef constant.
        let ctor_dest = self.alloc_value();
        let ctor_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(ctor_function_id));
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: ctor_dest,
                constant: ctor_ref_const,
            },
        );

        // Create prototype object.
        let proto_dest = self.alloc_value();
        // 计算非构造函数方法数量，作为原型对象的容量
        let method_count = class_decl.class.body.iter().filter(|m| {
            matches!(m, swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method))
        }).count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            outer_block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        // For each member, process according to its kind.
        let mut static_init_idx = 0u32;
        for member in &class_decl.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => {
                    match method.kind {
                        swc_ast::MethodKind::Method => {
                            let is_static = method.is_static;
                            let target = if is_static { ctor_dest } else { proto_dest };

                            let method_name = match &method.key {
                                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}", class_name, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));

                            let env_scope_id = self
                                .scopes
                                .declare("$env", VarKind::Let, true)
                                .map_err(|msg| self.error(method.span, msg))?;
                            let this_scope_id = self
                                .scopes
                                .declare("$this", VarKind::Let, true)
                                .map_err(|msg| self.error(method.span, msg))?;

                            let mut method_param_ir_names = vec![
                                format!("${env_scope_id}.$env"),
                                format!("${this_scope_id}.$this"),
                            ];
                            for param in &method.function.params {
                                if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                    let name = binding_ident.id.sym.to_string();
                                    let scope_id = self
                                        .scopes
                                        .declare(&name, VarKind::Let, true)
                                        .map_err(|msg| self.error(method.span, msg))?;
                                    method_param_ir_names.push(format!("${scope_id}.{name}"));
                                }
                            }

                            if let Some(body) = &method.function.body {
                                self.predeclare_block_stmts(&body.stmts)?;
                            }

                            let m_entry = BasicBlockId(0);
                            self.emit_hoisted_var_initializers(m_entry);
                            let m_entry = self.emit_arguments_init(m_entry)?;

                            let mut m_flow = StmtFlow::Open(m_entry);
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    if matches!(m_flow, StmtFlow::Terminated) {
                                        continue;
                                    }
                                    m_flow = self.lower_stmt(stmt, m_flow)?;
                                }
                            }

                            if let StmtFlow::Open(b) = m_flow {
                                self.current_function
                                    .set_terminator(b, Terminator::Return { value: None });
                            }

                            let m_old_fn = std::mem::replace(
                                &mut self.current_function,
                                FunctionBuilder::new("", BasicBlockId(0)),
                            );
                            let m_has_eval = m_old_fn.has_eval();
                            let m_blocks = m_old_fn.into_blocks();
                            let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                            m_ir_function.set_has_eval(m_has_eval);
                            m_ir_function.set_params(method_param_ir_names);
                            let m_captured = self.captured_names_stack.last().unwrap().clone();
                            m_ir_function
                                .set_captured_names(Self::captured_display_names(&m_captured));
                            // 设置 home_object（实例方法才有 super 访问）
                            if !is_static {
                                m_ir_function.home_object = Some(ctor_function_id);
                            }
                            for b in m_blocks {
                                m_ir_function.push_block(b);
                            }
                            let m_function_id = self.module.push_function(m_ir_function);

                            self.pop_function_context();

                            let m_dest = self.alloc_value();
                            let m_ref_const = self
                                .module
                                .add_constant(Constant::FunctionRef(m_function_id));
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: m_dest,
                                    constant: m_ref_const,
                                },
                            );

                            let m_key_const =
                                self.module.add_constant(Constant::String(method_name));
                            let m_key_dest = self.alloc_value();
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: m_key_dest,
                                    constant: m_key_const,
                                },
                            );
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::SetProp {
                                    object: target,
                                    key: m_key_dest,
                                    value: m_dest,
                                },
                            );
                        }
                        swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                            let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                                "get"
                            } else {
                                "set"
                            };
                            let is_static = method.is_static;
                            let target = if is_static { ctor_dest } else { proto_dest };

                            let method_name = match &method.key {
                                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));

                            let env_scope_id = self
                                .scopes
                                .declare("$env", VarKind::Let, true)
                                .map_err(|msg| self.error(method.span, msg))?;
                            let this_scope_id = self
                                .scopes
                                .declare("$this", VarKind::Let, true)
                                .map_err(|msg| self.error(method.span, msg))?;

                            let mut param_ir_names = vec![
                                format!("${env_scope_id}.$env"),
                                format!("${this_scope_id}.$this"),
                            ];
                            for param in &method.function.params {
                                if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                    let name = binding_ident.id.sym.to_string();
                                    let scope_id = self
                                        .scopes
                                        .declare(&name, VarKind::Let, true)
                                        .map_err(|msg| self.error(method.span, msg))?;
                                    param_ir_names.push(format!("${scope_id}.{name}"));
                                }
                            }

                            if let Some(body) = &method.function.body {
                                self.predeclare_block_stmts(&body.stmts)?;
                            }

                            let m_entry = BasicBlockId(0);
                            self.emit_hoisted_var_initializers(m_entry);
                            let m_entry = self.emit_arguments_init(m_entry)?;

                            let mut m_flow = StmtFlow::Open(m_entry);
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    if matches!(m_flow, StmtFlow::Terminated) {
                                        continue;
                                    }
                                    m_flow = self.lower_stmt(stmt, m_flow)?;
                                }
                            }

                            if let StmtFlow::Open(b) = m_flow {
                                self.current_function
                                    .set_terminator(b, Terminator::Return { value: None });
                            }

                            let m_old_fn = std::mem::replace(
                                &mut self.current_function,
                                FunctionBuilder::new("", BasicBlockId(0)),
                            );
                            let m_has_eval = m_old_fn.has_eval();
                            let m_blocks = m_old_fn.into_blocks();
                            let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                            m_ir_function.set_has_eval(m_has_eval);
                            m_ir_function.set_params(param_ir_names);
                            let m_captured = self.captured_names_stack.last().unwrap().clone();
                            m_ir_function
                                .set_captured_names(Self::captured_display_names(&m_captured));
                            if !is_static {
                                m_ir_function.home_object = Some(ctor_function_id);
                            }
                            for b in m_blocks {
                                m_ir_function.push_block(b);
                            }
                            let m_function_id = self.module.push_function(m_ir_function);
                            self.pop_function_context();

                            let fn_dest = self.alloc_value();
                            let fn_ref_const = self
                                .module
                                .add_constant(Constant::FunctionRef(m_function_id));
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: fn_dest,
                                    constant: fn_ref_const,
                                },
                            );

                            // Build descriptor and call DefineProperty
                            let desc =
                                self.build_descriptor(accessor, fn_dest, false, true, outer_block)?;
                            let m_key_const =
                                self.module.add_constant(Constant::String(method_name));
                            let m_key_dest = self.alloc_value();
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: m_key_dest,
                                    constant: m_key_const,
                                },
                            );
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::CallBuiltin {
                                    dest: None,
                                    builtin: Builtin::DefineProperty,
                                    args: vec![target, m_key_dest, desc],
                                },
                            );
                        }
                    }
                }
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    let fn_name = format!("{}.static_init_{}", class_name, static_init_idx);
                    static_init_idx += 1;

                    self.push_function_context(&fn_name, BasicBlockId(0));

                    let env_scope_id = self
                        .scopes
                        .declare("$env", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;
                    let this_scope_id = self
                        .scopes
                        .declare("$this", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;

                    let param_ir_names = vec![
                        format!("${env_scope_id}.$env"),
                        format!("${this_scope_id}.$this"),
                    ];

                    self.predeclare_block_stmts(&static_block.body.stmts)?;

                    let m_entry = BasicBlockId(0);
                    self.emit_hoisted_var_initializers(m_entry);
                    let m_entry = self.emit_arguments_init(m_entry)?;

                    let mut m_flow = StmtFlow::Open(m_entry);
                    for stmt in &static_block.body.stmts {
                        if matches!(m_flow, StmtFlow::Terminated) {
                            continue;
                        }
                        m_flow = self.lower_stmt(stmt, m_flow)?;
                    }

                    if let StmtFlow::Open(b) = m_flow {
                        self.current_function
                            .set_terminator(b, Terminator::Return { value: None });
                    }

                    let m_old_fn = std::mem::replace(
                        &mut self.current_function,
                        FunctionBuilder::new("", BasicBlockId(0)),
                    );
                    let m_has_eval = m_old_fn.has_eval();
                    let m_blocks = m_old_fn.into_blocks();
                    let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                    m_ir_function.set_has_eval(m_has_eval);
                    m_ir_function.set_params(param_ir_names);
                    let m_captured = self.captured_names_stack.last().unwrap().clone();
                    m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                    for b in m_blocks {
                        m_ir_function.push_block(b);
                    }
                    let m_function_id = self.module.push_function(m_ir_function);

                    self.pop_function_context();

                    // 创建 FunctionRef 并立即调用 Call(ctor, this=ctor)
                    let fn_dest = self.alloc_value();
                    let fn_ref_const = self
                        .module
                        .add_constant(Constant::FunctionRef(m_function_id));
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::Const {
                            dest: fn_dest,
                            constant: fn_ref_const,
                        },
                    );

                    // Call(fn, this=ctor, args=[])
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::Call {
                            dest: None,
                            callee: fn_dest,
                            this_val: ctor_dest,
                            args: vec![],
                        },
                    );
                }
                swc_ast::ClassMember::PrivateProp(prop) if prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, outer_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            outer_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![ctor_dest, key_dest, init_val],
                        },
                    );
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, outer_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            outer_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::SetProp { object: ctor_dest, key: key_dest, value: init_val },
                    );
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if *is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    outer_block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    outer_block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
                );
                self.current_function.append_instruction(
                    outer_block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::PrivateSet,
                        args: vec![ctor_dest, key_dest, fn_dest],
                    },
                );
            }
        }

        // Set Foo.prototype = proto_obj.
        let proto_key_const = self
            .module
            .add_constant(Constant::String("prototype".to_string()));
        let proto_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: proto_key_dest,
                constant: proto_key_const,
            },
        );
        self.current_function.append_instruction(
            outer_block,
            Instruction::SetProp {
                object: ctor_dest,
                key: proto_key_dest,
                value: proto_dest,
            },
        );

        // Register class name in module scope with constructor as value.
        let (scope_id, _) = self
            .scopes
            .lookup(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let ir_name = format!("${}.{}", scope_id, class_name);
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: ctor_dest,
            },
        );

        Ok(StmtFlow::Open(outer_block))
    }

    fn lower_class_expr(
        &mut self,
        class_expr: &swc_ast::ClassExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 类表达式可选名称（匿名类表达式无名称）
        let class_name = class_expr
            .ident
            .as_ref()
            .map(|id| id.sym.to_string())
            .unwrap_or_else(|| format!("anon_class_{}", self.anon_counter));
        if class_expr.ident.is_none() {
            self.anon_counter += 1;
        }

        // 查找构造函数
        let constructor = class_expr
            .class
            .body
            .iter()
            .find_map(|member| match member {
                swc_ast::ClassMember::Constructor(c) => Some(c),
                _ => None,
            });

        let mut private_method_ids: Vec<(String, bool, FunctionId)> = Vec::new();
        for member in &class_expr.class.body {
            if let swc_ast::ClassMember::PrivateMethod(pm) = member {
                let field_name = format!("#{}", pm.key.name);
                let is_static = pm.is_static;
                let fn_name = if is_static {
                    format!("{}.static_{}", class_name, pm.key.name)
                } else {
                    format!("{}.{}", class_name, pm.key.name)
                };

                self.push_function_context(&fn_name, BasicBlockId(0));
                let env_scope_id = self
                    .scopes
                    .declare("$env", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let this_scope_id = self
                    .scopes
                    .declare("$this", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let mut param_ir_names = vec![
                    format!("${env_scope_id}.$env"),
                    format!("${this_scope_id}.$this"),
                ];
                for param in &pm.function.params {
                    if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(pm.span, msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
                if let Some(body) = &pm.function.body {
                    self.predeclare_block_stmts(&body.stmts)?;
                }
                let m_entry = BasicBlockId(0);
                self.emit_hoisted_var_initializers(m_entry);
                let m_entry = self.emit_arguments_init(m_entry)?;
                let mut m_flow = StmtFlow::Open(m_entry);
                if let Some(body) = &pm.function.body {
                    for stmt in &body.stmts {
                        if matches!(m_flow, StmtFlow::Terminated) {
                            continue;
                        }
                        m_flow = self.lower_stmt(stmt, m_flow)?;
                    }
                }
                if let StmtFlow::Open(b) = m_flow {
                    self.current_function
                        .set_terminator(b, Terminator::Return { value: None });
                }
                let m_old_fn = std::mem::replace(
                    &mut self.current_function,
                    FunctionBuilder::new("", BasicBlockId(0)),
                );
                let m_has_eval = m_old_fn.has_eval();
                let m_blocks = m_old_fn.into_blocks();
                let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                m_ir_function.set_has_eval(m_has_eval);
                m_ir_function.set_params(param_ir_names);
                let m_captured = self.captured_names_stack.last().unwrap().clone();
                m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                for b in m_blocks {
                    m_ir_function.push_block(b);
                }
                let m_function_id = self.module.push_function(m_ir_function);
                self.pop_function_context();
                private_method_ids.push((field_name, is_static, m_function_id));
            }
        }

        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_expr.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_expr.span(), msg))?;

        let mut param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        if let Some(ctor) = constructor {
            for param in &ctor.params {
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_expr.span(), msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
            }

            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        for member in &class_expr.class.body {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![this_val, key_dest, init_val],
                        },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::SetProp { object: this_val, key: key_dest, value: init_val },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if !is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
                );
                self.current_function.append_instruction(
                    field_block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::PrivateSet,
                        args: vec![this_val, key_dest, fn_dest],
                    },
                );
                field_block = self.resolve_store_block(field_block);
            }
        }

        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if let Some(_ctor) = constructor {
            inner_flow = StmtFlow::Open(self.emit_arguments_init(
                match inner_flow { StmtFlow::Open(b) => b, _ => entry }
            )?);
        }
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let ctor_captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&ctor_captured));
        for blk in blocks {
            ir_function.push_block(blk);
        }
        let ctor_function_id = self.module.push_function(ir_function);

        self.pop_function_context();

        // 在当前 block 中创建构造函数 FunctionRef 常量
        let ctor_dest = self.alloc_value();
        let ctor_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(ctor_function_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: ctor_dest,
                constant: ctor_ref_const,
            },
        );

        // 创建 prototype 对象
        let proto_dest = self.alloc_value();
        // 计算非构造函数方法数量，作为原型对象的容量
        let method_count = class_expr.class.body.iter().filter(|m| {
            matches!(m, swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method))
        }).count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        // Methods (full support for all method kinds, static, and static blocks)
        let mut static_init_idx = 0u32;
        for member in &class_expr.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => match method.kind {
                    swc_ast::MethodKind::Method => {
                        let is_static = method.is_static;
                        let target = if is_static { ctor_dest } else { proto_dest };

                        let method_name = match &method.key {
                            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}", class_name, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));

                        let env_scope_id = self
                            .scopes
                            .declare("$env", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        let this_scope_id = self
                            .scopes
                            .declare("$this", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;

                        let mut method_param_ir_names = vec![
                            format!("${env_scope_id}.$env"),
                            format!("${this_scope_id}.$this"),
                        ];
                        for param in &method.function.params {
                            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                let name = binding_ident.id.sym.to_string();
                                let scope_id = self
                                    .scopes
                                    .declare(&name, VarKind::Let, true)
                                    .map_err(|msg| self.error(method.span, msg))?;
                                method_param_ir_names.push(format!("${scope_id}.{name}"));
                            }
                        }

                        if let Some(body) = &method.function.body {
                            self.predeclare_block_stmts(&body.stmts)?;
                        }

                        let m_entry = BasicBlockId(0);
                        self.emit_hoisted_var_initializers(m_entry);
                        let m_entry = self.emit_arguments_init(m_entry)?;

                        let mut m_flow = StmtFlow::Open(m_entry);
                        if let Some(body) = &method.function.body {
                            for stmt in &body.stmts {
                                m_flow = self.lower_stmt(stmt, m_flow)?;
                            }
                        }

                        if let StmtFlow::Open(b) = m_flow {
                            self.current_function
                                .set_terminator(b, Terminator::Return { value: None });
                        }

                        let m_old_fn = std::mem::replace(
                            &mut self.current_function,
                            FunctionBuilder::new("", BasicBlockId(0)),
                        );
                        let m_has_eval = m_old_fn.has_eval();
                        let m_blocks = m_old_fn.into_blocks();
                        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                        m_ir_function.set_has_eval(m_has_eval);
                        m_ir_function.set_params(method_param_ir_names);
                        let m_captured = self.captured_names_stack.last().unwrap().clone();
                        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                        if !is_static {
                            m_ir_function.home_object = Some(ctor_function_id);
                        }
                        for b in m_blocks {
                            m_ir_function.push_block(b);
                        }
                        let m_function_id = self.module.push_function(m_ir_function);

                        self.pop_function_context();

                        let m_dest = self.alloc_value();
                        let m_ref_const = self
                            .module
                            .add_constant(Constant::FunctionRef(m_function_id));
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_dest,
                                constant: m_ref_const,
                            },
                        );

                        let m_key_const = self.module.add_constant(Constant::String(method_name));
                        let m_key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_key_dest,
                                constant: m_key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: target,
                                key: m_key_dest,
                                value: m_dest,
                            },
                        );
                    }
                    swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                        let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                            "get"
                        } else {
                            "set"
                        };
                        let is_static = method.is_static;
                        let target = if is_static { ctor_dest } else { proto_dest };

                        let method_name = match &method.key {
                            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));

                        let env_scope_id = self
                            .scopes
                            .declare("$env", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        let this_scope_id = self
                            .scopes
                            .declare("$this", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;

                        let mut param_ir_names = vec![
                            format!("${env_scope_id}.$env"),
                            format!("${this_scope_id}.$this"),
                        ];
                        for param in &method.function.params {
                            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                let name = binding_ident.id.sym.to_string();
                                let scope_id = self
                                    .scopes
                                    .declare(&name, VarKind::Let, true)
                                    .map_err(|msg| self.error(method.span, msg))?;
                                param_ir_names.push(format!("${scope_id}.{name}"));
                            }
                        }

                        if let Some(body) = &method.function.body {
                            self.predeclare_block_stmts(&body.stmts)?;
                        }

                        let m_entry = BasicBlockId(0);
                        self.emit_hoisted_var_initializers(m_entry);
                        let m_entry = self.emit_arguments_init(m_entry)?;

                        let mut m_flow = StmtFlow::Open(m_entry);
                        if let Some(body) = &method.function.body {
                            for stmt in &body.stmts {
                                m_flow = self.lower_stmt(stmt, m_flow)?;
                            }
                        }

                        if let StmtFlow::Open(b) = m_flow {
                            self.current_function
                                .set_terminator(b, Terminator::Return { value: None });
                        }

                        let m_old_fn = std::mem::replace(
                            &mut self.current_function,
                            FunctionBuilder::new("", BasicBlockId(0)),
                        );
                        let m_has_eval = m_old_fn.has_eval();
                        let m_blocks = m_old_fn.into_blocks();
                        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                        m_ir_function.set_has_eval(m_has_eval);
                        m_ir_function.set_params(param_ir_names);
                        let m_captured = self.captured_names_stack.last().unwrap().clone();
                        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                        if !is_static {
                            m_ir_function.home_object = Some(ctor_function_id);
                        }
                        for b in m_blocks {
                            m_ir_function.push_block(b);
                        }
                        let m_function_id = self.module.push_function(m_ir_function);
                        self.pop_function_context();

                        let fn_dest = self.alloc_value();
                        let fn_ref_const = self
                            .module
                            .add_constant(Constant::FunctionRef(m_function_id));
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: fn_dest,
                                constant: fn_ref_const,
                            },
                        );

                        let desc = self.build_descriptor(accessor, fn_dest, false, true, block)?;
                        let m_key_const = self.module.add_constant(Constant::String(method_name));
                        let m_key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_key_dest,
                                constant: m_key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![target, m_key_dest, desc],
                            },
                        );
                    }
                },
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    let fn_name = format!("{}.static_init_{}", class_name, static_init_idx);
                    static_init_idx += 1;

                    self.push_function_context(&fn_name, BasicBlockId(0));

                    let env_scope_id = self
                        .scopes
                        .declare("$env", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;
                    let this_scope_id = self
                        .scopes
                        .declare("$this", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;

                    let param_ir_names = vec![
                        format!("${env_scope_id}.$env"),
                        format!("${this_scope_id}.$this"),
                    ];

                    self.predeclare_block_stmts(&static_block.body.stmts)?;

                    let m_entry = BasicBlockId(0);
                    self.emit_hoisted_var_initializers(m_entry);
                    let m_entry = self.emit_arguments_init(m_entry)?;

                    let mut m_flow = StmtFlow::Open(m_entry);
                    for stmt in &static_block.body.stmts {
                        m_flow = self.lower_stmt(stmt, m_flow)?;
                    }

                    if let StmtFlow::Open(b) = m_flow {
                        self.current_function
                            .set_terminator(b, Terminator::Return { value: None });
                    }

                    let m_old_fn = std::mem::replace(
                        &mut self.current_function,
                        FunctionBuilder::new("", BasicBlockId(0)),
                    );
                    let m_has_eval = m_old_fn.has_eval();
                    let m_blocks = m_old_fn.into_blocks();
                    let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                    m_ir_function.set_has_eval(m_has_eval);
                    m_ir_function.set_params(param_ir_names);
                    let m_captured = self.captured_names_stack.last().unwrap().clone();
                    m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                    for b in m_blocks {
                        m_ir_function.push_block(b);
                    }
                    let m_function_id = self.module.push_function(m_ir_function);

                    self.pop_function_context();

                    let fn_dest = self.alloc_value();
                    let fn_ref_const = self
                        .module
                        .add_constant(Constant::FunctionRef(m_function_id));
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: fn_dest,
                            constant: fn_ref_const,
                        },
                    );

                    self.current_function.append_instruction(
                        block,
                        Instruction::Call {
                            dest: None,
                            callee: fn_dest,
                            this_val: ctor_dest,
                            args: vec![],
                        },
                    );
                }
                swc_ast::ClassMember::PrivateProp(prop) if prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![ctor_dest, key_dest, init_val],
                        },
                    );
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp { object: ctor_dest, key: key_dest, value: init_val },
                    );
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if *is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
                );
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::PrivateSet,
                        args: vec![ctor_dest, key_dest, fn_dest],
                    },
                );
            }
        }

        // Set constructor.prototype = proto_obj
        let proto_key_const = self
            .module
            .add_constant(Constant::String("prototype".to_string()));
        let proto_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: proto_key_dest,
                constant: proto_key_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: ctor_dest,
                key: proto_key_dest,
                value: proto_dest,
            },
        );

        Ok(ctor_dest)
    }
    // ── TypeScript declarations ──────────────────────────────────────────

    fn lower_ts_enum(
        &mut self,
        ts_enum: &swc_ast::TsEnumDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let enum_name = ts_enum.id.sym.to_string();

        // 创建枚举对象
        let capacity = std::cmp::max(4, ts_enum.members.len() as u32);
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        // 遍历成员，生成正向和反向映射
        let mut implicit_value: f64 = 0.0;
        for member in &ts_enum.members {
            // 获取成员名（字符串）
            let member_name = match &member.id {
                swc_ast::TsEnumMemberId::Ident(ident) => ident.sym.to_string(),
                swc_ast::TsEnumMemberId::Str(s) => s.value.to_string_lossy().into_owned(),
            };

            // 计算成员值
            let member_value = if let Some(init_expr) = &member.init {
                // 有显式初始化表达式
                let val = self.lower_expr(init_expr, block)?;
                // 尝试从数值常量读取隐式递增值起点
                if let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = init_expr.as_ref() {
                    implicit_value = num.value + 1.0;
                }
                val
            } else {
                // 无初始化表达式，使用隐式递增值
                let const_id = self.module.add_constant(Constant::Number(implicit_value));
                let val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: val,
                        constant: const_id,
                    },
                );
                implicit_value += 1.0;
                val
            };

            // 正向映射：obj[memberName] = value
            let key_const = self.module.add_constant(Constant::String(member_name.clone()));
            let key_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_dest,
                    constant: key_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: obj_dest,
                    key: key_dest,
                    value: member_value,
                },
            );

            // 反向映射：obj[value] = memberName（数字值 → 成员名）
            let reverse_key_str = if let Some(init_expr) = &member.init {
                if let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = init_expr.as_ref() {
                    Some(if num.value == num.value.trunc() {
                        format!("{}", num.value as i64)
                    } else {
                        format!("{}", num.value)
                    })
                } else {
                    None
                }
            } else {
                Some(format!("{}", (implicit_value - 1.0) as i64))
            };
            if let Some(num_str) = reverse_key_str {
                self.emit_enum_reverse_mapping(block, obj_dest, &num_str, &member_name);
            }
        }

        // StoreVar: 将枚举对象赋给枚举名
        let scope_id = self
            .scopes
            .resolve_scope_id(&enum_name)
            .map_err(|msg| self.error(ts_enum.span(), msg))?;
        let ir_name = format!("${scope_id}.{enum_name}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: ir_name,
                value: obj_dest,
            },
        );
        let _ = self.scopes.mark_initialised(&enum_name);

        Ok(StmtFlow::Open(block))
    }

    fn emit_enum_reverse_mapping(
        &mut self,
        block: BasicBlockId,
        obj_dest: ValueId,
        num_str: &str,
        member_name: &str,
    ) {
        let rev_key_const = self.module.add_constant(Constant::String(num_str.to_string()));
        let rev_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: rev_key_dest,
                constant: rev_key_const,
            },
        );
        let rev_val_const = self.module.add_constant(Constant::String(member_name.to_string()));
        let rev_val_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: rev_val_dest,
                constant: rev_val_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: obj_dest,
                key: rev_key_dest,
                value: rev_val_dest,
            },
        );
    }

    fn lower_ts_module(
        &mut self,
        ts_module: &swc_ast::TsModuleDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        if ts_module.declare {
            return Ok(StmtFlow::Open(block));
        }

        let module_name = match &ts_module.id {
            swc_ast::TsModuleName::Ident(ident) => ident.sym.to_string(),
            swc_ast::TsModuleName::Str(s) => s.value.to_string_lossy().into_owned(),
        };

        let obj_dest = self.lower_ts_module_body(ts_module, block)?;

        let scope_id = self
            .scopes
            .resolve_scope_id(&module_name)
            .map_err(|msg| self.error(ts_module.span(), msg))?;
        let ir_name = format!("${scope_id}.{module_name}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: ir_name,
                value: obj_dest,
            },
        );
        let _ = self.scopes.mark_initialised(&module_name);

        Ok(StmtFlow::Open(block))
    }

    fn lower_ts_module_body(
        &mut self,
        ts_module: &swc_ast::TsModuleDecl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity: 4,
            },
        );

        if let Some(body) = &ts_module.body {
            match body {
                swc_ast::TsNamespaceBody::TsModuleBlock(module_block) => {
                    self.scopes.push_scope(ScopeKind::Block);
                    for item in &module_block.body {
                        match item {
                            swc_ast::ModuleItem::Stmt(stmt) => {
                                self.predeclare_stmt_with_mode_and_eval_strings(
                                    stmt,
                                    LexicalMode::Include,
                                    &mut std::collections::HashMap::new(),
                                )?;
                            }
                            swc_ast::ModuleItem::ModuleDecl(module_decl) => {
                                if let swc_ast::ModuleDecl::ExportDecl(export_decl) = module_decl {
                                    self.predeclare_stmt_with_mode_and_eval_strings(
                                        &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                                        LexicalMode::Include,
                                        &mut std::collections::HashMap::new(),
                                    )?;
                                }
                            }
                        }
                    }
                    for item in &module_block.body {
                        match item {
                            swc_ast::ModuleItem::Stmt(stmt) => {
                                if let swc_ast::Stmt::Decl(_decl) = stmt {
                                    self.lower_stmt(stmt, StmtFlow::Open(block))?;
                                }
                            }
                            swc_ast::ModuleItem::ModuleDecl(module_decl) => {
                                self.lower_module_decl_into_object(module_decl, obj_dest, block)?;
                            }
                        }
                    }
                    self.scopes.pop_scope();
                }
                swc_ast::TsNamespaceBody::TsNamespaceDecl(nested) => {
                    let nested_module = swc_ast::TsModuleDecl {
                        span: nested.span,
                        declare: false,
                        global: false,
                        namespace: true,
                        id: swc_ast::TsModuleName::Ident(nested.id.clone()),
                        body: Some(*nested.body.clone()),
                    };
                    let nested_obj = self.lower_ts_module_body(&nested_module, block)?;
                    let nested_name = nested.id.sym.to_string();
                    let key_const = self.module.add_constant(Constant::String(nested_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_dest,
                            key: key_dest,
                            value: nested_obj,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    fn lower_module_decl_into_object(
        &mut self,
        module_decl: &swc_ast::ModuleDecl,
        obj_dest: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        match module_decl {
            swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                let decl_name = match &export_decl.decl {
                    swc_ast::Decl::Fn(fn_decl) => Some(fn_decl.ident.sym.to_string()),
                    swc_ast::Decl::Var(var_decl) => {
                        var_decl.decls.first().and_then(|d| {
                            match &d.name {
                                swc_ast::Pat::Ident(ident) => Some(ident.id.sym.to_string()),
                                _ => None,
                            }
                        })
                    }
                    swc_ast::Decl::TsEnum(ts_enum) => Some(ts_enum.id.sym.to_string()),
                    swc_ast::Decl::TsModule(ts_module) => {
                        match &ts_module.id {
                            swc_ast::TsModuleName::Ident(ident) => Some(ident.sym.to_string()),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                self.lower_stmt(
                    &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                    StmtFlow::Open(block),
                )?;
                if let Some(name) = decl_name {
                    if let Ok(scope_id) = self.scopes.resolve_scope_id(&name) {
                        let ir_name = format!("${scope_id}.{name}");
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::LoadVar {
                                dest: val,
                                name: ir_name,
                            },
                        );
                        let key_const = self.module.add_constant(Constant::String(name));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── using 声明 (Explicit Resource Management) ────────────────────────────

    fn lower_using_decl(
        &mut self,
        using_decl: &swc_ast::UsingDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        for declarator in &using_decl.decls {
            let mut names = Vec::new();
            Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
            for name in names {
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(using_decl.span, msg))?;
                let ir_name = format!("${scope_id}.{name}");

                // 降低初始化表达式
                if let Some(init_expr) = &declarator.init {
                    let value = self.lower_expr(init_expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::StoreVar {
                            name: ir_name.clone(),
                            value,
                        },
                    );
                }

                // 标记已初始化
                let _ = self.scopes.mark_initialised(&name);

                // 记录 using 变量
                self.active_using_vars.push(ActiveUsingVar {
                    ir_name,
                    is_async: using_decl.is_await,
                });
            }
        }

        Ok(StmtFlow::Open(block))
    }

    fn emit_using_disposal(&mut self, block: BasicBlockId) -> BasicBlockId {
        if self.active_using_vars.is_empty() {
            return block;
        }

        // Clone vars to avoid borrow checker issues
        let vars = self.active_using_vars.clone();
        let mut current_block = block;
        for var in vars.iter().rev() {
            // 1. LoadVar
            let val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::LoadVar {
                    dest: val,
                    name: var.ir_name.clone(),
                },
            );

            // 2. 检查值不是 null/undefined（用条件分支跳过 dispose）
            let skip_block = self.current_function.new_block();
            let dispose_block = self.current_function.new_block();
            let merge_block = self.current_function.new_block();

            // 检查是否为 null 或 undefined
            let is_nullish = self.alloc_value();
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            // Compare with undefined first
            self.current_function.append_instruction(
                current_block,
                Instruction::Compare {
                    dest: is_nullish,
                    op: CompareOp::StrictEq,
                    lhs: val,
                    rhs: undef_val,
                },
            );
            // Branch: if is_nullish → skip, else check null
            self.current_function.set_terminator(
                current_block,
                Terminator::Branch {
                    condition: is_nullish,
                    true_block: skip_block,
                    false_block: dispose_block,
                },
            );

            // In dispose_block: get @@dispose / @@asyncDispose and call it
            let symbol_idx = if var.is_async { WK_SYMBOL_ASYNC_DISPOSE } else { WK_SYMBOL_DISPOSE };
            let symbol_const = self.module.add_constant(Constant::Number(symbol_idx as f64));
            let symbol_val = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::Const {
                    dest: symbol_val,
                    constant: symbol_const,
                },
            );
            let wk_sym = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::CallBuiltin {
                    dest: Some(wk_sym),
                    builtin: Builtin::SymbolWellKnown,
                    args: vec![symbol_val],
                },
            );

            // obj[Symbol.dispose]
            let dispose_method = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::GetProp {
                    dest: dispose_method,
                    object: val,
                    key: wk_sym,
                },
            );

            // Call dispose method with obj as this
            self.current_function.append_instruction(
                dispose_block,
                Instruction::Call {
                    dest: None,
                    callee: dispose_method,
                    this_val: val,
                    args: vec![],
                },
            );

            self.current_function.set_terminator(
                dispose_block,
                Terminator::Jump {
                    target: merge_block,
                },
            );

            // skip_block: just jump to merge
            self.current_function.set_terminator(
                skip_block,
                Terminator::Jump {
                    target: merge_block,
                },
            );

            current_block = merge_block;
        }

        current_block
    }

    // ── JSX lowering ─────────────────────────────────────────────────────────

    fn lower_jsx_element(
        &mut self,
        jsx_el: &swc_ast::JSXElement,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 降低 tag 名
        let tag_val = self.lower_jsx_element_name(&jsx_el.opening.name, block)?;

        // 降低 props
        let props_val = self.lower_jsx_attrs(&jsx_el.opening.attrs, block)?;

        // 降低 children（作为数组）
        let children_val = self.lower_jsx_children(&jsx_el.children, block)?;

        // 调用 jsx_create_element(tag, props, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    fn lower_jsx_fragment(
        &mut self,
        jsx_frag: &swc_ast::JSXFragment,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Fragment 使用字符串标记 "$JsxFragment"
        let tag_str = "$JsxFragment".to_string();
        let tag_const = self.module.add_constant(Constant::String(tag_str));
        let tag_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: tag_val,
                constant: tag_const,
            },
        );

        // Fragment 的 props 为 null
        let null_const = self.module.add_constant(Constant::Null);
        let props_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: props_val,
                constant: null_const,
            },
        );

        // 收集 children
        let children_val = self.lower_jsx_children(&jsx_frag.children, block)?;

        // 调用 jsx_create_element(tag, null, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    fn lower_jsx_element_name(
        &mut self,
        name: &swc_ast::JSXElementName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match name {
            swc_ast::JSXElementName::Ident(ident) => {
                // HTML 标签名 → 字符串常量
                let tag_str = ident.sym.to_string();
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
            swc_ast::JSXElementName::JSXMemberExpr(member_expr) => {
                // <Foo.Bar /> → 降低为成员表达式
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXElementName::JSXNamespacedName(ns_name) => {
                // <ns:tag /> → 字符串 "ns:tag"
                let tag_str = format!("{}:{}", ns_name.ns.sym, ns_name.name.sym);
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
        }
    }

    fn lower_jsx_object(
        &mut self,
        obj: &swc_ast::JSXObject,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match obj {
            swc_ast::JSXObject::JSXMemberExpr(member_expr) => {
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXObject::Ident(ident) => {
                self.lower_ident(ident, block)
            }
        }
    }

    fn lower_jsx_attrs(
        &mut self,
        attrs: &[swc_ast::JSXAttrOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if attrs.is_empty() {
            // 无属性 → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 props 对象
        let capacity = std::cmp::max(4, attrs.len() as u32);
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        for attr_or_spread in attrs {
            match attr_or_spread {
                swc_ast::JSXAttrOrSpread::JSXAttr(attr) => {
                    let attr_name = match &attr.name {
                        swc_ast::JSXAttrName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::JSXAttrName::JSXNamespacedName(ns_name) => {
                            format!("{}:{}", ns_name.ns.sym, ns_name.name.sym)
                        }
                    };

                    let attr_value = if let Some(ref value) = attr.value {
                        match &*value {
                            swc_ast::JSXAttrValue::Str(s) => {
                                let str_val = s.value.to_string_lossy().into_owned();
                                let const_id = self.module.add_constant(Constant::String(str_val));
                                let val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: val,
                                        constant: const_id,
                                    },
                                );
                                val
                            }
                            swc_ast::JSXAttrValue::JSXExprContainer(expr_container) => {
                                match &expr_container.expr {
                                    swc_ast::JSXExpr::Expr(expr) => {
                                        self.lower_expr(expr, block)?
                                    }
                                    swc_ast::JSXExpr::JSXEmptyExpr(_) => {
                                        // 空表达式 → true
                                        let true_const =
                                            self.module.add_constant(Constant::Bool(true));
                                        let val = self.alloc_value();
                                        self.current_function.append_instruction(
                                            block,
                                            Instruction::Const {
                                                dest: val,
                                                constant: true_const,
                                            },
                                        );
                                        val
                                    }
                                }
                            }
                            swc_ast::JSXAttrValue::JSXElement(el) => {
                                self.lower_jsx_element(&el, block)?
                            }
                            swc_ast::JSXAttrValue::JSXFragment(frag) => {
                                self.lower_jsx_fragment(&frag, block)?
                            }
                        }
                    } else {
                        // 无值属性（如 <input disabled />）→ true
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: val,
                                constant: true_const,
                            },
                        );
                        val
                    };

                    // SetProp(obj, attr_name, attr_value)
                    let key_const = self.module.add_constant(Constant::String(attr_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_dest,
                            key: key_dest,
                            value: attr_value,
                        },
                    );
                }
                swc_ast::JSXAttrOrSpread::SpreadElement(spread) => {
                    let source = self.lower_expr(&spread.expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    fn lower_jsx_children(
        &mut self,
        children: &[swc_ast::JSXElementChild],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if children.is_empty() {
            // 无 children → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 children 数组
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: children.len() as u32,
            },
        );

        for child in children {
            let child_val = match child {
                swc_ast::JSXElementChild::JSXText(text) => {
                    let trimmed = text.value.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let str_const = self.module.add_constant(Constant::String(trimmed.to_string()));
                    let val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val,
                            constant: str_const,
                        },
                    );
                    val
                }
                swc_ast::JSXElementChild::JSXExprContainer(expr_container) => {
                    match &expr_container.expr {
                        swc_ast::JSXExpr::Expr(expr) => self.lower_expr(expr, block)?,
                        swc_ast::JSXExpr::JSXEmptyExpr(_) => continue,
                    }
                }
                swc_ast::JSXElementChild::JSXElement(el) => {
                    self.lower_jsx_element(el, block)?
                }
                swc_ast::JSXElementChild::JSXFragment(frag) => {
                    self.lower_jsx_fragment(frag, block)?
                }
                _ => continue,
            };
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, child_val],
                },
            );
        }

        Ok(arr)
    }

    // ── Expressions ─────────────────────────────────────────────────────────

    fn lower_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match expr {
            swc_ast::Expr::Bin(bin) => self.lower_binary(bin, block),
            swc_ast::Expr::Lit(lit) => self.lower_literal(lit, block),
            swc_ast::Expr::Ident(ident) => self.lower_ident(ident, block),
            swc_ast::Expr::Assign(assign) => self.lower_assign(assign, block),
            swc_ast::Expr::Unary(unary) => self.lower_unary(unary, block),
            swc_ast::Expr::Cond(cond) => self.lower_cond(cond, block),
            swc_ast::Expr::Seq(seq) => self.lower_seq(seq, block),
            swc_ast::Expr::Paren(paren) => self.lower_expr(&paren.expr, block),
            swc_ast::Expr::Call(call) => self.lower_call_expr(call, block),
            swc_ast::Expr::Fn(fn_expr) => self.lower_fn_expr(fn_expr, block),
            swc_ast::Expr::Arrow(arrow) => self.lower_arrow_expr(arrow, block),
            swc_ast::Expr::Object(obj_expr) => self.lower_object_expr(obj_expr, block),
            swc_ast::Expr::Array(arr) => self.lower_array_expr(arr, block),
            swc_ast::Expr::Member(member) => self.lower_member_expr(member, block),
            swc_ast::Expr::This(_) => self.lower_this(block),
            swc_ast::Expr::New(new_expr) => self.lower_new_expr(new_expr, block),
            swc_ast::Expr::Class(class_expr) => self.lower_class_expr(class_expr, block),
            swc_ast::Expr::Update(update) => self.lower_update(update, block),
            swc_ast::Expr::Tpl(tpl) => self.lower_tpl(tpl, block),
            swc_ast::Expr::TaggedTpl(tagged_tpl) => self.lower_tagged_tpl(tagged_tpl, block),
            swc_ast::Expr::SuperProp(super_prop) => self.lower_super_prop(super_prop, block),
            swc_ast::Expr::Await(await_expr) => {
                if !self.is_async_fn {
                    return Err(self.error(expr.span(), "await is only valid in async functions"));
                }
                self.lower_await_expr(await_expr, block)
            }
            swc_ast::Expr::Yield(yield_expr) => self.lower_yield_expr(yield_expr, block),
            // TS type assertion expressions — 编译时类型信息，透传内层表达式
            swc_ast::Expr::TsTypeAssertion(ts_assert) => {
                self.lower_expr(&ts_assert.expr, block)
            }
            swc_ast::Expr::TsConstAssertion(assert) => {
                self.lower_expr(&assert.expr, block)
            }
            swc_ast::Expr::TsNonNull(ts_non_null) => {
                self.lower_expr(&ts_non_null.expr, block)
            }
            swc_ast::Expr::TsAs(ts_as) => {
                self.lower_expr(&ts_as.expr, block)
            }
            swc_ast::Expr::TsSatisfies(ts_satisfies) => {
                self.lower_expr(&ts_satisfies.expr, block)
            }
            swc_ast::Expr::TsInstantiation(ts_inst) => {
                self.lower_expr(&ts_inst.expr, block)
            }
            // JSX expressions
            swc_ast::Expr::JSXElement(jsx_el) => self.lower_jsx_element(jsx_el, block),
            swc_ast::Expr::JSXFragment(jsx_frag) => self.lower_jsx_fragment(jsx_frag, block),
            swc_ast::Expr::JSXEmpty(_) => {
                let null_const = self.module.add_constant(Constant::Null);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest,
                        constant: null_const,
                    },
                );
                Ok(dest)
            }
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    fn lower_tpl(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // quasis (静态文本段) + exprs (动态表达式) 交错: quasi[0] expr[0] quasi[1] expr[1] ... quasi[n]
        let mut parts = Vec::with_capacity(tpl.quasis.len() + tpl.exprs.len());
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            let cooked_str = quasi.cooked.as_ref().ok_or_else(|| {
                self.error(
                    quasi.span,
                    "template string quasi has no cooked value".to_string(),
                )
            })?;
            let cooked_s = cooked_str.to_atom_lossy().to_string();
            let const_id = self.module.add_constant(Constant::String(cooked_s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            parts.push(val);
            if i < tpl.exprs.len() {
                let expr_val = self.lower_expr(&tpl.exprs[i], block)?;
                parts.push(expr_val);
            }
        }
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::StringConcatVa { dest, parts });
        Ok(dest)
    }

    fn lower_tagged_tpl(
        &mut self,
        tagged_tpl: &swc_ast::TaggedTpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let tpl = &tagged_tpl.tpl;
        // 1. 构建 cooked quasi 数组
        let cooked_arr = self.lower_quasis_to_array(tpl, block, false)?;
        // 2. 构建 raw quasi 数组
        let raw_arr = self.lower_quasis_to_array(tpl, block, true)?;
        // 3. Object.defineProperty(cooked_arr, "raw", { value: raw_arr, ... })
        let define_prop_const = self
            .module
            .add_constant(Constant::String("raw".to_string()));
        let define_prop_key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: define_prop_key,
                constant: define_prop_const,
            },
        );
        // 描述符对象: { value: raw_arr, writable: false, enumerable: false, configurable: false }
        let desc_obj_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_obj_val,
                capacity: 4,
            },
        );
        // value
        let value_key = self
            .module
            .add_constant(Constant::String("value".to_string()));
        let value_key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: value_key_val,
                constant: value_key,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_obj_val,
                key: value_key_val,
                value: raw_arr,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::DefineProperty,
                args: vec![cooked_arr, define_prop_key, desc_obj_val],
            },
        );
        // 4. 解析 callee + this_val（复用 lower_call_expr 的逻辑）
        let (callee_val, this_val) = self.lower_tag_expr(&tagged_tpl.tag, block)?;
        // 5. 收集参数: [cooked_arr, ...exprs]
        let mut args = Vec::with_capacity(1 + tpl.exprs.len());
        args.push(cooked_arr);
        for expr in &tpl.exprs {
            let expr_val = self.lower_expr(expr, block)?;
            args.push(expr_val);
        }
        // 6. 发出 Call 指令
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: Some(dest),
                callee: callee_val,
                this_val,
                args,
            },
        );
        Ok(dest)
    }

    fn lower_quasis_to_array(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
        raw: bool,
    ) -> Result<ValueId, LoweringError> {
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: tpl.quasis.len() as u32,
            },
        );
        for quasi in &tpl.quasis {
            let s = if raw {
                quasi.raw.as_str().to_string()
            } else {
                let cooked = quasi.cooked.as_ref().ok_or_else(|| {
                    self.error(
                        quasi.span,
                        "template string quasi has no cooked value".to_string(),
                    )
                })?;
                cooked.to_atom_lossy().to_string()
            };
            let const_id = self.module.add_constant(Constant::String(s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, val],
                },
            );
        }
        Ok(arr)
    }

    /// 解析 tagged template 的 tag 表达式，返回 (callee, this_val)。
    /// 复用 lower_call_expr 的 MemberExpression 解析逻辑。
    fn lower_tag_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<(ValueId, ValueId), LoweringError> {
        match expr {
            swc_ast::Expr::Member(member_expr) => {
                let this_val = self.lower_expr(&member_expr.obj, block)?;
                let callee_val = self.lower_member_expr(member_expr, block)?;
                Ok((callee_val, this_val))
            }
            _ => {
                let undef_const = self.module.add_constant(Constant::Undefined);
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: this_val,
                        constant: undef_const,
                    },
                );
                let callee_val = self.lower_expr(expr, block)?;
                Ok((callee_val, this_val))
            }
        }
    }

    fn lower_object_expr(
        &mut self,
        obj_expr: &swc_ast::ObjectLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let obj_dest = self.alloc_value();
        // 容量取 4 和属性数量的较大值，确保对象字面量有足够的槽位
        let capacity = std::cmp::max(4, obj_expr.props.len() as u32);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        for prop in &obj_expr.props {
            match prop {
                swc_ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                    swc_ast::Prop::KeyValue(kv) => {
                        let val_dest = self.lower_expr(&kv.value, block)?;
                        self.lower_object_prop(obj_dest, &kv.key, val_dest, block)?;
                    }
                    swc_ast::Prop::Shorthand(ident) => {
                        let val_dest = self.lower_ident(ident, block)?;
                        let key_str = ident.sym.to_string();
                        let key_const = self.module.add_constant(Constant::String(key_str));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val_dest,
                            },
                        );
                    }
                    swc_ast::Prop::Getter(getter) => {
                        let key_dest = self.lower_prop_name(&getter.key, block)?;
                        let body = getter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(getter.span, "getter must have a body"))?;
                        let fn_value = self.lower_method_to_fn(&getter.key, body, None, block)?;
                        let desc = self.build_descriptor("get", fn_value, true, true, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Setter(setter) => {
                        let key_dest = self.lower_prop_name(&setter.key, block)?;
                        let body = setter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(setter.span, "setter must have a body"))?;
                        let fn_value =
                            self.lower_method_to_fn(&setter.key, body, Some(true), block)?;
                        let desc = self.build_descriptor("set", fn_value, true, true, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Method(method) => {
                        let key_dest = self.lower_prop_name(&method.key, block)?;
                        let fn_value =
                            self.lower_method_prop_to_fn(&method.key, &method.function, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: fn_value,
                            },
                        );
                    }
                    _ => {
                        return Err(
                            self.error(prop.span(), "unsupported property kind in object literal")
                        );
                    }
                },
                swc_ast::PropOrSpread::Spread(spread) => {
                    let source = self.lower_expr(&spread.expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    /// 将 PropName 转换为运行时的 key value：静态名生成 String 常量，Computed 则 lower 表达式
    fn lower_prop_name(
        &mut self,
        key: &swc_ast::PropName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match key {
            swc_ast::PropName::Ident(ident) => {
                let key_str = ident.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Str(s) => {
                let key_str = s.value.to_string_lossy().into_owned();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Computed(computed) => self.lower_expr(&computed.expr, block),
            _ => Err(self.error(key.span(), "unsupported property key kind")),
        }
    }

    /// 对对象字面量中的 KeyValue prop 设置属性，支持计算属性名
    fn lower_object_prop(
        &mut self,
        obj_dest: ValueId,
        key: &swc_ast::PropName,
        val_dest: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        let key_dest = self.lower_prop_name(key, block)?;
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: obj_dest,
                key: key_dest,
                value: val_dest,
            },
        );
        Ok(())
    }

    /// 将 getter/setter 方法体编译为内联函数，返回 FunctionRef 的 ValueId
    fn lower_method_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        body: &swc_ast::BlockStmt,
        _is_setter: Option<bool>,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        // 推入新的函数上下文（使用 push_function_context 管理作用域栈）
        self.push_function_context(&fn_name, BasicBlockId(0));

        // 声明 $env 和 $this
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let method_param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        // 预声明提升变量
        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);

        let m_entry = self.emit_arguments_init(m_entry)?;

        // 降低方法体
        let mut m_flow = StmtFlow::Open(m_entry);
        for stmt in &body.stmts {
            if matches!(m_flow, StmtFlow::Terminated) {
                continue;
            }
            m_flow = self.lower_stmt(stmt, m_flow)?;
        }

        if let StmtFlow::Open(b) = m_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize method function
        let m_old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let m_has_eval = m_old_fn.has_eval();
        let m_blocks = m_old_fn.into_blocks();
        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
        m_ir_function.set_has_eval(m_has_eval);
        m_ir_function.set_params(method_param_ir_names);
        let m_captured = self.captured_names_stack.last().unwrap().clone();
        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
        for b in m_blocks {
            m_ir_function.push_block(b);
        }
        let m_function_id = self.module.push_function(m_ir_function);

        self.pop_function_context();

        // Create FunctionRef
        let m_dest = self.alloc_value();
        let m_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(m_function_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: m_dest,
                constant: m_ref_const,
            },
        );

        Ok(m_dest)
    }

    fn lower_method_prop_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        function: &swc_ast::Function,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        self.push_function_context(&fn_name, BasicBlockId(0));

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let param_ir_names =
            self.build_param_ir_names(&function.params, env_scope_id, this_scope_id)?;

        if let Some(body) = &function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let body_entry = self.emit_param_inits(&function.params, &param_ir_names, entry)?;

        let body_entry = self.emit_arguments_init(body_entry)?;

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&fn_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        self.pop_function_context();

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, key.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    /// 构建 getter/setter descriptor 对象 { get/set: fn, enumerable, configurable }
    fn build_descriptor(
        &mut self,
        accessor_kind: &str,
        fn_value: ValueId,
        enumerable: bool,
        configurable: bool,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let desc_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_dest,
                capacity: 4,
            },
        );

        // descriptor[accessor_kind] = fn
        let key_const = self
            .module
            .add_constant(Constant::String(accessor_kind.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: key_dest,
                value: fn_value,
            },
        );

        // descriptor.enumerable
        let enum_key = self
            .module
            .add_constant(Constant::String("enumerable".to_string()));
        let enum_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_key_dest,
                constant: enum_key,
            },
        );
        let enum_val_dest = self.alloc_value();
        let enum_const = self.module.add_constant(Constant::Bool(enumerable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_val_dest,
                constant: enum_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: enum_key_dest,
                value: enum_val_dest,
            },
        );

        // descriptor.configurable
        let conf_key = self
            .module
            .add_constant(Constant::String("configurable".to_string()));
        let conf_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_key_dest,
                constant: conf_key,
            },
        );
        let conf_val_dest = self.alloc_value();
        let conf_const = self.module.add_constant(Constant::Bool(configurable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_val_dest,
                constant: conf_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: conf_key_dest,
                value: conf_val_dest,
            },
        );

        Ok(desc_dest)
    }

    fn lower_array_expr(
        &mut self,
        arr: &swc_ast::ArrayLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let elem_count = arr.elems.len();
        // 根据元素数量分配容量（最少 4 个元素槽位减少扩容）
        let capacity = std::cmp::max(4, elem_count as u32);
        let arr_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr_dest,
                capacity,
            },
        );

        // 遍历元素：对每个元素 push 到数组
        for elem in &arr.elems {
            let val = match elem {
                Some(elem) => self.lower_expr(&elem.expr, block)?,
                None => {
                    // 稀疏数组的空位 → undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let val_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val_dest,
                            constant: undef_const,
                        },
                    );
                    val_dest
                }
            };
            // 使用 CallBuiltin(ArrayPush) 添加元素（同时自动更新 length）
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr_dest, val],
                },
            );
        }

        Ok(arr_dest)
    }

    fn lower_member_expr(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 拦截 Math 常量属性访问（Math.PI, Math.E 等）
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop {
            if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref() {
                if obj_ident.sym.to_string() == "Math" && self.scopes.lookup("Math").is_err() {
                    let prop_name = prop_ident.sym.to_string();
                    let is_math_const = matches!(
                        prop_name.as_str(),
                        "E" | "LN10" | "LN2" | "LOG10E" | "LOG2E" | "PI" | "SQRT1_2" | "SQRT2"
                    );
                    if is_math_const {
                        let math_const_name = format!("$0.Math.{}", prop_name);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::LoadVar {
                                dest,
                                name: math_const_name,
                            },
                        );
                        return Ok(dest);
                    }
                }

                // 拦截 Number 常量属性访问（Number.EPSILON, Number.MAX_VALUE 等）
                if obj_ident.sym.to_string() == "Number" && self.scopes.lookup("Number").is_err() {
                    let prop_name = prop_ident.sym.to_string();
                    let is_number_const = matches!(
                        prop_name.as_str(),
                        "EPSILON" | "MAX_VALUE" | "MIN_VALUE" | "MAX_SAFE_INTEGER"
                            | "MIN_SAFE_INTEGER" | "NaN" | "NEGATIVE_INFINITY" | "POSITIVE_INFINITY"
                    );
                    if is_number_const {
                        let number_const_name = format!("$0.Number.{}", prop_name);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::LoadVar {
                                dest,
                                name: number_const_name,
                            },
                        );
                        return Ok(dest);
                    }
                }
            }
        }

        let obj_val = self.lower_expr(&member.obj, block)?;

        let key = match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let key_const = self
                    .module
                    .add_constant(Constant::String(ident.sym.to_string()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                key_dest
            }
            swc_ast::MemberProp::Computed(computed) => self.lower_expr(&computed.expr, block)?,
            swc_ast::MemberProp::PrivateName(name) => {
                let field_name = format!("#{}", name.name);
                let key_const = self.module.add_constant(Constant::String(field_name));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::PrivateGet,
                        args: vec![obj_val, key_dest],
                    },
                );
                return Ok(dest);
            }
            _ => return Err(self.error(member.span, "unsupported member property kind")),
        };

        let dest = self.alloc_value();
        match &member.prop {
            // Ident（命名属性）→ GetProp（走原型链，或读取 length 等内置属性）
            // Ident（命名属性）→ 检查是否为 Symbol 的静态属性（如 Symbol.dispose）
            swc_ast::MemberProp::Ident(ident) => {
                // 检查对象是否为 Symbol（编译时已知的 well-known symbol 访问）
                if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref() {
                    if obj_ident.sym.to_string() == "Symbol" {
                        let prop_name = ident.sym.to_string();
                        // 将 Symbol.dispose 等映射为 well-known symbol
                        let wk_index = match prop_name.as_str() {
                            "iterator" => Some(WK_SYMBOL_ITERATOR),
                            "species" => Some(WK_SYMBOL_SPECIES),
                            "toStringTag" => Some(WK_SYMBOL_TO_STRING_TAG),
                            "asyncIterator" => Some(WK_SYMBOL_ASYNC_ITERATOR),
                            "hasInstance" => Some(WK_SYMBOL_HAS_INSTANCE),
                            "toPrimitive" => Some(WK_SYMBOL_TO_PRIMITIVE),
                            "dispose" => Some(WK_SYMBOL_DISPOSE),
                            "match" => Some(WK_SYMBOL_MATCH),
                            "asyncDispose" => Some(WK_SYMBOL_ASYNC_DISPOSE),
                            _ => None,
                        };
                        if let Some(idx) = wk_index {
                            let idx_const = self.module.add_constant(Constant::Number(idx as f64));
                            let idx_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: idx_val,
                                    constant: idx_const,
                                },
                            );
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: Builtin::SymbolWellKnown,
                                    args: vec![idx_val],
                                },
                            );
                            return Ok(dest);
                        }
                    }
                }
                // 默认走 GetProp 路径
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key,
                    },
                );
            }
            // Computed（计算属性）：数字字面量用 GetElem，其他用 GetProp
            swc_ast::MemberProp::Computed(_) => {
                // 检查 computed key 是否为数字字面量 → GetElem
                let use_get_elem = matches!(
                    member.prop,
                    swc_ast::MemberProp::Computed(swc_ast::ComputedPropName { ref expr, .. })
                        if matches!(expr.as_ref(), swc_ast::Expr::Lit(swc_ast::Lit::Num(_)))
                );
                if use_get_elem {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: obj_val,
                            index: key,
                        },
                    );
                } else {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: obj_val,
                            key,
                        },
                    );
                }
            }
            _ => unreachable!(),
        }
        Ok(dest)
    }

    /// 加载当前函数的闭包环境对象（$env 参数）
    fn load_env_object(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        let name = if let Some(ref env_name) = self.async_closure_env_ir_name {
            env_name.clone()
        } else {
            "$env".to_string()
        };
        self.current_function
            .append_instruction(block, Instruction::LoadVar { dest, name });
        dest
    }

    /// 获取或创建当前外层函数的共享 env 对象，并确保所有捕获变量都已写入。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，保证可变捕获变量的修改对所有闭包可见。
    fn ensure_shared_env(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        _span: Span,
    ) -> Result<ValueId, LoweringError> {
        // 步骤 1：读取当前共享 env 状态（不持有引用的情况下）
        let existing_env_val = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(v, _)| *v);
        let existing_names = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(_, names)| names.clone())
            .unwrap_or_default();

        let env_val = match existing_env_val {
            Some(val) => val,
            None => {
                if captured
                    .iter()
                    .any(|binding| !self.binding_belongs_to_current_function(binding))
                {
                    // 子闭包继续捕获祖先绑定时，复用父 env，保持同一个绑定槽。
                    self.load_env_object(block)
                } else {
                    // 当前函数首次共享本地绑定时创建 env 对象。
                    let env_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::NewObject {
                            dest: env_val,
                            capacity: captured.len() as u32,
                        },
                    );
                    env_val
                }
            }
        };

        // 步骤 2：写入新变量到 env 对象（仅写入尚未存在的变量）
        for binding in captured {
            if existing_names.contains(binding) {
                continue;
            }

            let current_val = if self.binding_belongs_to_current_function(binding) {
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: current_val,
                        name: binding.var_ir_name(),
                    },
                );
                current_val
            } else {
                self.record_capture(binding.clone());
                let parent_env = self.load_env_object(block);
                let parent_key = self.append_env_key_const(block, binding);
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: current_val,
                        object: parent_env,
                        key: parent_key,
                    },
                );
                current_val
            };

            let key_val = self.append_env_key_const(block, binding);
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value: current_val,
                },
            );
        }

        // 步骤 3：更新共享 env 状态
        if existing_env_val.is_none() {
            let mut name_set = std::collections::HashSet::new();
            for binding in captured {
                name_set.insert(binding.clone());
            }
            *self.shared_env_stack.last_mut().unwrap() = Some((env_val, name_set));
        } else {
            // 追加新变量名到已有集合
            let shared = self.shared_env_stack.last_mut().unwrap();
            if let Some((_, names)) = shared {
                for binding in captured {
                    names.insert(binding.clone());
                }
            }
        }

        Ok(env_val)
    }

    fn lower_super_prop(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 1. GetSuperBase: 从 home_object 的 proto 读取基类原型
        let base_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::GetSuperBase { dest: base_val });

        // 2. 根据 prop 类型进行属性访问
        match &super_prop.prop {
            swc_ast::SuperProp::Ident(ident_name) => {
                let key_str = ident_name.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: base_val,
                        key: key_dest,
                    },
                );
                Ok(dest)
            }
            swc_ast::SuperProp::Computed(computed) => {
                let key_val = self.lower_expr(&computed.expr, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetElem {
                        dest,
                        object: base_val,
                        index: key_val,
                    },
                );
                Ok(dest)
            }
        }
    }

    fn lower_this(&mut self, block: BasicBlockId) -> Result<ValueId, LoweringError> {
        // 箭头函数的 this 是词法捕获的，通过 env 对象读取
        let is_arrow = self.is_arrow_fn_stack.last().copied().unwrap_or(false);
        if is_arrow {
            let binding = CapturedBinding::lexical_this();
            self.record_capture(binding.clone());
            // 通过 env 对象读取 this
            let env_val = self.load_env_object(block);
            let key_val = self.append_env_key_const(block, &binding);
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest,
                    object: env_val,
                    key: key_val,
                },
            );
            Ok(dest)
        } else {
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest,
                    name: "$this".to_string(),
                },
            );
            Ok(dest)
        }
    }

    fn init_async_continuation_slots(&mut self, param_ir_names: &[String], first_param_slot: u32) {
        self.captured_var_slots.clear();
        for (offset, name) in param_ir_names.iter().skip(2).enumerate() {
            self.captured_var_slots
                .insert(name.clone(), first_param_slot + offset as u32);
        }
        self.async_next_continuation_slot =
            first_param_slot + param_ir_names.len().saturating_sub(2) as u32;
    }
    /// 为包含 top-level await 的模块设置 async main 上下文。
    /// 在 entry block (block 0) 中 emit 从 continuation 加载状态的指令，
    /// 创建 dispatch block 和 body entry block，返回 body_entry。
    /// 调用者应使用返回的 body_entry 作为后续 emit 的起始 block。
    fn init_async_main_context(
        &mut self,
        span: swc_core::common::Span,
    ) -> Result<BasicBlockId, LoweringError> {
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        // 为 main 函数设置函数上下文栈（async_visible_binding_names 依赖此栈）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false);

        let entry = BasicBlockId(0);

        // 声明 async 内部变量
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        // 无用户参数，continuation slots 从 4 开始
        self.init_async_continuation_slots(&[], 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        self.async_main_param_ir_names = param_ir_names;

        // ── entry block: 从 continuation 加载状态 ──

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        // continuation slot 0 → $state
        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        // continuation slot 1 → $is_rejected
        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        // $this → $resume_val
        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        // continuation slot 2 → $promise
        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        // continuation slot 3 → $closure_env
        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        // 创建 dispatch block 和 body entry
        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);
        self.async_main_body_entry = Some(body_entry);

        self.current_function.set_terminator(
            entry,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        Ok(body_entry)
    }

    /// 生成 dispatch block，保存 main$async 函数，创建 wrapper main 函数。
    /// 调用前需要确保：
    /// - 模块体的最后一个 block 已正确终止（open block 需要 emit PromiseResolve + Return）
    /// - async_resume_blocks 已填充
    fn finalize_async_main(&mut self) -> Result<(), LoweringError> {
        let dispatch_block = self
            .async_dispatch_block
            .expect("async_dispatch_block not set");
        let body_entry = self
            .async_main_body_entry
            .expect("async_main_body_entry not set");

        // ── 1. 生成 dispatch block（状态机 switch）──
        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${}.$state", self.async_state_scope_id),
                },
            );

            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases = vec![SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            }];

            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }

            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);

            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        // ── 2. 提取 main$async 函数 ──
        let continuation_slot_count = self.async_next_continuation_slot;
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut async_fn = Function::new("main$async", BasicBlockId(0));
        async_fn.set_has_eval(has_eval);
        async_fn.set_params(self.async_main_param_ir_names.clone());
        for b in blocks {
            async_fn.push_block(b);
        }
        let async_fn_id = self.module.push_function(async_fn);

        // ── 3. 创建 wrapper main 函数 ──
        self.next_value = 0;
        self.next_temp = 0;

        let wrapper_entry = BasicBlockId(0);

        // NewPromise
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(wrapper_entry, Instruction::NewPromise { dest: promise_val });

        // FunctionRef for main$async
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // ContinuationCreate(func_ref, promise, slot_count)
        let count_const = self
            .module
            .add_constant(Constant::Number(continuation_slot_count as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![func_ref_val, promise_val, count_val],
            },
        );

        // ContinuationSaveVar slot 2 = promise
        let save_slot2_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot2_val,
                constant: save_slot2_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot2_val, promise_val],
            },
        );

        // ContinuationSaveVar slot 3 = undefined (no closure env)
        let save_slot3_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot3_val,
                constant: save_slot3_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot3_val, undef_val],
            },
        );

        // AsyncFunctionResume(func_ref, continuation, state=0, resume_val=undefined, is_rejected=false)
        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![func_ref_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function
            .set_terminator(wrapper_entry, Terminator::Return { value: None });

        // 提取 wrapper blocks，推入模块
        let wrapper_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let wrapper_has_eval = wrapper_fn.has_eval();
        let wrapper_blocks = wrapper_fn.into_blocks();
        let mut wrapper_ir = Function::new("main", BasicBlockId(0));
        wrapper_ir.set_has_eval(wrapper_has_eval);
        wrapper_ir.set_params(self.async_main_param_ir_names.clone());
        for b in wrapper_blocks {
            wrapper_ir.push_block(b);
        }
        self.module.push_function(wrapper_ir);

        Ok(())
    }

    fn is_async_internal_binding(name: &str) -> bool {
        matches!(
            name,
            "$env"
                | "$this"
                | "$state"
                | "$resume_val"
                | "$is_rejected"
                | "$promise"
                | "$closure_env"
                | "$generator"
        ) || name.starts_with("$tmp.")
    }

    fn async_visible_binding_names(&self) -> Vec<String> {
        let Some(&function_scope_id) = self.function_scope_id_stack.last() else {
            return Vec::new();
        };

        let mut scope_chain = Vec::new();
        let mut cursor = self.scopes.current_scope_id();
        loop {
            scope_chain.push(cursor);
            if cursor == function_scope_id {
                break;
            }
            let Some(parent) = self.scopes.arenas[cursor].parent else {
                break;
            };
            cursor = parent;
        }
        scope_chain.reverse();

        let mut seen = std::collections::HashSet::new();
        let mut bindings = Vec::new();
        for scope_id in scope_chain {
            let scope = &self.scopes.arenas[scope_id];
            let mut names: Vec<String> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if Self::is_async_internal_binding(&name) {
                    continue;
                }
                let ir_name = format!("${scope_id}.{name}");
                if seen.insert(ir_name.clone()) {
                    bindings.push(ir_name);
                }
            }
        }
        bindings
    }

    fn async_binding_slot(&mut self, ir_name: &str) -> u32 {
        if let Some(slot) = self.captured_var_slots.get(ir_name) {
            return *slot;
        }
        let slot = self.async_next_continuation_slot;
        self.async_next_continuation_slot += 1;
        self.captured_var_slots.insert(ir_name.to_string(), slot);
        slot
    }

    fn emit_save_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let continuation = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: continuation,
                name: format!("${}.$env", self.async_env_scope_id),
            },
        );

        for binding in bindings {
            let slot = self.async_binding_slot(binding);
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: value,
                    name: binding.clone(),
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![continuation, slot_val, value],
                },
            );
        }
    }

    fn emit_restore_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let continuation = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: continuation,
                name: format!("${}.$env", self.async_env_scope_id),
            },
        );

        for binding in bindings {
            let Some(&slot) = self.captured_var_slots.get(binding) else {
                continue;
            };
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(value),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![continuation, slot_val],
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: binding.clone(),
                    value,
                },
            );
        }
    }

    fn lower_await_expr(
        &mut self,
        await_expr: &swc_ast::AwaitExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr(&await_expr.arg, block)?;

        let promised = self.alloc_value();
        {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::PromiseResolveStatic,
                    args: vec![undef_val, value],
                },
            );
        }

        let next_state = self.async_state_counter;
        self.async_state_counter += 1;

        let resume_block = self.current_function.new_block();
        let reject_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();

        self.async_resume_blocks.push((next_state, resume_block));
        let saved_bindings = self.async_visible_binding_names();
        self.emit_save_async_bindings(block, &saved_bindings);

        self.current_function.append_instruction(
            block,
            Instruction::Suspend {
                promise: promised,
                state: next_state,
            },
        );

        self.current_function.set_terminator(
            block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        self.emit_restore_async_bindings(resume_block, &saved_bindings);

        let resume_val = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: resume_val,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let is_rejected = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: is_rejected,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );

        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_rejected,
                true_block: reject_block,
                false_block: continue_block,
            },
        );

        self.emit_throw_value(reject_block, resume_val)?;
        let result = self.alloc_value();
        self.current_function.append_instruction(
            continue_block,
            Instruction::LoadVar {
                dest: result,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );

        Ok(result)
    }

    fn lower_yield_expr(
        &mut self,
        yield_expr: &swc_ast::YieldExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = if let Some(arg) = &yield_expr.arg {
            self.lower_expr(arg, block)?
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

        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: gen_val,
                name: format!("${}.$generator", self.async_generator_scope_id),
            },
        );

        if self.is_async_fn {
            let next_state = self.async_state_counter;
            self.async_state_counter += 1;

            let resume_block = self.current_function.new_block();
            let reject_block = self.current_function.new_block();
            let continue_block = self.current_function.new_block();

            self.async_resume_blocks.push((next_state, resume_block));
            let saved_bindings = self.async_visible_binding_names();
            self.emit_save_async_bindings(block, &saved_bindings);

            let promised = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::AsyncGeneratorNext,
                    args: vec![gen_val, value],
                },
            );

            self.current_function.append_instruction(
                block,
                Instruction::Suspend {
                    promise: promised,
                    state: next_state,
                },
            );

            self.current_function.set_terminator(
                block,
                Terminator::Jump {
                    target: continue_block,
                },
            );

            self.emit_restore_async_bindings(resume_block, &saved_bindings);
            let resume_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: resume_val,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );
            let is_rejected = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: is_rejected,
                    name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
                },
            );

            self.current_function.set_terminator(
                resume_block,
                Terminator::Branch {
                    condition: is_rejected,
                    true_block: reject_block,
                    false_block: continue_block,
                },
            );

            let gen_for_throw = self.alloc_value();
            self.current_function.append_instruction(
                reject_block,
                Instruction::LoadVar {
                    dest: gen_for_throw,
                    name: format!("${}.$generator", self.async_generator_scope_id),
                },
            );
            self.current_function.append_instruction(
                reject_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::AsyncGeneratorThrow,
                    args: vec![gen_for_throw, resume_val],
                },
            );
            self.current_function
                .set_terminator(reject_block, Terminator::Return { value: None });

            let result = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::LoadVar {
                    dest: result,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );

            Ok(result)
        } else {
            let result = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::AsyncGeneratorNext,
                    args: vec![gen_val, value],
                },
            );
            Ok(result)
        }
    }

    fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref() {
            if ident.sym == "Promise" && self.scopes.lookup(&ident.sym).is_err() {
                return self.lower_new_promise(new_expr, block);
            }
            if ident.sym == "Proxy" && self.scopes.lookup(&ident.sym).is_err() {
                // new Proxy(target, handler) → CallBuiltin(ProxyCreate, [target, handler])
                let mut arg_vals = Vec::new();
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        let arg_val = self.lower_expr(&arg.expr, block)?;
                        arg_vals.push(arg_val);
                    }
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ProxyCreate,
                        args: arg_vals,
                    },
                );
                return Ok(dest);
            }
            // Error constructors: new Error(msg), new TypeError(msg), etc.
            if self.scopes.lookup(&ident.sym).is_err() {
                if let Some(builtin) = builtin_from_global_ident(&ident.sym) {
                    if matches!(
                        builtin,
                        Builtin::ErrorConstructor
                            | Builtin::TypeErrorConstructor
                            | Builtin::RangeErrorConstructor
                            | Builtin::SyntaxErrorConstructor
                            | Builtin::ReferenceErrorConstructor
                            | Builtin::URIErrorConstructor
                            | Builtin::EvalErrorConstructor
                            | Builtin::MapConstructor
                            | Builtin::SetConstructor
                            | Builtin::WeakMapConstructor
                            | Builtin::WeakSetConstructor
                            | Builtin::DateConstructor
                            | Builtin::ArrayBufferConstructor
                            | Builtin::DataViewConstructor
                            | Builtin::Int8ArrayConstructor
                            | Builtin::Uint8ArrayConstructor
                            | Builtin::Uint8ClampedArrayConstructor
                            | Builtin::Int16ArrayConstructor
                            | Builtin::Uint16ArrayConstructor
                            | Builtin::Int32ArrayConstructor
                            | Builtin::Uint32ArrayConstructor
                            | Builtin::Float32ArrayConstructor
                            | Builtin::Float64ArrayConstructor
                    ) {
                        let mut arg_vals = Vec::new();
                        if let Some(args) = &new_expr.args {
                            for arg in args {
                                let arg_val = self.lower_expr(&arg.expr, block)?;
                                arg_vals.push(arg_val);
                            }
                        }
                        if arg_vals.is_empty() {
                            arg_vals.push({
                                let c = self.module.add_constant(Constant::Undefined);
                                let dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const { dest, constant: c },
                                );
                                dest
                            });
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin,
                                args: arg_vals,
                            },
                        );
                        return Ok(dest);
                    }
                }
            }
        }

        let callee_val = self.lower_expr(&new_expr.callee, block)?;

        // Create new object.
        let obj_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_val,
                capacity: 4,
            },
        );

        // Get prototype from constructor via GetPrototypeFromConstructor builtin.
        // 语义等价于 ECMAScript GetPrototypeFromConstructor(F)：
        // 1. 读取 ctor.prototype（含原型链遍历）
        // 2. 若非 Object 类型（包含 Array、Function、Closure 等），回退到 Object.prototype
        let proto_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(proto_val),
                builtin: Builtin::GetPrototypeFromConstructor,
                args: vec![callee_val],
            },
        );

        // Set __proto__ on the new object directly via SetProto.
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: obj_val,
                value: proto_val,
            },
        );

        // Lower arguments.
        // 性能优化：预分配容量避免循环中多次 reallocation
        let cap = new_expr.args.as_ref().map_or(0, |a| a.len());
        let mut arg_vals = Vec::with_capacity(cap);
        if let Some(args) = &new_expr.args {
            for arg in args {
                let arg_val = self.lower_expr(&arg.expr, block)?;
                arg_vals.push(arg_val);
            }
        }

        // Call the constructor with the new object as `this`.
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: None,
                callee: callee_val,
                this_val: obj_val,
                args: arg_vals,
            },
        );

        Ok(obj_val)
    }

    fn lower_new_promise(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::NewPromise { dest: promise_val });

        if let Some(args) = &new_expr.args {
            if let Some(first_arg) = args.first() {
                let callback_val = self.lower_expr(&first_arg.expr, block)?;

                let resolve_fn = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(resolve_fn),
                        builtin: Builtin::PromiseCreateResolveFunction,
                        args: vec![promise_val],
                    },
                );

                let reject_fn = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(reject_fn),
                        builtin: Builtin::PromiseCreateRejectFunction,
                        args: vec![promise_val],
                    },
                );

                let undef_const = self.module.add_constant(Constant::Undefined);
                let undef_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_const,
                    },
                );

                self.current_function.append_instruction(
                    block,
                    Instruction::Call {
                        dest: None,
                        callee: callback_val,
                        this_val: undef_val,
                        args: vec![resolve_fn, reject_fn],
                    },
                );
            }
        }

        Ok(promise_val)
    }

    // ── Identifiers ─────────────────────────────────────────────────────────

    fn lower_host_builtin_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
        builtin: Builtin,
    ) -> Result<ValueId, LoweringError> {
        let (name, min_args) = builtin_call_signature(builtin);
        if call.args.len() < min_args {
            return Err(self.error(
                call.span(),
                format!("{name} requires at least {min_args} argument"),
            ));
        }

        let mut args = Vec::with_capacity(call.args.len().max(1));
        for arg in &call.args {
            let arg_val = self.lower_expr(&arg.expr, block)?;
            args.push(arg_val);
        }

        let dest = self.alloc_value();
        let call_block = self.resolve_store_block(block);
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin,
                args,
            },
        );
        Ok(dest)
    }

    /// 处理动态 import() 调用
    fn lower_dynamic_import_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 1. 提取 specifier 字符串
        let first_arg = call.args.first().ok_or_else(|| {
            self.error(
                call.span,
                "import() requires a module specifier; \
                 in AOT compilation mode, only string literal specifiers are supported",
            )
        })?;

        let specifier = match first_arg.expr.as_ref() {
            swc_ast::Expr::Lit(swc_ast::Lit::Str(s)) => s.value.to_string_lossy().into_owned(),
            swc_ast::Expr::Tpl(tpl) => {
                if tpl.exprs.is_empty() {
                    let mut result = String::new();
                    for quasi in &tpl.quasis {
                        result.push_str(&quasi.raw);
                    }
                    result
                } else {
                    return Err(self.error(
                        call.span,
                        "import() with template literal containing expressions is not supported; \
                         AOT compilation requires the specifier to be a static string literal",
                    ));
                }
            }
            _ => {
                return Err(self.error(
                    call.span,
                    "import() requires a string literal specifier; \
                     AOT compilation cannot resolve dynamic specifiers at compile time. \
                     Use a string literal like import('./module.js') instead",
                ));
            }
        };

        // 2. 查找目标模块 ID
        let current_module_id = self.current_module_id.ok_or_else(|| {
            self.error(
                call.span,
                "dynamic import is only supported in multi-module mode",
            )
        })?;

        let target_id = self
            .find_dynamic_import_target(current_module_id, &specifier)
            .ok_or_else(|| {
                self.error(
                    call.span,
                    format!("cannot resolve dynamic import specifier '{}'", specifier),
                )
            })?;

        // 3. 生成 CallBuiltin(DynamicImport, [module_id])
        let module_id_const = self.module.add_constant(Constant::ModuleId(target_id));
        let module_id_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: module_id_val,
                constant: module_id_const,
            },
        );
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::DynamicImport,
                args: vec![module_id_val],
            },
        );
        Ok(dest)
    }

    /// 从 specifier 映射中查找动态 import 目标的 ModuleId
    fn find_dynamic_import_target(
        &self,
        current_module_id: wjsm_ir::ModuleId,
        specifier: &str,
    ) -> Option<wjsm_ir::ModuleId> {
        self.dynamic_import_specifier_map
            .get(&(current_module_id, specifier.to_string()))
            .copied()
    }

    fn lower_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let callee_val: ValueId;
        let this_val: ValueId;

        match &call.callee {
            swc_ast::Callee::Expr(expr) => {
                if let swc_ast::Expr::Ident(ident) = expr.as_ref() {
                    if ident.sym.as_ref() == "eval" && self.scopes.lookup("eval").is_err() {
                        return self.lower_direct_eval_call(call, block);
                    }
                    if let Some(builtin) = builtin_from_global_ident(&ident.sym) {
                        if self.scopes.lookup(&ident.sym).is_err() {
                            return self.lower_host_builtin_call_expr(call, block, builtin);
                        }
                    }
                }

                // 检测 MemberExpr 被调用者 → 提取 obj 作为 this
                if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
                    // 静态宿主 API（console.*, Object.*, JSON.*）不读取对象本身。
                    if let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref() {
                        if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                            if let Some(builtin) =
                                builtin_from_static_member(&obj_ident.sym, &prop_ident.sym)
                            {
                                if self.scopes.lookup(&obj_ident.sym).is_err() {
                                    // Promise 静态方法需要传递构造器作为第一个参数（species-aware）
                                    if matches!(
                                        builtin,
                                        Builtin::PromiseResolveStatic
                                            | Builtin::PromiseRejectStatic
                                            | Builtin::PromiseAll
                                            | Builtin::PromiseRace
                                            | Builtin::PromiseAllSettled
                                            | Builtin::PromiseAny
                                            | Builtin::PromiseWithResolvers
                                    ) {
                                        let undef_const =
                                            self.module.add_constant(Constant::Undefined);
                                        let constructor_val = self.alloc_value();
                                        self.current_function.append_instruction(
                                            block,
                                            Instruction::Const {
                                                dest: constructor_val,
                                                constant: undef_const,
                                            },
                                        );
                                        let mut args = vec![constructor_val];
                                        for arg in &call.args {
                                            args.push(self.lower_expr(&arg.expr, block)?);
                                        }
                                        // 无参数时补 undefined
                                        if args.len() == 1 {
                                            let undef_val = self.alloc_value();
                                            self.current_function.append_instruction(
                                                block,
                                                Instruction::Const {
                                                    dest: undef_val,
                                                    constant: undef_const,
                                                },
                                            );
                                            args.push(undef_val);
                                        }
                                        let dest = self.alloc_value();
                                        self.current_function.append_instruction(
                                            block,
                                            Instruction::CallBuiltin {
                                                dest: Some(dest),
                                                builtin,
                                                args,
                                            },
                                        );
                                        return Ok(dest);
                                    }
                                    return self.lower_host_builtin_call_expr(call, block, builtin);
                                }
                            }
                        }
                    }

                    // String.prototype 方法调用优化（必须在 Array 之前，因为 at/slice/concat 等方法在 String 和 Array 上同名）
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                        if let Some(string_builtin) =
                            builtin_from_string_proto_method(&prop_ident.sym)
                        {
                            let _ = builtin_call_signature(string_builtin);
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: string_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }
                    }

                    // Array.prototype 方法调用优化：发出 CallBuiltin 代替 Call，
                    // 跳过运行时属性解析（原型链查找）。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                        if let Some(array_builtin) =
                            builtin_from_array_proto_method(&prop_ident.sym)
                        {
                            // obj.method() → obj 是 this
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: array_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        // Function.prototype.call/apply/bind: func.call(thisArg, ...args)
                        if let Some(func_builtin) =
                            builtin_from_function_proto_method(&prop_ident.sym)
                        {
                            let func_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![func_val];

                            if matches!(func_builtin, Builtin::FuncApply) {
                                // func.apply(thisArg, argsArray)
                                if let Some(first_arg) = call.args.first() {
                                    builtin_args.push(self.lower_expr(&first_arg.expr, block)?);
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
                                    builtin_args.push(undef_val);
                                }
                                if call.args.len() > 1 {
                                    builtin_args.push(self.lower_expr(&call.args[1].expr, block)?);
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
                                    builtin_args.push(undef_val);
                                }
                            } else {
                                // func.call(thisArg, ...restArgs) / func.bind(thisArg, ...boundArgs)
                                for arg in &call.args {
                                    builtin_args.push(self.lower_expr(&arg.expr, block)?);
                                }
                                // Ensure at least thisArg (first arg after func) exists
                                if call.args.is_empty() {
                                    let undef_const = self.module.add_constant(Constant::Undefined);
                                    let undef_val = self.alloc_value();
                                    self.current_function.append_instruction(
                                        block,
                                        Instruction::Const {
                                            dest: undef_val,
                                            constant: undef_const,
                                        },
                                    );
                                    builtin_args.push(undef_val);
                                }
                            }

                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: func_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        // Object.prototype 方法调用优化：hasOwnProperty
                        if let Some(obj_proto_builtin) =
                            builtin_from_object_proto_method(&prop_ident.sym)
                        {
                            // obj.method() → obj 是 this
                            let this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: obj_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(promise_proto_builtin) =
                            builtin_from_promise_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            if builtin_args.len() < 3
                                && matches!(promise_proto_builtin, Builtin::PromiseThen)
                            {
                                let undef_const = self.module.add_constant(Constant::Undefined);
                                let undef_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: undef_val,
                                        constant: undef_const,
                                    },
                                );
                                builtin_args.push(undef_val);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: promise_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(number_proto_builtin) =
                            builtin_from_number_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: number_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(boolean_proto_builtin) =
                            builtin_from_boolean_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: boolean_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(error_proto_builtin) =
                            builtin_from_error_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: error_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }
                    }

                    // obj.method() → obj 是 this，method 是 callee（未被拦截时）
                    this_val = self.lower_expr(&member_expr.obj, block)?;
                    callee_val = self.lower_member_expr(member_expr, block)?;
                } else {
                    // 普通调用 → this = undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: this_val,
                            constant: undef_const,
                        },
                    );
                    callee_val = self.lower_expr(expr, block)?;
                }
            }
            swc_ast::Callee::Import { .. } => {
                // 动态 import() 调用
                return self.lower_dynamic_import_call(call, block);
            }
            swc_ast::Callee::Super(_) => {
                return Err(self.error(call.span, "super call is not supported"));
            }
        }

        // 性能优化：预分配容量避免循环中多次 reallocation
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            let arg_val = self.lower_expr(&arg.expr, block)?;
            args.push(arg_val);
        }

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: Some(dest),
                callee: callee_val,
                this_val,
                args,
            },
        );
        Ok(dest)
    }

    fn lower_direct_eval_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        self.current_function.mark_has_eval();

        let code_val = if let Some(first_arg) = call.args.first() {
            self.lower_expr(&first_arg.expr, block)?
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

        let bindings: Vec<_> = self
            .scopes
            .visible_bindings()
            .into_iter()
            .filter(|(_, name, _)| !matches!(name.as_str(), "undefined" | "NaN" | "Infinity"))
            .collect();

        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env_val,
                capacity: bindings.len() as u32 + u32::from(self.strict_mode),
            },
        );

        if self.strict_mode {
            let key_const = self
                .module
                .add_constant(Constant::String("__wjsm_eval_strict".to_string()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
                },
            );
            let true_const = self.module.add_constant(Constant::Bool(true));
            let true_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: true_val,
                    constant: true_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value: true_val,
                },
            );
        }

        for (scope_id, name, _) in &bindings {
            let key_const = self.module.add_constant(Constant::String(name.clone()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
                },
            );

            let binding = CapturedBinding::new(name.clone(), *scope_id);
            let value = if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                self.load_captured_binding(block, &binding)?
            } else {
                let value = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: value,
                        name: binding.var_ir_name(),
                    },
                );
                value
            };

            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value,
                },
            );
        }

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::Eval,
                args: vec![code_val, env_val],
            },
        );

        for (scope_id, name, _) in &bindings {
            let binding = CapturedBinding::new(name.clone(), *scope_id);
            if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                continue;
            }

            let key_const = self.module.add_constant(Constant::String(name.clone()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
                },
            );

            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest: value,
                    object: env_val,
                    key: key_val,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: binding.var_ir_name(),
                    value,
                },
            );
        }

        Ok(dest)
    }

    fn eval_scope_bridge_active(&self) -> bool {
        self.eval_mode && self.eval_has_scope_bridge
    }

    fn load_eval_scope_env(&mut self, block: BasicBlockId) -> ValueId {
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env,
                name: EVAL_SCOPE_ENV_PARAM.to_string(),
            },
        );
        env
    }

    fn append_eval_env_key_const(&mut self, block: BasicBlockId, name: &str) -> ValueId {
        let key_const = self.module.add_constant(Constant::String(name.to_string()));
        let key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key,
                constant: key_const,
            },
        );
        key
    }

    fn lower_eval_env_read(&mut self, name: &str, block: BasicBlockId) -> ValueId {
        let env = self.load_eval_scope_env(block);
        let key = self.append_eval_env_key_const(block, name);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object: env,
                key,
            },
        );
        dest
    }

    fn append_eval_env_write(&mut self, name: &str, value: ValueId, block: BasicBlockId) {
        if !self.eval_scope_bridge_active() {
            return;
        }
        let env = self.load_eval_scope_env(block);
        let key = self.append_eval_env_key_const(block, name);
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: env,
                key,
                value,
            },
        );
    }

    fn append_eval_var_leak_if_needed(
        &mut self,
        name: &str,
        kind: VarKind,
        value: ValueId,
        block: BasicBlockId,
    ) {
        if self.eval_var_writes_to_scope && matches!(kind, VarKind::Var) {
            self.append_eval_env_write(name, value, block);
        }
    }

    fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();

        if let Some(alias_ir_name) = self.import_aliases.get(&name).cloned() {
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest,
                    name: alias_ir_name,
                },
            );
            return Ok(dest);
        }

        if name == "eval" && self.scopes.lookup("eval").is_err() {
            let constant = self.module.add_constant(Constant::NativeCallableEval);
            let dest = self.alloc_value();
            self.current_function
                .append_instruction(block, Instruction::Const { dest, constant });
            return Ok(dest);
        }

        let (scope_id, _kind) = match self.scopes.lookup(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return Ok(self.lower_eval_env_read(&name, block));
            }
            Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
                let name_const = self.module.add_constant(Constant::String(name));
                let name_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: name_val,
                        constant: name_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::GetBuiltinGlobal,
                        args: vec![name_val],
                    },
                );
                return Ok(dest);
            }
            Err(msg) => return Err(self.error(ident.span, msg)),
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            return self.load_captured_binding(block, &binding);
        }

        // 局部变量：直接 LoadVar
        let ir_name = format!("${scope_id}.{name}");
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: ir_name,
            },
        );
        Ok(dest)
    }

    // ── Assignments ─────────────────────────────────────────────────────────

    fn lower_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Handle member expression assignment targets (e.g. obj.prop = value).
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left {
            if let swc_ast::SimpleAssignTarget::Member(member_expr) = simple {
                let obj_val = self.lower_expr(&member_expr.obj, block)?;
                let key = match &member_expr.prop {
                    swc_ast::MemberProp::Ident(ident) => {
                        let key_const = self
                            .module
                            .add_constant(Constant::String(ident.sym.to_string()));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        key_dest
                    }
                    swc_ast::MemberProp::Computed(computed) => {
                        self.lower_expr(&computed.expr, block)?
                    }
                    swc_ast::MemberProp::PrivateName(name) => {
                        let field_name = format!("#{}", name.name);
                        let key_const = self.module.add_constant(Constant::String(field_name));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        if assign.op == swc_ast::AssignOp::Assign {
                            let value_val = self.lower_expr(assign.right.as_ref(), block)?;
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: Builtin::PrivateSet,
                                    args: vec![obj_val, key_dest, value_val],
                                },
                            );
                            return Ok(value_val);
                        }
                        let old_val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(old_val),
                                builtin: Builtin::PrivateGet,
                                args: vec![obj_val, key_dest],
                            },
                        );
                        let rhs_val = self.lower_expr(assign.right.as_ref(), block)?;
                        let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                            self.error(assign.span, "unsupported compound assignment operator")
                        })?;
                        let result = self.alloc_value();
                        match bin_op {
                            BinaryOp::Mod => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::CallBuiltin {
                                        dest: Some(result),
                                        builtin: Builtin::F64Mod,
                                        args: vec![old_val, rhs_val],
                                    },
                                );
                            }
                            BinaryOp::Exp => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::CallBuiltin {
                                        dest: Some(result),
                                        builtin: Builtin::F64Exp,
                                        args: vec![old_val, rhs_val],
                                    },
                                );
                            }
                            _ => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Binary {
                                        dest: result,
                                        op: bin_op,
                                        lhs: old_val,
                                        rhs: rhs_val,
                                    },
                                );
                            }
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::PrivateSet,
                                args: vec![obj_val, key_dest, result],
                            },
                        );
                        return Ok(result);
                    }
                    _ => {
                        return Err(self.error(
                            assign.span,
                            "unsupported member property in assignment target",
                        ));
                    }
                };

                let is_computed = matches!(&member_expr.prop, swc_ast::MemberProp::Computed(_));
                if assign.op == swc_ast::AssignOp::Assign {
                    // 简单赋值: obj.x = value 或 arr[computed] = value
                    let value_val = self.lower_expr(assign.right.as_ref(), block)?;
                    match &member_expr.prop {
                        swc_ast::MemberProp::Computed(_) => {
                            self.current_function.append_instruction(
                                block,
                                Instruction::SetElem {
                                    object: obj_val,
                                    index: key,
                                    value: value_val,
                                },
                            );
                        }
                        _ => {
                            self.current_function.append_instruction(
                                block,
                                Instruction::SetProp {
                                    object: obj_val,
                                    key,
                                    value: value_val,
                                },
                            );
                        }
                    }
                    return Ok(value_val);
                }

                // 逻辑复合赋值需要短路求值，走专用路径
                if matches!(
                    assign.op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self.lower_logical_assign_member(assign, block, obj_val, key);
                }

                // 算术/位运算复合赋值
                let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                // 用 GetElem/GetProp 读取当前值（取决于是否为 computed 成员）
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    if is_computed {
                        Instruction::GetElem {
                            dest: loaded,
                            object: obj_val,
                            index: key,
                        }
                    } else {
                        Instruction::GetProp {
                            dest: loaded,
                            object: obj_val,
                            key,
                        }
                    },
                );

                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();

                // Mod 和 Exp 需要使用 CallBuiltin
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }
                let instr = if is_computed {
                    Instruction::SetElem {
                        object: obj_val,
                        index: key,
                        value: dest,
                    }
                } else {
                    Instruction::SetProp {
                        object: obj_val,
                        key,
                        value: dest,
                    }
                };
                self.current_function.append_instruction(block, instr);

                return Ok(dest);
            }
        }

        let name = match &assign.left {
            swc_ast::AssignTarget::Simple(simple) => match simple {
                swc_ast::SimpleAssignTarget::Ident(binding_ident) => {
                    binding_ident.id.sym.to_string()
                }
                _ => {
                    return Err(self.error(
                        assign.left.span(),
                        "only simple identifier assignment targets are supported",
                    ));
                }
            },
            swc_ast::AssignTarget::Pat(pat) => {
                if assign.op != swc_ast::AssignOp::Assign {
                    return Err(self.error(
                        assign.span,
                        "compound assignment with destructuring is not supported",
                    ));
                }
                let value = self.lower_expr(assign.right.as_ref(), block)?;
                let ir_pat = swc_ast::Pat::from(pat.clone());
                self.lower_destructure_pattern(&ir_pat, value, block, VarKind::Let)?;
                return Ok(value);
            }
        };

        // 性能优化：使用 lookup_for_assign 一次遍历完成 const 检查 + TDZ 检查 + scope 解析，
        // 避免 check_mutable and lookup 各自遍历 scope chain 的冗余。
        let (scope_id, kind) = match self.scopes.lookup_for_assign(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return self.lower_assign_eval_env(assign, block, &name);
            }
            Err(msg) => return Err(self.error(assign.span, msg)),
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            return self.lower_assign_captured(assign, block, &binding);
        }

        let ir_name = format!("${scope_id}.{name}");

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: rhs,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, rhs, block);
                Ok(rhs)
            }
            op => {
                // 逻辑复合赋值需要短路求值，走专用路径
                if matches!(
                    op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self.lower_logical_assign(assign, block, ir_name);
                }

                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: loaded,
                        name: ir_name.clone(),
                    },
                );

                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();

                // Mod 和 Exp 需要使用 CallBuiltin
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }

                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: dest,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, dest, block);

                Ok(dest)
            }
        }
    }

    fn lower_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        if assign.op == swc_ast::AssignOp::Assign {
            let rhs = self.lower_expr(assign.right.as_ref(), block)?;
            self.append_eval_env_write(name, rhs, block);
            return Ok(rhs);
        }

        if matches!(
            assign.op,
            swc_ast::AssignOp::AndAssign
                | swc_ast::AssignOp::OrAssign
                | swc_ast::AssignOp::NullishAssign
        ) {
            return self.lower_logical_assign_eval_env(assign, block, name);
        }

        let bin_op = assign_op_to_binary(assign.op)
            .ok_or_else(|| self.error(assign.span, "unsupported compound assignment operator"))?;
        let loaded = self.lower_eval_env_read(name, block);
        let rhs = self.lower_expr(assign.right.as_ref(), block)?;
        let dest = self.alloc_value();
        match bin_op {
            BinaryOp::Mod => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Mod,
                        args: vec![loaded, rhs],
                    },
                );
            }
            BinaryOp::Exp => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Exp,
                        args: vec![loaded, rhs],
                    },
                );
            }
            _ => {
                self.current_function.append_instruction(
                    block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );
            }
        }
        self.append_eval_env_write(name, dest, block);
        Ok(dest)
    }

    /// 对捕获变量的赋值：通过 env 对象的 GetProp/SetProp 实现
    fn lower_assign_captured(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let env_val = if self.binding_belongs_to_current_function(binding) {
            self.shared_env_value()
                .expect("shared binding must have a materialized env")
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(block)
        };
        let key_val = self.append_env_key_const(block, binding);

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val,
                        value: rhs,
                    },
                );
                Ok(rhs)
            }
            op => {
                // 逻辑复合赋值需短路求值 → 走专用路径
                if matches!(
                    op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self
                        .lower_logical_assign_captured(assign, block, binding, env_val, key_val);
                }
                // 算术/位运算复合赋值
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                // 从 env 对象读取当前值
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: loaded,
                        object: env_val,
                        key: key_val,
                    },
                );
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }
                // 写回 env 对象
                let key_val2 = self.append_env_key_const(block, binding);
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val2,
                        value: dest,
                    },
                );
                Ok(dest)
            }
        }
    }

    /// 逻辑复合赋值到捕获变量（通过 env 对象）
    fn lower_logical_assign_captured(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
        env_val: ValueId,
        key_val: ValueId,
    ) -> Result<ValueId, LoweringError> {
        // 从 env 读取当前值
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: env_val,
                key: key_val,
            },
        );

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_block, false_block) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        let key_val2 = self.append_env_key_const(assign_end, binding);
        self.current_function.append_instruction(
            assign_end,
            Instruction::SetProp {
                object: env_val,
                key: key_val2,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );
        Ok(result)
    }

    fn lower_logical_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let env_val = self.load_eval_scope_env(block);
        let key_val = self.append_eval_env_key_const(block, name);
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: env_val,
                key: key_val,
            },
        );

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_block, false_block) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.append_eval_env_write(name, rhs, assign_end);
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );
        Ok(result)
    }

    /// Lower logical compound assignment `&&=`, `||=`, `??=` with short-circuit CFG.
    /// Decomposed into LoadVar + Branch(Phi) + StoreVar just like lower_logical.
    fn lower_logical_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        ir_name: String,
    ) -> Result<ValueId, LoweringError> {
        // 1. 加载当前值
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: loaded,
                name: ir_name.clone(),
            },
        );

        // 2. 创建 assign block 和 merge block
        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        // 3. 确定 condition 和分支目标
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_target, false_target) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        // 4. 在 assign_block 中降低右值并写回
        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.current_function.append_instruction(
            assign_end,
            Instruction::StoreVar {
                name: ir_name,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        // 5. 在 merge 处用 Phi 合并
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );

        Ok(result)
    }

    /// Lower logical compound assignment to member expression target (&&=, ||=, ??=)
    /// with short-circuit CFG, using GetProp/SetProp instead of LoadVar/StoreVar.
    fn lower_logical_assign_member(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        obj_val: ValueId,
        key: ValueId,
    ) -> Result<ValueId, LoweringError> {
        // 1. 加载当前值 (GetProp)
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: obj_val,
                key,
            },
        );

        // 2. 创建 assign block 和 merge block
        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        // 3. 确定 condition 和分支目标
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_target, false_target) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        // 4. 在 assign_block 中降低右值并写回 (SetProp)
        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.current_function.append_instruction(
            assign_end,
            Instruction::SetProp {
                object: obj_val,
                key,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        // 5. 在 merge 处用 Phi 合并
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );

        Ok(result)
    }

    // ── Binary operators ────────────────────────────────────────────────────

    fn lower_binary(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::BinaryOp::*;

        match bin.op {
            // Logical operators — short circuit, may create new blocks
            LogicalAnd | LogicalOr | NullishCoalescing => self.lower_logical(bin, block),
            // Comparison operators
            EqEq | NotEq | EqEqEq | NotEqEq | Lt | LtEq | Gt | GtEq => {
                self.lower_comparison(bin, block)
            }
            // Standard arithmetic
            Add | Sub | Mul | Div => {
                let lhs = self.lower_expr(bin.left.as_ref(), block)?;
                let rhs = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                let op = match bin.op {
                    Add => BinaryOp::Add,
                    Sub => BinaryOp::Sub,
                    Mul => BinaryOp::Mul,
                    Div => BinaryOp::Div,
                    _ => unreachable!(),
                };
                self.current_function
                    .append_instruction(block, Instruction::Binary { dest, op, lhs, rhs });
                Ok(dest)
            }
            // Mod / Exp → CallBuiltin
            Mod => {
                let lhs = self.lower_expr(bin.left.as_ref(), block)?;
                let rhs = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Mod,
                        args: vec![lhs, rhs],
                    },
                );
                Ok(dest)
            }
            Exp => {
                let lhs = self.lower_expr(bin.left.as_ref(), block)?;
                let rhs = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Exp,
                        args: vec![lhs, rhs],
                    },
                );
                Ok(dest)
            }
            // Bitwise operators — convert to i32, operate, NaN-box back
            BitOr | BitXor | BitAnd | LShift | RShift | ZeroFillRShift => {
                let lhs = self.lower_expr(bin.left.as_ref(), block)?;
                let rhs = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                let op = match bin.op {
                    BitOr => BinaryOp::BitOr,
                    BitXor => BinaryOp::BitXor,
                    BitAnd => BinaryOp::BitAnd,
                    LShift => BinaryOp::Shl,
                    RShift => BinaryOp::Shr,
                    ZeroFillRShift => BinaryOp::UShr,
                    _ => unreachable!(),
                };
                self.current_function
                    .append_instruction(block, Instruction::Binary { dest, op, lhs, rhs });
                Ok(dest)
            }
            // in 操作符：检查对象是否有属性
            In => {
                let prop = self.lower_expr(bin.left.as_ref(), block)?;
                let object = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::In,
                        args: vec![object, prop],
                    },
                );
                Ok(dest)
            }
            // instanceof 操作符：检查原型链
            InstanceOf => {
                let value = self.lower_expr(bin.left.as_ref(), block)?;
                let constructor = self.lower_expr(bin.right.as_ref(), block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::InstanceOf,
                        args: vec![value, constructor],
                    },
                );
                Ok(dest)
            }
        }
    }

    /// Lower comparison operators → Compare instruction.
    /// 注意: == 和 != 使用 abstract_eq builtin 而不是 Compare 指令
    fn lower_comparison(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
        let rhs = self.lower_expr(bin.right.as_ref(), block)?;
        let dest = self.alloc_value();

        match bin.op {
            // == 使用 abstract_eq builtin
            swc_ast::BinaryOp::EqEq => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractEq,
                        args: vec![lhs, rhs],
                    },
                );
            }
            // != 使用 abstract_eq builtin 然后 Not
            swc_ast::BinaryOp::NotEq => {
                let eq_result = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(eq_result),
                        builtin: Builtin::AbstractEq,
                        args: vec![lhs, rhs],
                    },
                );
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: eq_result,
                    },
                );
            }
            // < 使用 abstract_compare builtin
            swc_ast::BinaryOp::Lt => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare,
                        args: vec![lhs, rhs],
                    },
                );
            }
            // > 相当于 (rhs < lhs)
            swc_ast::BinaryOp::Gt => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare,
                        args: vec![rhs, lhs],
                    },
                );
            }
            // <= 相当于 NOT (rhs < lhs)
            swc_ast::BinaryOp::LtEq => {
                let cmp_result = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(cmp_result),
                        builtin: Builtin::AbstractCompare,
                        args: vec![rhs, lhs],
                    },
                );
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: cmp_result,
                    },
                );
            }
            // >= 相当于 NOT (lhs < rhs)
            swc_ast::BinaryOp::GtEq => {
                let cmp_result = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(cmp_result),
                        builtin: Builtin::AbstractCompare,
                        args: vec![lhs, rhs],
                    },
                );
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: cmp_result,
                    },
                );
            }
            // === 和 !== 仍使用 Compare 指令
            _ => {
                let op = match bin.op {
                    swc_ast::BinaryOp::EqEqEq => CompareOp::StrictEq,
                    swc_ast::BinaryOp::NotEqEq => CompareOp::StrictNotEq,
                    _ => unreachable!(),
                };
                self.current_function
                    .append_instruction(block, Instruction::Compare { dest, op, lhs, rhs });
            }
        }

        Ok(dest)
    }

    /// Lower logical operators `&&`, `||`, `??` with short-circuit CFG.
    /// The merge block receives a real Phi so expression-level control flow is explicit in IR.
    fn lower_logical(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
        let rhs_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(bin.op, swc_ast::BinaryOp::NullishCoalescing) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: lhs,
                },
            );
            is_nullish
        } else {
            lhs
        };

        let (true_block, false_block) = match bin.op {
            swc_ast::BinaryOp::LogicalAnd => (rhs_block, merge),
            swc_ast::BinaryOp::LogicalOr => (merge, rhs_block),
            swc_ast::BinaryOp::NullishCoalescing => (rhs_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(bin.right.as_ref(), rhs_block)?;
        let rhs_end = self.resolve_store_block(rhs_block);
        self.current_function
            .set_terminator(rhs_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: lhs,
                    },
                    PhiSource {
                        predecessor: rhs_end,
                        value: rhs,
                    },
                ],
            },
        );

        Ok(result)
    }

    // ── Unary operators ─────────────────────────────────────────────────────

    fn lower_unary(
        &mut self,
        unary: &swc_ast::UnaryExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::UnaryOp::*;

        match unary.op {
            Bang => {
                let value = self.lower_expr(&unary.arg, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value,
                    },
                );
                Ok(dest)
            }
            Minus => {
                let value = self.lower_expr(&unary.arg, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Neg,
                        value,
                    },
                );
                Ok(dest)
            }
            Plus => {
                let value = self.lower_expr(&unary.arg, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Pos,
                        value,
                    },
                );
                Ok(dest)
            }
            Tilde => {
                let value = self.lower_expr(&unary.arg, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::BitNot,
                        value,
                    },
                );
                Ok(dest)
            }
            Void => {
                let _ = self.lower_expr(&unary.arg, block)?;
                // void returns undefined
                let undef = self.module.add_constant(Constant::Undefined);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest,
                        constant: undef,
                    },
                );
                Ok(dest)
            }
            TypeOf => {
                let arg = self.lower_expr(&unary.arg, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::TypeOf,
                        args: vec![arg],
                    },
                );
                Ok(dest)
            }
            Delete => {
                // delete 操作符
                match unary.arg.as_ref() {
                    // delete obj.prop → DeleteProp 指令
                    swc_ast::Expr::Member(member) => {
                        let object = self.lower_expr(&member.obj, block)?;
                        let key = match &member.prop {
                            swc_ast::MemberProp::Ident(ident) => {
                                let key_str = ident.sym.to_string();
                                let key_const = self.module.add_constant(Constant::String(key_str));
                                let key_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: key_val,
                                        constant: key_const,
                                    },
                                );
                                key_val
                            }
                            swc_ast::MemberProp::Computed(computed) => {
                                self.lower_expr(&computed.expr, block)?
                            }
                            _ => {
                                return Err(self.error(
                                    member.span(),
                                    "delete only supports identifier or computed property keys",
                                ));
                            }
                        };
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::DeleteProp { dest, object, key },
                        );
                        Ok(dest)
                    }
                    // delete x 对变量总是返回 true（不能删除变量）
                    swc_ast::Expr::Ident(_) => {
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest,
                                constant: true_const,
                            },
                        );
                        Ok(dest)
                    }
                    _ => Err(self.error(
                        unary.span(),
                        "delete only supports member expressions or identifiers",
                    )),
                }
            }
        }
    }

    // ── Update expression (++x, x++, --x, x--) ─────────────────────────────

    fn lower_update(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::UpdateOp;

        // ── Step 1: 确定存储目标类型并加载当前值 ──
        enum Target {
            Var(String),
            Member { obj: ValueId, key: ValueId },
        }

        let target = match update.arg.as_ref() {
            swc_ast::Expr::Ident(ident) => {
                let name = ident.sym.to_string();
                // 性能优化：使用 lookup_for_assign 一次遍历完成 const 检查 + TDZ 检查 + scope 解析
                let (scope_id, _kind) = self
                    .scopes
                    .lookup_for_assign(&name)
                    .map_err(|msg| self.error(update.span(), msg))?;
                Target::Var(format!("${scope_id}.{name}"))
            }
            swc_ast::Expr::Member(member) => {
                let obj = self.lower_expr(&member.obj, block)?;
                let key = match &member.prop {
                    swc_ast::MemberProp::Ident(ident) => {
                        let key_const = self
                            .module
                            .add_constant(Constant::String(ident.sym.to_string()));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        key_dest
                    }
                    swc_ast::MemberProp::Computed(computed) => {
                        self.lower_expr(&computed.expr, block)?
                    }
                    _ => {
                        return Err(self.error(
                            update.span(),
                            "unsupported member property in update expression target",
                        ));
                    }
                };
                Target::Member { obj, key }
            }
            _ => {
                return Err(self.error(
                    update.span(),
                    "update expression only supports identifier or member expression operands",
                ));
            }
        };

        // 1. 读取当前值
        let old_val = self.alloc_value();
        match &target {
            Target::Var(ir_name) => {
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: old_val,
                        name: ir_name.clone(),
                    },
                );
            }
            Target::Member { obj, key } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: old_val,
                        object: *obj,
                        key: *key,
                    },
                );
            }
        }

        // 2. 转换为 Number (ToNumber)
        let num_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Unary {
                dest: num_val,
                op: UnaryOp::Pos,
                value: old_val,
            },
        );

        // 3. 常量 1.0
        let one = self.module.add_constant(Constant::Number(1.0));
        let one_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: one_val,
                constant: one,
            },
        );

        // 4. 执行加法或减法
        let new_val = self.alloc_value();
        let op = match update.op {
            UpdateOp::PlusPlus => BinaryOp::Add,
            UpdateOp::MinusMinus => BinaryOp::Sub,
        };
        self.current_function.append_instruction(
            block,
            Instruction::Binary {
                dest: new_val,
                op,
                lhs: num_val,
                rhs: one_val,
            },
        );

        // 5. 写回 (StoreVar or SetProp)
        match target {
            Target::Var(ir_name) => {
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: new_val,
                    },
                );
            }
            Target::Member { obj, key } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: obj,
                        key,
                        value: new_val,
                    },
                );
            }
        }

        Ok(if update.prefix { new_val } else { num_val })
    }

    fn lower_cond(
        &mut self,
        cond: &swc_ast::CondExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 评估条件表达式
        let test = self.lower_expr(&cond.test, block)?;

        let cons_block = self.current_function.new_block();
        let alt_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: test,
                true_block: cons_block,
                false_block: alt_block,
            },
        );

        let cons_val = self.lower_expr(&cond.cons, cons_block)?;
        let cons_end = self.resolve_store_block(cons_block);
        self.current_function
            .set_terminator(cons_end, Terminator::Jump { target: merge });

        let alt_val = self.lower_expr(&cond.alt, alt_block)?;
        let alt_end = self.resolve_store_block(alt_block);
        self.current_function
            .set_terminator(alt_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: cons_end,
                        value: cons_val,
                    },
                    PhiSource {
                        predecessor: alt_end,
                        value: alt_val,
                    },
                ],
            },
        );

        Ok(result)
    }
    // ── Comma expression ────────────────────────────────────────────────────

    fn lower_seq(
        &mut self,
        seq: &swc_ast::SeqExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut last = self.alloc_value();
        for expr in &seq.exprs {
            last = self.lower_expr(expr, block)?;
        }
        Ok(last)
    }

    // ── Literals ────────────────────────────────────────────────────────────

    fn lower_literal(
        &mut self,
        lit: &swc_ast::Lit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let constant = match lit {
            swc_ast::Lit::Num(num) => Constant::Number(num.value),
            swc_ast::Lit::Str(string) => {
                Constant::String(string.value.to_string_lossy().into_owned())
            }
            swc_ast::Lit::Bool(b) => Constant::Bool(b.value),
            swc_ast::Lit::BigInt(b) => Constant::BigInt(b.value.to_str_radix(10)),
            swc_ast::Lit::Regex(regex) => Constant::RegExp {
                pattern: regex.exp.to_string(),
                flags: regex.flags.to_string(),
            },
            swc_ast::Lit::Null(_) => Constant::Null,
            _ => {
                return Err(self.error(
                    lit.span(),
                    format!("unsupported literal kind `{}`", literal_kind(lit)),
                ));
            }
        };

        let constant = self.module.add_constant(constant);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        Ok(dest)
    }

    // ── Helper: load bool constant ──────────────────────────────────────────

    fn load_bool_constant(&mut self, val: bool, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(val));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    // ── Flow helper ─────────────────────────────────────────────────────────

    fn ensure_open(&self, flow: StmtFlow) -> Result<BasicBlockId, LoweringError> {
        match flow {
            StmtFlow::Open(block) => Ok(block),
            StmtFlow::Terminated => Err(LoweringError::Diagnostic(Diagnostic::new(
                0,
                0,
                "cannot lower statement after a terminated path",
            ))),
        }
    }

    // ── Pre-scan / hoisting ─────────────────────────────────────────────────

    /// 递归遍历 Pat 树，收集所有 Pat::Ident 绑定的名称。
    /// 用于预扫描阶段注册解构模式中的变量绑定。
    fn extract_pat_bindings(pats: &[swc_ast::Pat], result: &mut Vec<String>) {
        for pat in pats {
            match pat {
                swc_ast::Pat::Ident(binding) => {
                    result.push(binding.id.sym.to_string());
                }
                swc_ast::Pat::Array(array_pat) => {
                    Self::extract_pat_bindings(
                        &array_pat
                            .elems
                            .iter()
                            .flatten()
                            .cloned()
                            .collect::<Vec<_>>(),
                        result,
                    );
                }
                swc_ast::Pat::Object(object_pat) => {
                    for prop in &object_pat.props {
                        match prop {
                            swc_ast::ObjectPatProp::KeyValue(kv) => {
                                Self::extract_pat_bindings(&[*kv.value.clone()], result);
                            }
                            swc_ast::ObjectPatProp::Assign(assign) => {
                                result.push(assign.key.id.sym.to_string());
                            }
                            swc_ast::ObjectPatProp::Rest(rest) => {
                                Self::extract_pat_bindings(&[*rest.arg.clone()], result);
                            }
                        }
                    }
                }
                swc_ast::Pat::Rest(rest) => {
                    Self::extract_pat_bindings(&[*rest.arg.clone()], result);
                }
                swc_ast::Pat::Assign(assign) => {
                    Self::extract_pat_bindings(&[*assign.left.clone()], result);
                }
                swc_ast::Pat::Expr(_) | swc_ast::Pat::Invalid(_) => {}
            }
        }
    }

    fn predeclare_stmts(&mut self, stmts: &[swc_ast::ModuleItem]) -> Result<(), LoweringError> {
        let mut eval_string_bindings = std::collections::HashMap::new();
        for item in stmts {
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    self.predeclare_stmt_with_mode_and_eval_strings(
                        stmt,
                        LexicalMode::Include,
                        &mut eval_string_bindings,
                    )?;
                }
                swc_ast::ModuleItem::ModuleDecl(swc_ast::ModuleDecl::ExportDecl(export_decl)) => {
                    // export const/let/var/function/class 需要预声明，确保 TDZ 正确
                    self.predeclare_stmt_with_mode_and_eval_strings(
                        &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                        LexicalMode::Include,
                        &mut eval_string_bindings,
                    )?;
                }
                // 其他 ModuleDecl（import、re-export 等）不需要预声明
                _ => {}
            }
        }
        Ok(())
    }

    fn predeclare_block_stmts(&mut self, stmts: &[swc_ast::Stmt]) -> Result<(), LoweringError> {
        let mut eval_string_bindings = std::collections::HashMap::new();
        for stmt in stmts {
            self.predeclare_stmt_with_mode_and_eval_strings(
                stmt,
                LexicalMode::Include,
                &mut eval_string_bindings,
            )?;
        }
        Ok(())
    }

    fn predeclare_stmt_with_mode_and_eval_strings(
        &mut self,
        stmt: &swc_ast::Stmt,
        mode: LexicalMode,
        eval_string_bindings: &mut std::collections::HashMap<String, String>,
    ) -> Result<(), LoweringError> {
        match stmt {
            swc_ast::Stmt::Decl(decl) => match decl {
                swc_ast::Decl::Var(var_decl) => {
                    let kind = match var_decl.kind {
                        swc_ast::VarDeclKind::Var => VarKind::Var,
                        swc_ast::VarDeclKind::Let => VarKind::Let,
                        swc_ast::VarDeclKind::Const => VarKind::Const,
                    };
                    for declarator in &var_decl.decls {
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
                        for name in names {
                            if !matches!(kind, VarKind::Var) && matches!(mode, LexicalMode::Exclude)
                            {
                                continue;
                            }

                            let declared = matches!(kind, VarKind::Var);
                            let scope_id = self
                                .scopes
                                .declare(&name, kind, declared)
                                .map_err(|msg| self.error(var_decl.span, msg))?;
                            if matches!(kind, VarKind::Var) {
                                self.record_hoisted_var(scope_id, name);
                            }
                        }
                        if let swc_ast::Pat::Ident(binding) = &declarator.name
                            && let Some(code) = literal_string_expr(&declarator.init)
                        {
                            eval_string_bindings.insert(binding.id.sym.to_string(), code);
                        }
                    }
                }
                swc_ast::Decl::Fn(fn_decl) => {
                    let name = fn_decl.ident.sym.to_string();
                    let _scope_id = self
                        .scopes
                        .declare(&name, VarKind::Var, true)
                        .map_err(|msg| self.error(fn_decl.span(), msg))?;
                }
                swc_ast::Decl::Class(class_decl) => {
                    let name = class_decl.ident.sym.to_string();
                    let _scope_id = self
                        .scopes
                        .declare(&name, VarKind::Var, true)
                        .map_err(|msg| self.error(class_decl.span(), msg))?;
                }
                swc_ast::Decl::TsEnum(ts_enum) => {
                    let name = ts_enum.id.sym.to_string();
                    let _scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, false)
                        .map_err(|msg| self.error(ts_enum.span(), msg))?;
                }
                swc_ast::Decl::TsModule(ts_module) => {
                    if let swc_ast::TsModuleName::Ident(ident) = &ts_module.id {
                        let name = ident.sym.to_string();
                        let _scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, false)
                            .map_err(|msg| self.error(ts_module.span(), msg))?;
                    }
                }
                swc_ast::Decl::Using(using_decl) => {
                    for declarator in &using_decl.decls {
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
                        for name in names {
                            let _scope_id = self
                                .scopes
                                .declare(&name, VarKind::Const, false)
                                .map_err(|msg| self.error(using_decl.span, msg))?;
                        }
                    }
                }
                _ => {}
            },
            swc_ast::Stmt::Block(block_stmt) => {
                for stmt in &block_stmt.stmts {
                    self.predeclare_stmt_with_mode_and_eval_strings(
                        stmt,
                        LexicalMode::Exclude,
                        eval_string_bindings,
                    )?;
                }
            }
            swc_ast::Stmt::For(for_stmt) => {
                // For `for (let x ...)`, the init variable is in a separate scope
                if let Some(init) = &for_stmt.init {
                    match init {
                        swc_ast::VarDeclOrExpr::VarDecl(var_decl) => {
                            self.predeclare_var_decl(var_decl)?;
                        }
                        _ => {}
                    }
                }
                // Recursively scan for nested declarations in the body
                self.predeclare_stmt_with_mode_and_eval_strings(
                    &for_stmt.body,
                    LexicalMode::Exclude,
                    eval_string_bindings,
                )?;
            }
            swc_ast::Stmt::ForIn(for_in) => {
                // Pre-declare the loop variable if it's a var declaration
                match &for_in.left {
                    swc_ast::ForHead::VarDecl(var_decl) => {
                        self.predeclare_var_decl(var_decl)?;
                    }
                    _ => {}
                }
                self.predeclare_stmt_with_mode_and_eval_strings(
                    &for_in.body,
                    LexicalMode::Exclude,
                    eval_string_bindings,
                )?;
            }
            swc_ast::Stmt::ForOf(for_of) => {
                match &for_of.left {
                    swc_ast::ForHead::VarDecl(var_decl) => {
                        self.predeclare_var_decl(var_decl)?;
                    }
                    _ => {}
                }
                self.predeclare_stmt_with_mode_and_eval_strings(
                    &for_of.body,
                    LexicalMode::Exclude,
                    eval_string_bindings,
                )?;
            }
            swc_ast::Stmt::Labeled(labeled) => {
                self.predeclare_stmt_with_mode_and_eval_strings(
                    &labeled.body,
                    mode,
                    eval_string_bindings,
                )?;
            }
            swc_ast::Stmt::Expr(expr_stmt) => {
                if !self.strict_mode
                    && let Some(code) =
                        direct_eval_predeclare_code(&expr_stmt.expr, eval_string_bindings)
                    && !eval_code_has_use_strict_directive(&code)
                {
                    for name in eval_literal_binding_names(&code) {
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Var, true)
                            .map_err(|msg| self.error(expr_stmt.span, msg))?;
                        self.record_hoisted_var(scope_id, name);
                    }
                }
            }

            _ => {}
        }
        Ok(())
    }

    fn predeclare_var_decl(&mut self, var_decl: &swc_ast::VarDecl) -> Result<(), LoweringError> {
        let kind = match var_decl.kind {
            swc_ast::VarDeclKind::Var => VarKind::Var,
            swc_ast::VarDeclKind::Let => VarKind::Let,
            swc_ast::VarDeclKind::Const => VarKind::Const,
        };
        for declarator in &var_decl.decls {
            let mut names = Vec::new();
            Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
            for name in names {
                let declared = matches!(kind, VarKind::Var);
                let scope_id = self
                    .scopes
                    .declare(&name, kind, declared)
                    .map_err(|msg| self.error(var_decl.span, msg))?;
                if matches!(kind, VarKind::Var) {
                    self.record_hoisted_var(scope_id, name);
                }
            }
        }
        Ok(())
    }

    fn record_hoisted_var(&mut self, scope_id: usize, name: String) {
        if !self.hoisted_vars_set.insert((scope_id, name.clone())) {
            return;
        }
        self.hoisted_vars.push(HoistedVar { scope_id, name });
    }

    fn emit_hoisted_var_initializers(&mut self, block: BasicBlockId) {
        if self.hoisted_vars.is_empty() {
            return;
        }

        let undef = self.module.add_constant(Constant::Undefined);
        let value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: value,
                constant: undef,
            },
        );

        for var in &self.hoisted_vars {
            let name = format!("${}.{}", var.scope_id, var.name);
            self.current_function
                .append_instruction(block, Instruction::StoreVar { name, value });
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn alloc_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn alloc_temp_name(&mut self) -> String {
        let name = format!("$tmp.{}", self.next_temp);
        self.next_temp += 1;
        name
    }

    /// Check if a block has been terminated by lower_expr (e.g. ternary/short-circuit).
    /// If so, find the merge block where subsequent instructions should go.
    /// Check if a block has been terminated by lower_expr (e.g. ternary/short-circuit).
    /// If so, find the merge block where subsequent instructions should go.
    fn resolve_store_block(&self, block: BasicBlockId) -> BasicBlockId {
        let Some(b) = self.current_function.block(block) else {
            return block;
        };

        if let Terminator::Jump { target } = b.terminator() {
            return *target;
        }

        let Terminator::Branch {
            true_block,
            false_block,
            ..
        } = b.terminator()
        else {
            return block;
        };

        let jump_target = |id: BasicBlockId| -> Option<BasicBlockId> {
            self.current_function
                .block(id)
                .and_then(|candidate| match candidate.terminator() {
                    Terminator::Jump { target } => Some(*target),
                    _ => None,
                })
        };

        match (jump_target(*true_block), jump_target(*false_block)) {
            (Some(left), Some(right)) if left == right => left,
            (Some(target), _) => target,
            (_, Some(target)) => target,
            _ => {
                let true_has_phi =
                    self.current_function
                        .block(*true_block)
                        .map_or(false, |candidate| {
                            candidate
                                .instructions()
                                .iter()
                                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
                        });
                if true_has_phi {
                    return *true_block;
                }

                let false_has_phi =
                    self.current_function
                        .block(*false_block)
                        .map_or(false, |candidate| {
                            candidate
                                .instructions()
                                .iter()
                                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
                        });
                if false_has_phi {
                    return *false_block;
                }

                block
            }
        }
    }
    fn resolve_open_after_expr(
        &self,
        _original_block: BasicBlockId,
        store_block: BasicBlockId,
    ) -> BasicBlockId {
        store_block
    }
    fn lower_pending_finalizers(&mut self, block: BasicBlockId) -> Result<StmtFlow, LoweringError> {
        if self.active_finalizers.is_empty() {
            return Ok(StmtFlow::Open(block));
        }

        // abrupt completion 会按从内到外的顺序执行 finally。
        // 降低某个 finally 时，只把“更外层”的 finalizer 保留在 active 栈里，
        // 这样 finally 内部的 return/throw/break/continue 能继续展开剩余外层 finally，
        // 而不是因为当前批量展开把 active_finalizers 清空而跳过它们。
        let saved = self.active_finalizers.clone();
        let mut pending = saved.clone();
        let mut flow = StmtFlow::Open(block);

        while let Some(finalizer) = pending.pop() {
            self.active_finalizers = pending.clone();
            flow = self.lower_block_body(&finalizer, flow)?;
            if matches!(flow, StmtFlow::Terminated) {
                break;
            }
        }

        self.active_finalizers = saved;
        Ok(flow)
    }

    fn error(&self, span: Span, message: impl Into<String>) -> LoweringError {
        LoweringError::Diagnostic(Diagnostic::new(span.lo.0, span.hi.0, message))
    }
}

// ── Error types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoweringError {
    #[error("{0}")]
    Diagnostic(Diagnostic),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: u32,
    pub end: u32,
    pub message: String,
}

impl Diagnostic {
    fn new(start: u32, end: u32, message: impl Into<String>) -> Self {
        Self {
            start,
            end: if end > start { end } else { start + 1 },
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "semantic lowering error [{}..{}]: {}",
            self.start, self.end, self.message
        )
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

const BUILTIN_GLOBALS: &[&str] = &[
    "Array",
    "Object",
    "Function",
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
    "SuppressedError",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Date",
    "Promise",
    "ArrayBuffer",
    "SharedArrayBuffer",
    "DataView",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "Float16Array",
    "BigInt64Array",
    "BigUint64Array",
    "Proxy",
    "Math",
    "JSON",
    "Reflect",
    "globalThis",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "eval",
    "Atomics",
    "FinalizationRegistry",
    "WeakRef",
    "Temporal",
    "Intl",
    "Iterator",
    "AsyncIterator",
    "$262",
];

fn is_builtin_global(name: &str) -> bool {
    BUILTIN_GLOBALS.contains(&name)
}

fn builtin_from_global_ident(name: &str) -> Option<Builtin> {
    match name {
        "setTimeout" => Some(Builtin::SetTimeout),
        "clearTimeout" => Some(Builtin::ClearTimeout),
        "setInterval" => Some(Builtin::SetInterval),
        "clearInterval" => Some(Builtin::ClearInterval),
        "fetch" => Some(Builtin::Fetch),
        "eval" => Some(Builtin::Eval),
        "Symbol" => Some(Builtin::SymbolCreate),
        "queueMicrotask" => Some(Builtin::QueueMicrotask),
        "Proxy" => Some(Builtin::ProxyCreate),
        "Number" => Some(Builtin::NumberConstructor),
        "Boolean" => Some(Builtin::BooleanConstructor),
        "Error" => Some(Builtin::ErrorConstructor),
        "TypeError" => Some(Builtin::TypeErrorConstructor),
        "RangeError" => Some(Builtin::RangeErrorConstructor),
        "SyntaxError" => Some(Builtin::SyntaxErrorConstructor),
        "ReferenceError" => Some(Builtin::ReferenceErrorConstructor),
        "URIError" => Some(Builtin::URIErrorConstructor),
        "EvalError" => Some(Builtin::EvalErrorConstructor),
        "Map" => Some(Builtin::MapConstructor),
        "Set" => Some(Builtin::SetConstructor),
        "WeakMap" => Some(Builtin::WeakMapConstructor),
        "WeakSet" => Some(Builtin::WeakSetConstructor),
        "Date" => Some(Builtin::DateConstructor),
        "ArrayBuffer" => Some(Builtin::ArrayBufferConstructor),
        "DataView" => Some(Builtin::DataViewConstructor),
        "Int8Array" => Some(Builtin::Int8ArrayConstructor),
        "Uint8Array" => Some(Builtin::Uint8ArrayConstructor),
        "Uint8ClampedArray" => Some(Builtin::Uint8ClampedArrayConstructor),
        "Int16Array" => Some(Builtin::Int16ArrayConstructor),
        "Uint16Array" => Some(Builtin::Uint16ArrayConstructor),
        "Int32Array" => Some(Builtin::Int32ArrayConstructor),
        "Uint32Array" => Some(Builtin::Uint32ArrayConstructor),
        "Float32Array" => Some(Builtin::Float32ArrayConstructor),
        "Float64Array" => Some(Builtin::Float64ArrayConstructor),
        _ => None,
    }
}

fn builtin_from_static_member(object: &str, property: &str) -> Option<Builtin> {
    match object {
        "console" => match property {
            "log" => Some(Builtin::ConsoleLog),
            "error" => Some(Builtin::ConsoleError),
            "warn" => Some(Builtin::ConsoleWarn),
            "info" => Some(Builtin::ConsoleInfo),
            "debug" => Some(Builtin::ConsoleDebug),
            "trace" => Some(Builtin::ConsoleTrace),
            _ => None,
        },
        "Array" => match property {
            "isArray" => Some(Builtin::ArrayIsArray),
            _ => None,
        },
        "Object" => match property {
            "defineProperty" => Some(Builtin::DefineProperty),
            "getOwnPropertyDescriptor" => Some(Builtin::GetOwnPropDesc),
            "keys" => Some(Builtin::ObjectKeys),
            "values" => Some(Builtin::ObjectValues),
            "entries" => Some(Builtin::ObjectEntries),
            "assign" => Some(Builtin::ObjectAssign),
            "create" => Some(Builtin::ObjectCreate),
            "getPrototypeOf" => Some(Builtin::ObjectGetPrototypeOf),
            "setPrototypeOf" => Some(Builtin::ObjectSetPrototypeOf),
            "getOwnPropertyNames" => Some(Builtin::ObjectGetOwnPropertyNames),
            "is" => Some(Builtin::ObjectIs),
            _ => None,
        },
        "JSON" => match property {
            "stringify" => Some(Builtin::JsonStringify),
            "parse" => Some(Builtin::JsonParse),
            _ => None,
        },
        "Symbol" => match property {
            "for" => Some(Builtin::SymbolFor),
            "keyFor" => Some(Builtin::SymbolKeyFor),
            _ => None,
        },
        "Promise" => match property {
            "resolve" => Some(Builtin::PromiseResolveStatic),
            "reject" => Some(Builtin::PromiseRejectStatic),
            "all" => Some(Builtin::PromiseAll),
            "race" => Some(Builtin::PromiseRace),
            "allSettled" => Some(Builtin::PromiseAllSettled),
            "any" => Some(Builtin::PromiseAny),
            "withResolvers" => Some(Builtin::PromiseWithResolvers),
            _ => None,
        },
        "String" => match property {
            "fromCharCode" => Some(Builtin::StringFromCharCode),
            "fromCodePoint" => Some(Builtin::StringFromCodePoint),
            _ => None,
        },
        "Proxy" => match property {
            "revocable" => Some(Builtin::ProxyRevocable),
            _ => None,
        },
        "Reflect" => match property {
            "get" => Some(Builtin::ReflectGet),
            "set" => Some(Builtin::ReflectSet),
            "has" => Some(Builtin::ReflectHas),
            "deleteProperty" => Some(Builtin::ReflectDeleteProperty),
            "apply" => Some(Builtin::ReflectApply),
            "construct" => Some(Builtin::ReflectConstruct),
            "getPrototypeOf" => Some(Builtin::ReflectGetPrototypeOf),
            "setPrototypeOf" => Some(Builtin::ReflectSetPrototypeOf),
            "isExtensible" => Some(Builtin::ReflectIsExtensible),
            "preventExtensions" => Some(Builtin::ReflectPreventExtensions),
            "getOwnPropertyDescriptor" => Some(Builtin::ReflectGetOwnPropertyDescriptor),
            "defineProperty" => Some(Builtin::ReflectDefineProperty),
            "ownKeys" => Some(Builtin::ReflectOwnKeys),
            _ => None,
        },
        "Math" => match property {
            "abs" => Some(Builtin::MathAbs),
            "acos" => Some(Builtin::MathAcos),
            "acosh" => Some(Builtin::MathAcosh),
            "asin" => Some(Builtin::MathAsin),
            "asinh" => Some(Builtin::MathAsinh),
            "atan" => Some(Builtin::MathAtan),
            "atanh" => Some(Builtin::MathAtanh),
            "atan2" => Some(Builtin::MathAtan2),
            "cbrt" => Some(Builtin::MathCbrt),
            "ceil" => Some(Builtin::MathCeil),
            "clz32" => Some(Builtin::MathClz32),
            "cos" => Some(Builtin::MathCos),
            "cosh" => Some(Builtin::MathCosh),
            "exp" => Some(Builtin::MathExp),
            "expm1" => Some(Builtin::MathExpm1),
            "floor" => Some(Builtin::MathFloor),
            "fround" => Some(Builtin::MathFround),
            "hypot" => Some(Builtin::MathHypot),
            "imul" => Some(Builtin::MathImul),
            "log" => Some(Builtin::MathLog),
            "log1p" => Some(Builtin::MathLog1p),
            "log10" => Some(Builtin::MathLog10),
            "log2" => Some(Builtin::MathLog2),
            "max" => Some(Builtin::MathMax),
            "min" => Some(Builtin::MathMin),
            "pow" => Some(Builtin::MathPow),
            "random" => Some(Builtin::MathRandom),
            "round" => Some(Builtin::MathRound),
            "sign" => Some(Builtin::MathSign),
            "sin" => Some(Builtin::MathSin),
            "sinh" => Some(Builtin::MathSinh),
            "sqrt" => Some(Builtin::MathSqrt),
            "tan" => Some(Builtin::MathTan),
            "tanh" => Some(Builtin::MathTanh),
            "trunc" => Some(Builtin::MathTrunc),
            _ => None,
        },
        "Number" => match property {
            "isNaN" => Some(Builtin::NumberIsNaN),
            "isFinite" => Some(Builtin::NumberIsFinite),
            "isInteger" => Some(Builtin::NumberIsInteger),
            "isSafeInteger" => Some(Builtin::NumberIsSafeInteger),
            "parseInt" => Some(Builtin::NumberParseInt),
            "parseFloat" => Some(Builtin::NumberParseFloat),
            _ => None,
        },
        "Date" => match property {
            "now" => Some(Builtin::DateNow),
            "parse" => Some(Builtin::DateParse),
            "UTC" => Some(Builtin::DateUTC),
            _ => None,
        },
        _ => None,
    }
}

/// 将 Array.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `a.filter(cb)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
/// 仅包含使用 Type 12 影子栈调用约定的方法（Group 2）。
fn builtin_from_array_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "shift" => Some(ArrayShift),
        "unshift" => Some(ArrayUnshiftVa),
        "sort" => Some(ArraySort),
        "at" => Some(ArrayAt),
        "copyWithin" => Some(ArrayCopyWithin),
        "forEach" => Some(ArrayForEach),
        "map" => Some(ArrayMap),
        "filter" => Some(ArrayFilter),
        "reduce" => Some(ArrayReduce),
        "reduceRight" => Some(ArrayReduceRight),
        "find" => Some(ArrayFind),
        "findIndex" => Some(ArrayFindIndex),
        "some" => Some(ArraySome),
        "every" => Some(ArrayEvery),
        "flatMap" => Some(ArrayFlatMap),
        "flat" => Some(ArrayFlat),
        "concat" => Some(ArrayConcatVa),
        "splice" => Some(ArraySpliceVa),
        _ => None,
    }
}

fn builtin_from_function_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "call" => Some(Builtin::FuncCall),
        "apply" => Some(Builtin::FuncApply),
        "bind" => Some(Builtin::FuncBind),
        _ => None,
    }
}
/// 将 Object.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `obj.hasOwnProperty(key)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
///
/// 注意: toString / valueOf 未在此处拦截，因为它们在不同原型上有不同实现
/// (Array.prototype.toString、Date.prototype.valueOf 等)，编译时无法确定接收者类型。
/// 这些方法将在原型链实现后通过运行时属性查找调用。
fn builtin_from_object_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "hasOwnProperty" => Some(Builtin::HasOwnProperty),
        _ => None,
    }
}

/// 将 String.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `str.match(/.../)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
fn builtin_from_string_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "match" => Some(StringMatch),
        "replace" => Some(StringReplace),
        "search" => Some(StringSearch),
        "split" => Some(StringSplit),
        "at" => Some(StringAt),
        "charAt" => Some(StringCharAt),
        "charCodeAt" => Some(StringCharCodeAt),
        "codePointAt" => Some(StringCodePointAt),
        "concat" => Some(StringConcatVa),
        "endsWith" => Some(StringEndsWith),
        "includes" => Some(StringIncludes),
        "indexOf" => Some(StringIndexOf),
        "lastIndexOf" => Some(StringLastIndexOf),
        "matchAll" => Some(StringMatchAll),
        "padEnd" => Some(StringPadEnd),
        "padStart" => Some(StringPadStart),
        "repeat" => Some(StringRepeat),
        "replaceAll" => Some(StringReplaceAll),
        "slice" => Some(StringSlice),
        "startsWith" => Some(StringStartsWith),
        "substring" => Some(StringSubstring),
        "toLowerCase" => Some(StringToLowerCase),
        "toUpperCase" => Some(StringToUpperCase),
        "trim" => Some(StringTrim),
        "trimEnd" => Some(StringTrimEnd),
        "trimStart" => Some(StringTrimStart),
        "toString" => Some(StringToString),
        "valueOf" => Some(StringValueOf),
        _ => None,
    }
}

/// 将 RegExp.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `regex.test(str)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
fn builtin_from_regexp_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "test" => Some(RegExpTest),
        "exec" => Some(RegExpExec),
        _ => None,
    }
}
fn builtin_from_promise_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "then" => Some(PromiseThen),
        "catch" => Some(PromiseCatch),
        "finally" => Some(PromiseFinally),
        _ => None,
    }
}

fn builtin_from_number_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "toFixed" => Some(NumberProtoToFixed),
        "toExponential" => Some(NumberProtoToExponential),
        "toPrecision" => Some(NumberProtoToPrecision),
        _ => None,
    }
}

fn builtin_from_boolean_proto_method(name: &str) -> Option<Builtin> {
    // Boolean.prototype methods (toString, valueOf) are dispatched at runtime
    // via property lookup on the Boolean prototype object, not via CallBuiltin.
    let _ = name;
    None
}

fn builtin_from_error_proto_method(name: &str) -> Option<Builtin> {
    // Error.prototype methods (toString) are dispatched at runtime
    // via property lookup on the Error prototype object, not via CallBuiltin.
    let _ = name;
    None
}

fn builtin_call_signature(builtin: Builtin) -> (&'static str, usize) {
    match builtin {
        Builtin::ConsoleLog => ("console.log", 1),
        Builtin::ConsoleError => ("console.error", 1),
        Builtin::ConsoleWarn => ("console.warn", 1),
        Builtin::ConsoleInfo => ("console.info", 1),
        Builtin::ConsoleDebug => ("console.debug", 1),
        Builtin::ConsoleTrace => ("console.trace", 1),
        Builtin::DefineProperty => ("Object.defineProperty", 3),
        Builtin::GetOwnPropDesc => ("Object.getOwnPropertyDescriptor", 2),
        Builtin::SetTimeout => ("setTimeout", 2),
        Builtin::ClearTimeout => ("clearTimeout", 1),
        Builtin::SetInterval => ("setInterval", 2),
        Builtin::ClearInterval => ("clearInterval", 1),
        Builtin::Fetch => ("fetch", 1),
        Builtin::Eval => ("eval", 2),
        Builtin::EvalIndirect => ("eval.indirect", 1),
        Builtin::EvalResult => ("eval.result", 1),
        Builtin::JsonStringify => ("JSON.stringify", 1),
        Builtin::JsonParse => ("JSON.parse", 1),
        Builtin::AbstractEq => ("abstract-eq", 2),
        Builtin::AbstractCompare => ("abstract-compare", 2),
        Builtin::HasOwnProperty => ("Object.prototype.hasOwnProperty", 2),
        Builtin::ObjectProtoToString => ("Object.prototype.toString", 1),
        Builtin::ObjectProtoValueOf => ("Object.prototype.valueOf", 1),
        Builtin::ObjectKeys => ("Object.keys", 1),
        Builtin::ObjectValues => ("Object.values", 1),
        Builtin::ObjectEntries => ("Object.entries", 1),
        Builtin::ObjectAssign => ("Object.assign", 1),
        Builtin::ObjectCreate => ("Object.create", 1),
        Builtin::ObjectGetPrototypeOf => ("Object.getPrototypeOf", 1),
        Builtin::ObjectSetPrototypeOf => ("Object.setPrototypeOf", 2),
        Builtin::ObjectGetOwnPropertyNames => ("Object.getOwnPropertyNames", 1),
        Builtin::ObjectIs => ("Object.is", 2),
        // ── BigInt builtins ──
        Builtin::BigIntFromLiteral => ("BigInt.fromLiteral", 1),
        Builtin::BigIntAdd => ("BigInt.add", 2),
        Builtin::BigIntSub => ("BigInt.sub", 2),
        Builtin::BigIntMul => ("BigInt.mul", 2),
        Builtin::BigIntDiv => ("BigInt.div", 2),
        Builtin::BigIntMod => ("BigInt.mod", 2),
        Builtin::BigIntPow => ("BigInt.pow", 2),
        Builtin::BigIntNeg => ("BigInt.neg", 1),
        Builtin::BigIntEq => ("BigInt.eq", 2),
        Builtin::BigIntCmp => ("BigInt.cmp", 2),
        // ── Symbol builtins ──
        Builtin::SymbolCreate => ("Symbol", 0),
        Builtin::SymbolFor => ("Symbol.for", 1),
        Builtin::SymbolKeyFor => ("Symbol.keyFor", 1),
        Builtin::SymbolWellKnown => ("Symbol.wellKnown", 1),
        // ── RegExp builtins ──
        Builtin::RegExpCreate => ("RegExp.create", 2),
        Builtin::RegExpTest => ("RegExp.test", 2),
        Builtin::RegExpExec => ("RegExp.exec", 2),
        // ── String prototype builtins ──
        Builtin::StringMatch => ("String.prototype.match", 2),
        Builtin::StringReplace => ("String.prototype.replace", 3),
        Builtin::StringSearch => ("String.prototype.search", 2),
        Builtin::StringSplit => ("String.prototype.split", 3),
        Builtin::StringAt => ("String.prototype.at", 2),
        Builtin::StringCharAt => ("String.prototype.charAt", 2),
        Builtin::StringCharCodeAt => ("String.prototype.charCodeAt", 2),
        Builtin::StringCodePointAt => ("String.prototype.codePointAt", 2),
        Builtin::StringConcatVa => ("String.prototype.concat", 1),
        Builtin::StringEndsWith => ("String.prototype.endsWith", 3),
        Builtin::StringIncludes => ("String.prototype.includes", 3),
        Builtin::StringIndexOf => ("String.prototype.indexOf", 3),
        Builtin::StringLastIndexOf => ("String.prototype.lastIndexOf", 3),
        Builtin::StringMatchAll => ("String.prototype.matchAll", 2),
        Builtin::StringPadEnd => ("String.prototype.padEnd", 3),
        Builtin::StringPadStart => ("String.prototype.padStart", 3),
        Builtin::StringRepeat => ("String.prototype.repeat", 2),
        Builtin::StringReplaceAll => ("String.prototype.replaceAll", 3),
        Builtin::StringSlice => ("String.prototype.slice", 3),
        Builtin::StringStartsWith => ("String.prototype.startsWith", 3),
        Builtin::StringSubstring => ("String.prototype.substring", 3),
        Builtin::StringToLowerCase => ("String.prototype.toLowerCase", 1),
        Builtin::StringToUpperCase => ("String.prototype.toUpperCase", 1),
        Builtin::StringTrim => ("String.prototype.trim", 1),
        Builtin::StringTrimEnd => ("String.prototype.trimEnd", 1),
        Builtin::StringTrimStart => ("String.prototype.trimStart", 1),
        Builtin::StringToString => ("String.prototype.toString", 1),
        Builtin::StringValueOf => ("String.prototype.valueOf", 1),
        Builtin::StringIterator => ("String.prototype[@@iterator]", 1),
        Builtin::StringFromCharCode => ("String.fromCharCode", 1),
        Builtin::StringFromCodePoint => ("String.fromCodePoint", 1),
        // ── Number builtins ──
        Builtin::NumberConstructor => ("Number", 1),
        Builtin::NumberIsNaN => ("Number.isNaN", 1),
        Builtin::NumberIsFinite => ("Number.isFinite", 1),
        Builtin::NumberIsInteger => ("Number.isInteger", 1),
        Builtin::NumberIsSafeInteger => ("Number.isSafeInteger", 1),
        Builtin::NumberParseInt => ("Number.parseInt", 1),
        Builtin::NumberParseFloat => ("Number.parseFloat", 1),
        Builtin::NumberProtoToString => ("Number.prototype.toString", 1),
        Builtin::NumberProtoValueOf => ("Number.prototype.valueOf", 1),
        Builtin::NumberProtoToFixed => ("Number.prototype.toFixed", 1),
        Builtin::NumberProtoToExponential => ("Number.prototype.toExponential", 1),
        Builtin::NumberProtoToPrecision => ("Number.prototype.toPrecision", 1),
        // ── Boolean builtins ──
        Builtin::BooleanConstructor => ("Boolean", 1),
        Builtin::BooleanProtoToString => ("Boolean.prototype.toString", 1),
        Builtin::BooleanProtoValueOf => ("Boolean.prototype.valueOf", 1),
        // ── Error builtins ──
        Builtin::ErrorConstructor => ("Error", 1),
        Builtin::TypeErrorConstructor => ("TypeError", 1),
        Builtin::RangeErrorConstructor => ("RangeError", 1),
        Builtin::SyntaxErrorConstructor => ("SyntaxError", 1),
        Builtin::ReferenceErrorConstructor => ("ReferenceError", 1),
        Builtin::URIErrorConstructor => ("URIError", 1),
        Builtin::EvalErrorConstructor => ("EvalError", 1),
        Builtin::ErrorProtoToString => ("Error.prototype.toString", 1),
        // ── Map builtins ──
        Builtin::MapConstructor => ("Map", 0),
        // ── Set builtins ──
        Builtin::SetConstructor => ("Set", 0),
        // ── WeakMap builtins ──
        Builtin::WeakMapConstructor => ("WeakMap", 0),
        Builtin::WeakMapProtoSet => ("WeakMap.prototype.set", 3),
        Builtin::WeakMapProtoGet => ("WeakMap.prototype.get", 2),
        Builtin::WeakMapProtoHas => ("WeakMap.prototype.has", 2),
        Builtin::WeakMapProtoDelete => ("WeakMap.prototype.delete", 2),
        // ── WeakSet builtins ──
        Builtin::WeakSetConstructor => ("WeakSet", 0),
        Builtin::WeakSetProtoAdd => ("WeakSet.prototype.add", 2),
        Builtin::WeakSetProtoHas => ("WeakSet.prototype.has", 2),
        Builtin::WeakSetProtoDelete => ("WeakSet.prototype.delete", 2),
        // ── ArrayBuffer builtins ──
        Builtin::ArrayBufferConstructor => ("ArrayBuffer", 1),
        Builtin::ArrayBufferProtoByteLength => ("ArrayBuffer.prototype.byteLength", 1),
        Builtin::ArrayBufferProtoSlice => ("ArrayBuffer.prototype.slice", 3),
        // ── DataView builtins ──
        Builtin::DataViewConstructor => ("DataView", 3),
        Builtin::DataViewProtoGetFloat64 => ("DataView.prototype.getFloat64", 2),
        Builtin::DataViewProtoGetFloat32 => ("DataView.prototype.getFloat32", 2),
        Builtin::DataViewProtoGetInt32 => ("DataView.prototype.getInt32", 2),
        Builtin::DataViewProtoGetUint32 => ("DataView.prototype.getUint32", 2),
        Builtin::DataViewProtoGetInt16 => ("DataView.prototype.getInt16", 2),
        Builtin::DataViewProtoGetUint16 => ("DataView.prototype.getUint16", 2),
        Builtin::DataViewProtoGetInt8 => ("DataView.prototype.getInt8", 2),
        Builtin::DataViewProtoGetUint8 => ("DataView.prototype.getUint8", 2),
        Builtin::DataViewProtoSetFloat64 => ("DataView.prototype.setFloat64", 3),
        Builtin::DataViewProtoSetFloat32 => ("DataView.prototype.setFloat32", 3),
        Builtin::DataViewProtoSetInt32 => ("DataView.prototype.setInt32", 3),
        Builtin::DataViewProtoSetUint32 => ("DataView.prototype.setUint32", 3),
        Builtin::DataViewProtoSetInt16 => ("DataView.prototype.setInt16", 3),
        Builtin::DataViewProtoSetUint16 => ("DataView.prototype.setUint16", 3),
        Builtin::DataViewProtoSetInt8 => ("DataView.prototype.setInt8", 3),
        Builtin::DataViewProtoSetUint8 => ("DataView.prototype.setUint8", 3),
        // ── TypedArray constructors ──
        Builtin::Int8ArrayConstructor => ("Int8Array", 3),
        Builtin::Uint8ArrayConstructor => ("Uint8Array", 3),
        Builtin::Uint8ClampedArrayConstructor => ("Uint8ClampedArray", 3),
        Builtin::Int16ArrayConstructor => ("Int16Array", 3),
        Builtin::Uint16ArrayConstructor => ("Uint16Array", 3),
        Builtin::Int32ArrayConstructor => ("Int32Array", 3),
        Builtin::Uint32ArrayConstructor => ("Uint32Array", 3),
        Builtin::Float32ArrayConstructor => ("Float32Array", 3),
        Builtin::Float64ArrayConstructor => ("Float64Array", 3),
        // ── TypedArray prototype methods ──
        Builtin::TypedArrayProtoLength => ("TypedArray.prototype.length", 1),
        Builtin::TypedArrayProtoByteLength => ("TypedArray.prototype.byteLength", 1),
        Builtin::TypedArrayProtoByteOffset => ("TypedArray.prototype.byteOffset", 1),
        Builtin::TypedArrayProtoSet => ("TypedArray.prototype.set", 3),
        Builtin::TypedArrayProtoSlice => ("TypedArray.prototype.slice", 3),
        Builtin::TypedArrayProtoSubarray => ("TypedArray.prototype.subarray", 3),
        // ── Date builtins ──
        Builtin::DateConstructor => ("Date", 0),
        Builtin::DateNow => ("Date.now", 0),
        Builtin::DateParse => ("Date.parse", 1),
        Builtin::DateUTC => ("Date.UTC", 2),
        _ => ("builtin", 0),
    }
}

fn direct_eval_predeclare_code(
    expr: &swc_ast::Expr,
    eval_string_bindings: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let swc_ast::Expr::Call(call) = expr else {
        return None;
    };
    let swc_ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let swc_ast::Expr::Ident(ident) = callee.as_ref() else {
        return None;
    };
    if ident.sym.as_ref() != "eval" {
        return None;
    }
    let first = call.args.first()?;
    literal_string_expr(&Some(first.expr.clone())).or_else(|| {
        let swc_ast::Expr::Ident(arg_ident) = first.expr.as_ref() else {
            return None;
        };
        eval_string_bindings.get(arg_ident.sym.as_ref()).cloned()
    })
}

fn literal_string_expr(expr: &Option<Box<swc_ast::Expr>>) -> Option<String> {
    let swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) = expr.as_deref()? else {
        return None;
    };
    Some(string.value.to_string_lossy().into_owned())
}

fn eval_code_has_use_strict_directive(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut index = 0;
    skip_js_trivia(bytes, &mut index);

    while let Some(quote @ (b'\'' | b'"')) = bytes.get(index).copied() {
        index += 1;
        let literal_start = index;
        while index < bytes.len() && bytes[index] != quote {
            if bytes[index] == b'\\' {
                return false;
            }
            index += 1;
        }
        if index >= bytes.len() {
            return false;
        }

        let directive = &code[literal_start..index];
        index += 1;
        skip_js_trivia(bytes, &mut index);
        if bytes.get(index) == Some(&b';') {
            index += 1;
        }

        if directive == "use strict" {
            return true;
        }

        skip_js_trivia(bytes, &mut index);
    }

    false
}

fn skip_js_trivia(bytes: &[u8], index: &mut usize) {
    loop {
        while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {
            *index += 1;
        }

        if bytes.get(*index..*index + 2) == Some(b"//") {
            *index += 2;
            while *index < bytes.len() && !matches!(bytes[*index], b'\n' | b'\r') {
                *index += 1;
            }
            continue;
        }

        if bytes.get(*index..*index + 2) == Some(b"/*") {
            *index += 2;
            while *index + 1 < bytes.len() && bytes.get(*index..*index + 2) != Some(b"*/") {
                *index += 1;
            }
            if *index + 1 < bytes.len() {
                *index += 2;
            }
            continue;
        }

        break;
    }
}

fn module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
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

fn eval_literal_binding_names(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = code.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if is_word_at(bytes, index, b"var") {
            index += 3;
            loop {
                while index < bytes.len()
                    && (bytes[index].is_ascii_whitespace() || bytes[index] == b',')
                {
                    index += 1;
                }
                if index >= bytes.len() || !is_ident_start(bytes[index]) {
                    break;
                }
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let name = &code[start..index];
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
                while index < bytes.len()
                    && bytes[index] != b','
                    && bytes[index] != b';'
                    && bytes[index] != b'\n'
                {
                    index += 1;
                }
                if index >= bytes.len() || bytes[index] != b',' {
                    break;
                }
            }
        } else if is_word_at(bytes, index, b"function") {
            index += "function".len();
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && bytes[index] == b'*' {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            if index < bytes.len() && is_ident_start(bytes[index]) {
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let name = &code[start..index];
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
            }
        }
        index += 1;
    }
    names
}

fn is_word_at(bytes: &[u8], index: usize, word: &[u8]) -> bool {
    bytes.get(index..index + word.len()) == Some(word)
        && index
            .checked_sub(1)
            .map_or(true, |prev| !is_ident_continue(bytes[prev]))
        && bytes
            .get(index + word.len())
            .map_or(true, |next| !is_ident_continue(*next))
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn assign_op_to_binary(op: swc_ast::AssignOp) -> Option<BinaryOp> {
    match op {
        swc_ast::AssignOp::AddAssign => Some(BinaryOp::Add),
        swc_ast::AssignOp::SubAssign => Some(BinaryOp::Sub),
        swc_ast::AssignOp::MulAssign => Some(BinaryOp::Mul),
        swc_ast::AssignOp::DivAssign => Some(BinaryOp::Div),
        swc_ast::AssignOp::ModAssign => Some(BinaryOp::Mod),
        swc_ast::AssignOp::ExpAssign => Some(BinaryOp::Exp),
        swc_ast::AssignOp::BitAndAssign => Some(BinaryOp::BitAnd),
        swc_ast::AssignOp::BitOrAssign => Some(BinaryOp::BitOr),
        swc_ast::AssignOp::BitXorAssign => Some(BinaryOp::BitXor),
        swc_ast::AssignOp::LShiftAssign => Some(BinaryOp::Shl),
        swc_ast::AssignOp::RShiftAssign => Some(BinaryOp::Shr),
        swc_ast::AssignOp::ZeroFillRShiftAssign => Some(BinaryOp::UShr),
        _ => None,
    }
}

// ── Kind strings ────────────────────────────────────────────────────────

#[allow(dead_code)]
fn stmt_kind(stmt: &swc_ast::Stmt) -> &'static str {
    match stmt {
        swc_ast::Stmt::Block(_) => "block",
        swc_ast::Stmt::Empty(_) => "empty",
        swc_ast::Stmt::Debugger(_) => "debugger",
        swc_ast::Stmt::With(_) => "with",
        swc_ast::Stmt::Return(_) => "return",
        swc_ast::Stmt::Labeled(_) => "labeled",
        swc_ast::Stmt::Break(_) => "break",
        swc_ast::Stmt::Continue(_) => "continue",
        swc_ast::Stmt::If(_) => "if",
        swc_ast::Stmt::Switch(_) => "switch",
        swc_ast::Stmt::Throw(_) => "throw",
        swc_ast::Stmt::Try(_) => "try",
        swc_ast::Stmt::While(_) => "while",
        swc_ast::Stmt::DoWhile(_) => "do-while",
        swc_ast::Stmt::For(_) => "for",
        swc_ast::Stmt::ForIn(_) => "for-in",
        swc_ast::Stmt::ForOf(_) => "for-of",
        swc_ast::Stmt::Decl(_) => "decl",
        swc_ast::Stmt::Expr(_) => "expr",
    }
}

fn decl_kind(decl: &swc_ast::Decl) -> &'static str {
    match decl {
        swc_ast::Decl::Class(_) => "class",
        swc_ast::Decl::Fn(_) => "function",
        swc_ast::Decl::Var(_) => "var",
        swc_ast::Decl::Using(_) => "using",
        swc_ast::Decl::TsInterface(_) => "ts-interface",
        swc_ast::Decl::TsTypeAlias(_) => "ts-type-alias",
        swc_ast::Decl::TsEnum(_) => "ts-enum",
        swc_ast::Decl::TsModule(_) => "ts-module",
    }
}

fn expr_kind(expr: &swc_ast::Expr) -> &'static str {
    match expr {
        swc_ast::Expr::This(_) => "this",
        swc_ast::Expr::Array(_) => "array",
        swc_ast::Expr::Object(_) => "object",
        swc_ast::Expr::Fn(_) => "function",
        swc_ast::Expr::Unary(_) => "unary",
        swc_ast::Expr::Update(_) => "update",
        swc_ast::Expr::Bin(_) => "binary",
        swc_ast::Expr::Assign(_) => "assign",
        swc_ast::Expr::Member(_) => "member",
        swc_ast::Expr::SuperProp(_) => "super-prop",
        swc_ast::Expr::Cond(_) => "conditional",
        swc_ast::Expr::Call(_) => "call",
        swc_ast::Expr::New(_) => "new",
        swc_ast::Expr::Seq(_) => "sequence",
        swc_ast::Expr::Ident(_) => "identifier",
        swc_ast::Expr::Lit(_) => "literal",
        swc_ast::Expr::Tpl(_) => "template",
        swc_ast::Expr::TaggedTpl(_) => "tagged-template",
        swc_ast::Expr::Arrow(_) => "arrow",
        swc_ast::Expr::Class(_) => "class",
        swc_ast::Expr::Yield(_) => "yield",
        swc_ast::Expr::MetaProp(_) => "meta-prop",
        swc_ast::Expr::Await(_) => "await",
        swc_ast::Expr::Paren(_) => "paren",
        swc_ast::Expr::JSXMember(_) => "jsx-member",
        swc_ast::Expr::JSXNamespacedName(_) => "jsx-namespaced-name",
        swc_ast::Expr::JSXEmpty(_) => "jsx-empty",
        swc_ast::Expr::JSXElement(_) => "jsx-element",
        swc_ast::Expr::JSXFragment(_) => "jsx-fragment",
        swc_ast::Expr::TsTypeAssertion(_) => "ts-type-assertion",
        swc_ast::Expr::TsConstAssertion(_) => "ts-const-assertion",
        swc_ast::Expr::TsNonNull(_) => "ts-non-null",
        swc_ast::Expr::TsAs(_) => "ts-as",
        swc_ast::Expr::TsInstantiation(_) => "ts-instantiation",
        swc_ast::Expr::TsSatisfies(_) => "ts-satisfies",
        swc_ast::Expr::PrivateName(_) => "private-name",
        swc_ast::Expr::OptChain(_) => "optional-chain",
        swc_ast::Expr::Invalid(_) => "invalid",
    }
}

fn literal_kind(lit: &swc_ast::Lit) -> &'static str {
    match lit {
        swc_ast::Lit::Str(_) => "string",
        swc_ast::Lit::Bool(_) => "bool",
        swc_ast::Lit::Null(_) => "null",
        swc_ast::Lit::Num(_) => "number",
        swc_ast::Lit::BigInt(_) => "bigint",
        swc_ast::Lit::Regex(_) => "regex",
        swc_ast::Lit::JSXText(_) => "jsx-text",
    }
}

#[allow(dead_code)]
fn module_decl_kind(decl: &swc_ast::ModuleDecl) -> &'static str {
    match decl {
        swc_ast::ModuleDecl::Import(_) => "import",
        swc_ast::ModuleDecl::ExportDecl(_) => "export-decl",
        swc_ast::ModuleDecl::ExportNamed(_) => "export-named",
        swc_ast::ModuleDecl::ExportDefaultDecl(_) => "export-default-decl",
        swc_ast::ModuleDecl::ExportDefaultExpr(_) => "export-default-expr",
        swc_ast::ModuleDecl::ExportAll(_) => "export-all",
        swc_ast::ModuleDecl::TsImportEquals(_) => "ts-import-equals",
        swc_ast::ModuleDecl::TsExportAssignment(_) => "ts-export-assignment",
        swc_ast::ModuleDecl::TsNamespaceExport(_) => "ts-namespace-export",
    }
}

/// 从 Decl 中提取所有导出的标识符名称
fn decl_exported_names(decl: &swc_ast::Decl) -> Vec<String> {
    match decl {
        swc_ast::Decl::Var(var_decl) => {
            var_decl
                .decls
                .iter()
                .map(|d| {
                    match &d.name {
                        swc_ast::Pat::Ident(ident) => ident.id.sym.to_string(),
                        _ => String::new(), // 解构导出暂不支持
                    }
                })
                .filter(|s| !s.is_empty())
                .collect()
        }
        swc_ast::Decl::Fn(fn_decl) => {
            vec![fn_decl.ident.sym.to_string()]
        }
        swc_ast::Decl::Class(class_decl) => {
            vec![class_decl.ident.sym.to_string()]
        }
        swc_ast::Decl::TsInterface(ts_iface) => {
            vec![ts_iface.id.sym.to_string()]
        }
        swc_ast::Decl::TsTypeAlias(ts_alias) => {
            vec![ts_alias.id.sym.to_string()]
        }
        swc_ast::Decl::TsEnum(ts_enum) => {
            vec![ts_enum.id.sym.to_string()]
        }
        swc_ast::Decl::TsModule(ts_module) => match &ts_module.id {
            swc_ast::TsModuleName::Ident(ident) => vec![ident.sym.to_string()],
            _ => vec![],
        },
        _ => vec![],
    }
}
