use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, HomeObject, Instruction, MODULE_ENTRY_IR_NAME, Module, PhiSource, Program,
    SwitchCaseTarget, Terminator, UnaryOp, ValueId,
};

const EVAL_SCOPE_ENV_PARAM: &str = "$eval_env";

use wjsm_ir::wk_symbol;
// 保留旧常量名作为别名，避免改动所有引用点
const WK_SYMBOL_ITERATOR: u32 = wk_symbol::ITERATOR;
const WK_SYMBOL_SPECIES: u32 = wk_symbol::SPECIES;
const WK_SYMBOL_TO_STRING_TAG: u32 = wk_symbol::TO_STRING_TAG;
const WK_SYMBOL_ASYNC_ITERATOR: u32 = wk_symbol::ASYNC_ITERATOR;
const WK_SYMBOL_HAS_INSTANCE: u32 = wk_symbol::HAS_INSTANCE;
const WK_SYMBOL_TO_PRIMITIVE: u32 = wk_symbol::TO_PRIMITIVE;
const WK_SYMBOL_DISPOSE: u32 = wk_symbol::DISPOSE;
const WK_SYMBOL_MATCH: u32 = wk_symbol::MATCH;
const WK_SYMBOL_ASYNC_DISPOSE: u32 = wk_symbol::ASYNC_DISPOSE;

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
    /// `true` only for the implicit `arguments` binding created by emit_arguments_init.
    /// Used to distinguish implicit `arguments` from explicit `var`/`let`/`const arguments`.
    implicit_arguments: bool,
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
        let arenas = vec![root];
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

        scope.variables.insert(
            name.to_string(),
            VarInfo {
                kind,
                initialised,
                implicit_arguments: false,
            },
        );
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

    /// Return all lexically visible bindings, including uninitialized (TDZ) ones.
    /// Returns (scope_id, name, kind, is_initialised).
    fn visible_bindings_all(&self) -> Vec<(usize, String, VarKind, bool)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &self.arenas[scope_id];
            let mut names: Vec<_> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if seen.insert(name.clone())
                    && let Some(info) = scope.variables.get(&name)
                {
                    result.push((scope.id, name.clone(), info.kind, info.initialised));
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

    /// True when the current function scope already has a binding named `arguments` (e.g. parameter).
    fn current_function_has_param_arguments(&self) -> bool {
        let Ok(scope_id) = self.nearest_function_scope() else {
            return false;
        };
        self.arenas[scope_id].variables.contains_key("arguments")
    }

    /// Mark an existing variable as implicit `arguments`.
    pub(crate) fn set_implicit_arguments(&mut self, name: &str) -> Result<(), String> {
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &mut self.arenas[scope_id];
            if let Some(info) = scope.variables.get_mut(name) {
                info.implicit_arguments = true;
                return Ok(());
            }
            cursor = scope.parent;
        }
        Err(format!("undeclared identifier `{name}`"))
    }
}

// ── CFG Builder ─────────────────────────────────────────────────────────

/// Internal helper that encapsulates CFG construction for one function.
struct FunctionBuilder {
    _name: String,
    _entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
    has_eval: bool,
    /// 该函数调用的"已知函数声明"变量名→FunctionId（Layer 3 callee 分析）。
    /// store_function_decl_callee 填充，finalize 时转移到 IR Function。
    known_callee_vars: std::collections::HashMap<String, wjsm_ir::FunctionId>,
}

impl FunctionBuilder {
    fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            _name: name.into(),
            _entry: entry,
            blocks: vec![BasicBlock::new(entry)],
            has_eval: false,
            known_callee_vars: std::collections::HashMap::new(),
        }
    }

    fn mark_has_eval(&mut self) {
        self.has_eval = true;
    }

    fn has_eval(&self) -> bool {
        self.has_eval
    }

    /// 记录 callee 变量（scope-qualified IR name）→ FunctionId（Layer 3）。
    fn record_known_callee(&mut self, ir_name: String, function_id: wjsm_ir::FunctionId) {
        self.known_callee_vars.insert(ir_name, function_id);
    }

    fn take_known_callee_vars(&mut self) -> std::collections::HashMap<String, wjsm_ir::FunctionId> {
        std::mem::take(&mut self.known_callee_vars)
    }

    fn name(&self) -> &str {
        &self._name
    }

    fn new_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::new(id));
        id
    }
    fn last_block_id(&self) -> BasicBlockId {
        BasicBlockId(self.blocks.len().saturating_sub(1) as u32)
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

    /// 以只读切片暴露当前函数的 blocks，用于函数级分析阶段。
    fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
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
                    .is_some_and(|b| matches!(b.terminator(), Terminator::Unreachable));
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
                .any(|e| e.as_ref().is_some_and(|e| expr_has_await(&e.expr))),
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
                        .is_some_and(|a| a.iter().any(|a| expr_has_await(&a.expr)))
            }
            swc_ast::Expr::Seq(s) => s.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::Tpl(t) => t.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::TaggedTpl(t) => {
                expr_has_await(&t.tag) || t.tpl.exprs.iter().any(|e| expr_has_await(e))
            }
            swc_ast::Expr::Yield(y) => y.arg.as_ref().is_some_and(|a| expr_has_await(a)),
            swc_ast::Expr::Paren(p) => expr_has_await(&p.expr),
            _ => false,
        }
    }

    fn decl_has_await(decl: &swc_ast::Decl) -> bool {
        match decl {
            swc_ast::Decl::Var(v) => v
                .decls
                .iter()
                .any(|d| d.init.as_ref().is_some_and(|i| expr_has_await(i))),
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
                    || i.alt.as_ref().is_some_and(|a| stmt_has_await(a))
            }
            swc_ast::Stmt::While(w) => expr_has_await(&w.test) || stmt_has_await(&w.body),
            swc_ast::Stmt::DoWhile(d) => expr_has_await(&d.test) || stmt_has_await(&d.body),
            swc_ast::Stmt::For(f) => {
                f.init.as_ref().is_some_and(|init| match init {
                    swc_ast::VarDeclOrExpr::VarDecl(v) => v
                        .decls
                        .iter()
                        .any(|d| d.init.as_ref().is_some_and(|i| expr_has_await(i))),
                    swc_ast::VarDeclOrExpr::Expr(e) => expr_has_await(e),
                }) || f.test.as_ref().is_some_and(|t| expr_has_await(t))
                    || f.update.as_ref().is_some_and(|u| expr_has_await(u))
                    || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::ForIn(f) => expr_has_await(&f.right) || stmt_has_await(&f.body),
            swc_ast::Stmt::ForOf(f) => {
                f.is_await || expr_has_await(&f.right) || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::Return(r) => r.arg.as_ref().is_some_and(|a| expr_has_await(a)),
            swc_ast::Stmt::Throw(t) => expr_has_await(&t.arg),
            swc_ast::Stmt::Try(t) => {
                t.block.stmts.iter().any(stmt_has_await)
                    || t.handler
                        .as_ref()
                        .is_some_and(|h| h.body.stmts.iter().any(stmt_has_await))
                    || t.finalizer
                        .as_ref()
                        .is_some_and(|f| f.stmts.iter().any(stmt_has_await))
            }
            swc_ast::Stmt::Switch(s) => {
                expr_has_await(&s.discriminant)
                    || s.cases.iter().any(|c| {
                        c.test.as_ref().is_some_and(|t| expr_has_await(t))
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
                swc_ast::ModuleDecl::ExportDefaultExpr(e) if expr_has_await(&e.expr) => {
                    return true;
                }
                _ => {}
            },
        }
    }
    false
}

pub fn lower_module(module: swc_ast::Module, script: bool) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.script_mode = script;
    lowerer.lower_module(&module)
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
    lowerer.eval_scope_record = true;
    lowerer.strict_mode = module_has_use_strict_directive(&module);
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
        return lower_module(module, false);
    }

    // 多模块编译路径
    // 早错误：对每个模块运行私有名静态校验（与单模块路径一致）。
    for (_, module_ast) in &modules {
        lowerer_classes_ts::validate_private_names(module_ast)?;
    }
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
        let known_callees = lowerer.current_function.take_known_callee_vars();
        let blocks = lowerer.current_function.into_blocks();
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
    function_hoisted_stack: Vec<FunctionHoistedState>,
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
    /// 当前函数是否拥有 [[HomeObject]] / 可合法解析 super。
    super_allowed: bool,
    /// 当前函数是否可合法执行 super() 构造调用。
    super_call_allowed: bool,
    function_super_allowed_stack: Vec<bool>,
    function_super_call_allowed_stack: Vec<bool>,
    function_is_arrow_stack: Vec<bool>,
    function_is_method_stack: Vec<bool>,
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
    pending_suspends: Vec<lowerer_async_eval::PendingSuspend>,
    strict_mode: bool,
    pub(crate) is_arrow: bool,
    pub(crate) is_method: bool,
    /// 当前函数形参个数，供 emit_arguments_init 使用。
    arguments_param_count: u32,
    script_mode: bool,
    eval_mode: bool,
    eval_has_scope_bridge: bool,
    eval_var_writes_to_scope: bool,
    pub(crate) eval_scope_record: bool,
    pub(crate) eval_caller_has_arguments: bool,
    eval_completion: Option<ValueId>,
    /// eval 调用在表达式上下文时的异常检查分叉后的 continue block。
    /// 由 lower_direct_eval_call 设置，由 resolve_store_block 消费。
    pub(crate) eval_continue_block: Option<BasicBlockId>,
    /// 由 lower_new_expr 在构建了异常检查分叉后设置，由 resolve_store_block 消费。
    pub(crate) new_expr_continue_block: Option<BasicBlockId>,
    /// 由 await 表达式设置，由 resolve_store_block 消费。
    pub(crate) await_continue_block: Option<BasicBlockId>,
    /// 由 lower_logical / lower_cond 在创建控制流表达式后设置其 merge block，
    /// 由 resolve_store_block 消费，确保后续指令插入到正确的继续块中。
    pub(crate) expr_merge_block: Option<BasicBlockId>,
    /// 当前作用域中活跃的 using 变量（用于作用域退出时自动 dispose）
    active_using_vars: Vec<ActiveUsingVar>,
    /// 追踪当前作用域中已推断为 TypedArray 的绑定（scope_id, name）。
    /// 用于在 lower_call_expr 中让 arr.at()/arr.indexOf() 等走 TypedArray dispatch，
    /// 而不是被 String.prototype dispatch 错误拦截。
    typedarray_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 SharedArrayBuffer 的绑定（scope_id, name）。
    /// 用于在 lower_call_expr 中让 sab.slice() / sab.grow() 等走 SAB dispatch，
    /// 而不是被 String.prototype dispatch 错误拦截（修复评审 P1 劫持问题）。
    sab_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 DataView 的绑定（scope_id, name）。
    /// DataView 原型方法使用专用宿主导入签名，静态已知 receiver 必须直连 CallBuiltin，避免通用 call_indirect 调用约定不匹配。
    dataview_bindings: std::collections::HashSet<(usize, String)>,
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
    pending_suspends: Vec<lowerer_async_eval::PendingSuspend>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct HoistedVar {
    scope_id: usize,
    name: String,
}

type HoistedBindingSet = std::collections::HashSet<(usize, String)>;
type FunctionHoistedState = (Vec<HoistedVar>, HoistedBindingSet);

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

    fn lexical_new_target() -> Self {
        Self {
            name: "__wjsm_new_target".to_string(),
            scope_id: None,
        }
    }

    fn is_lexical_new_target(&self) -> bool {
        self.scope_id.is_none() && self.name == "__wjsm_new_target"
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

mod lowerer_arrows;
mod lowerer_assignments;
mod lowerer_async_eval;
mod lowerer_binary_expr;
mod lowerer_branching;
mod lowerer_calls_eval;
mod lowerer_classes_ts;
mod lowerer_core;
mod lowerer_declarations;
mod lowerer_function_decls;
mod lowerer_functions;
mod lowerer_jsx_objects;
mod lowerer_predeclare;
mod lowerer_stmt;
mod lowerer_ts;

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

mod ast_kinds;
mod builtins;
mod eval_scan;

use ast_kinds::*;
use builtins::*;
pub use eval_scan::eval_literal_binding_names;
use eval_scan::*;
/// 判断表达式是否为 TypedArray 构造函数调用（`new Int8Array(...)` 等形式）。
fn is_typedarray_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return matches!(
            ident.sym.as_ref(),
            "Int8Array"
                | "Uint8Array"
                | "Uint8ClampedArray"
                | "Int16Array"
                | "Uint16Array"
                | "Int32Array"
                | "Uint32Array"
                | "Float32Array"
                | "Float64Array"
                | "BigInt64Array"
                | "BigUint64Array"
        );
    }
    false
}
/// 判断表达式是否为 SharedArrayBuffer 构造函数调用（`new SharedArrayBuffer(...)` 形式）。
fn is_sharedarraybuffer_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "SharedArrayBuffer";
    }
    false
}
/// 判断表达式是否为 DataView 构造函数调用（`new DataView(...)` 形式）。
fn is_dataview_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "DataView";
    }
    false
}
