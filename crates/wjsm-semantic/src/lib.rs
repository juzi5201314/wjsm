use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp, ValueId,
};

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
}

impl FunctionBuilder {
    fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            _name: name.into(),
            _entry: entry,
            blocks: vec![BasicBlock::new(entry)],
        }
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
        Function::new(self._name, entry)
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

pub fn lower_module(module: swc_ast::Module) -> Result<Program, LoweringError> {
    Lowerer::new().lower_module(&module)
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
    function_scope_stack: Vec<ScopeTree>,
    function_hoisted_stack: Vec<(Vec<HoistedVar>, std::collections::HashSet<(usize, String)>)>,
    function_next_value_stack: Vec<u32>,
    function_next_temp_stack: Vec<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct HoistedVar {
    scope_id: usize,
    name: String,
}

impl Lowerer {
    fn new() -> Self {
        Self {
            module: Module::new(),
            next_value: 0,
            scopes: ScopeTree::new(),
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
            function_scope_stack: Vec::new(),
            function_hoisted_stack: Vec::new(),
            function_next_value_stack: Vec::new(),
            function_next_temp_stack: Vec::new(),
        }
    }

    fn push_function_context(&mut self, name: impl Into<String>, entry: BasicBlockId) {
        self.function_stack.push(std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new(name, entry),
        ));
        self.function_scope_stack
            .push(std::mem::replace(&mut self.scopes, ScopeTree::new()));
        self.function_hoisted_stack.push((
            std::mem::take(&mut self.hoisted_vars),
            std::mem::take(&mut self.hoisted_vars_set),
        ));
        self.function_next_value_stack.push(self.next_value);
        self.function_next_temp_stack.push(self.next_temp);
        self.next_value = 0;
        self.next_temp = 0;
        self.label_stack.clear();
        self.finally_stack.clear();
        self.try_contexts.clear();
        self.active_finalizers.clear();
        self.pending_loop_label = None;
    }

    fn pop_function_context(&mut self) {
        self.current_function = self.function_stack.pop().expect("function stack underflow");
        self.scopes = self
            .function_scope_stack
            .pop()
            .expect("scope stack underflow");
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
    }

    fn lower_module(mut self, module: &swc_ast::Module) -> Result<Program, LoweringError> {
        // Pre-scan: hoist variable declarations so let/const are in TDZ.
        self.predeclare_stmts(&module.body)?;

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut flow = StmtFlow::Open(entry);

        for item in &module.body {
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    flow = self.lower_stmt(stmt, flow)?;
                }
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    return Err(self.error(
                        decl.span(),
                        format!(
                            "unsupported module declaration kind `{}`",
                            module_decl_kind(decl)
                        ),
                    ));
                }
            }
        }

        // If the last block is still open and hasn't been terminated, give it a Return.
        match flow {
            StmtFlow::Open(block) => {
                // 性能优化：使用 matches! 直接匹配引用，避免克隆整个 Terminator。
                // Terminator 可能包含 Vec（Switch 变体），克隆有内存分配开销。
                let is_unreachable = self
                    .current_function
                    .block(block)
                    .map_or(false, |b| matches!(b.terminator(), Terminator::Unreachable));
                if is_unreachable {
                    self.current_function
                        .set_terminator(block, Terminator::Return { value: None });
                }
            }
            StmtFlow::Terminated => {}
        }

        let blocks = self.current_function.into_blocks();
        let mut function = Function::new("main", BasicBlockId(0));
        for block in blocks {
            function.push_block(block);
        }
        self.module.push_function(function);
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
        let result_block = match expr_stmt.expr.as_ref() {
            swc_ast::Expr::Call(call) => {
                if is_console_log(call) {
                    self.lower_console_log_call(call, block)?
                } else {
                    self.lower_call(call, block)?
                }
            }
            expr => {
                let _ = self.lower_expr(expr, block)?;
                self.resolve_store_block(block)
            }
        };
        Ok(StmtFlow::Open(result_block))
    }

    fn lower_console_log_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let first_arg = call
            .args
            .first()
            .ok_or_else(|| self.error(call.span(), "console.log requires at least 1 argument"))?;

        let value = self.lower_expr(first_arg.expr.as_ref(), block)?;

        // If lower_expr terminated the block (e.g. ternary/short-circuit),
        // place the CallBuiltin in the merge block instead.
        let call_block = self.resolve_store_block(block);

        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ConsoleLog,
                args: vec![value],
            },
        );
        Ok(call_block)
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
        self.scopes.push_scope(ScopeKind::Block);
        self.predeclare_block_stmts(&block_stmt.stmts)?;

        let mut flow = flow;
        for stmt in &block_stmt.stmts {
            flow = self.lower_stmt(stmt, flow)?;
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

        // lower 'then' branch
        let then_flow = self.lower_stmt(&if_stmt.cons, StmtFlow::Open(then_block))?;

        let has_else = if let Some(alt) = &if_stmt.alt {
            // 'else' uses else_or_merge as its entry
            let else_flow = self.lower_stmt(alt, StmtFlow::Open(else_or_merge))?;
            match (then_flow, else_flow) {
                (StmtFlow::Terminated, StmtFlow::Terminated) => StmtFlow::Terminated,
                _ => {
                    // Create a merge block only if at least one path doesn't terminate
                    let merge = self.current_function.new_block();
                    let after_then = self
                        .current_function
                        .ensure_jump_or_terminated(then_flow, merge);
                    let _after_else = self
                        .current_function
                        .ensure_jump_or_terminated(else_flow, merge);
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
            StmtFlow::Open(merge)
        };

        Ok(has_else)
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
                // for (x in obj) or for (x of obj) — x must be an existing variable
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
                    _ => Err(self.error(
                        pat.span(),
                        "destructuring patterns in for...in/for...of are not yet supported",
                    )),
                }
            }
            swc_ast::ForHead::VarDecl(var_decl) => {
                // for (let x in obj) or for (const x in obj)
                let _kind = match var_decl.kind {
                    swc_ast::VarDeclKind::Var => VarKind::Var,
                    swc_ast::VarDeclKind::Let => VarKind::Let,
                    swc_ast::VarDeclKind::Const => VarKind::Const,
                };
                for declarator in &var_decl.decls {
                    let name = match &declarator.name {
                        swc_ast::Pat::Ident(binding) => binding.id.sym.to_string(),
                        _ => {
                            return Err(self.error(
                                declarator.name.span(),
                                "destructuring patterns in for...in/for...of are not yet supported",
                            ));
                        }
                    };
                    // Pre-declared during pre-scan; mark initialised
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
            match ctx.kind {
                LabelKind::Loop | LabelKind::Switch | LabelKind::Block => return Ok(index),
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

        let value = if let Some(arg) = &return_stmt.arg {
            Some(self.lower_expr(arg, block)?)
        } else {
            None
        };

        let return_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(return_block)? {
            StmtFlow::Open(after_finally) => {
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

    fn lower_throw(
        &mut self,
        throw_stmt: &swc_ast::ThrowStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let value = self.lower_expr(&throw_stmt.arg, block)?;

        if let Some(try_ctx) = self.try_contexts.last() {
            if let Some(catch_entry) = try_ctx.catch_entry {
                let exc_var = try_ctx.exception_var.clone();
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: exc_var,
                        value,
                    },
                );
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
                self.current_function
                    .set_terminator(after_finally, Terminator::Throw { value });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
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
                        _ => {}
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
            let name = match &declarator.name {
                swc_ast::Pat::Ident(binding_ident) => binding_ident.id.sym.to_string(),
                _ => {
                    return Err(self.error(
                        declarator.name.span(),
                        "destructuring patterns are not yet supported",
                    ));
                }
            };

            let scope_id = self
                .scopes
                .resolve_scope_id(&name)
                .map_err(|msg| self.error(var_decl.span, msg))?;
            let ir_name = format!("${scope_id}.{name}");

            if let Some(init) = &declarator.init {
                let value = self.lower_expr(init, block)?;
                self.scopes
                    .mark_initialised(&name)
                    .map_err(|msg| self.error(var_decl.span, msg))?;

                // Determine where to place the StoreVar.
                // If lower_expr terminated the block (e.g. ternary/short-circuit),
                // find the merge block and place the StoreVar there instead.
                let store_block = self.resolve_store_block(block);

                self.current_function.append_instruction(
                    store_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value,
                    },
                );
                // If the block was terminated, the open block is the merge block.
                // Return Open(store_block) so subsequent declarations go to the right place.
                let flow_block = self.resolve_open_after_expr(block, store_block);
                return Ok(StmtFlow::Open(flow_block));
            } else {
                if matches!(kind, VarKind::Const) {
                    return Err(self.error(var_decl.span, "const declarations must be initialised"));
                }
                if matches!(kind, VarKind::Var) {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(var_decl.span, msg))?;
                    continue;
                }

                // `let x;` — initialise with undefined at its declaration point.
                let undef = self.module.add_constant(Constant::Undefined);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest,
                        constant: undef,
                    },
                );
                self.scopes
                    .mark_initialised(&name)
                    .map_err(|msg| self.error(var_decl.span, msg))?;
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: dest,
                    },
                );
            }
        }

        Ok(StmtFlow::Open(block))
    }

    fn lower_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        self.push_function_context(&name, BasicBlockId(0));

        // Register $this so that this-keyword expressions resolve.
        let _ = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        // Register parameters in the root scope (already a function scope).

        let param_names: Vec<String> = fn_decl
            .function
            .params
            .iter()
            .filter_map(|param| match &param.pat {
                swc_ast::Pat::Ident(binding_ident) => Some(binding_ident.id.sym.to_string()),
                _ => None,
            })
            .collect();

        for param_name in &param_names {
            let _ = self
                .scopes
                .declare(param_name, VarKind::Let, true)
                .map_err(|msg| self.error(fn_decl.span(), msg))?;
        }

        // Predeclare hoisted vars in the function body.
        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        // Lower the function body.
        let mut inner_flow = StmtFlow::Open(entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
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
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_params(param_names);
        for block in blocks {
            ir_function.push_block(block);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore the outer function context.
        self.pop_function_context();

        // Emit StoreVar in the outer function to bind the function reference.
        let outer_block = self.ensure_open(flow)?;
        let dest = self.alloc_value();
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest,
                constant: func_ref_const,
            },
        );
        let (scope_id, _) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let ir_name = format!("${scope_id}.{name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: dest,
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
        let name = fn_expr.ident.as_ref().map_or_else(
            || format!("anon_{}", self.module.functions().len()),
            |ident| ident.sym.to_string(),
        );
        self.push_function_context(&name, BasicBlockId(0));

        // Register $this so that this-keyword expressions resolve.
        let _ = self
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

        // Register parameters in function scope.
        let param_names: Vec<String> = fn_expr
            .function
            .params
            .iter()
            .filter_map(|param| match &param.pat {
                swc_ast::Pat::Ident(binding_ident) => Some(binding_ident.id.sym.to_string()),
                _ => None,
            })
            .collect();

        for param_name in &param_names {
            let _ = self
                .scopes
                .declare(param_name, VarKind::Let, true)
                .map_err(|msg| self.error(fn_expr.span(), msg))?;
        }

        // Predeclare hoisted vars in body.
        if let Some(body) = &fn_expr.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        // Lower body.
        let mut inner_flow = StmtFlow::Open(entry);
        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
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
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_params(param_names);
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore outer context.
        self.pop_function_context();

        // Emit FunctionRef constant in the current (outer) block.
        let dest = self.alloc_value();
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: func_ref_const,
            },
        );
        Ok(dest)
    }

    /// Lower an arrow function expression `(params) => expr` or `(params) => { ... }`.
    fn lower_arrow_expr(
        &mut self,
        arrow: &swc_ast::ArrowExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = format!("arrow_{}", self.module.functions().len());
        self.push_function_context(&name, BasicBlockId(0));

        // Register $this so that this-keyword expressions resolve.
        let _ = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let param_names: Vec<String> = arrow
            .params
            .iter()
            .filter_map(|param| {
                // param: &Box<Pat>, auto-deref to &Pat via Box<Pat>: Deref
                let p: &swc_ast::Pat = param;
                match p {
                    swc_ast::Pat::Ident(binding_ident) => Some(binding_ident.id.sym.to_string()),
                    _ => None,
                }
            })
            .collect();
        for param_name in &param_names {
            let _ = self
                .scopes
                .declare(param_name, VarKind::Let, true)
                .map_err(|msg| self.error(arrow.span, msg))?;
        }

        let entry = BasicBlockId(0);
        let mut inner_flow = StmtFlow::Open(entry);

        match arrow.body.as_ref() {
            swc_ast::BlockStmtOrExpr::BlockStmt(block_stmt) => {
                // Predeclare and lower block body.
                self.predeclare_block_stmts(&block_stmt.stmts)?;
                self.emit_hoisted_var_initializers(entry);
                for stmt in &block_stmt.stmts {
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
            swc_ast::BlockStmtOrExpr::Expr(expr) => {
                // Expression body: lower expr, then return it.
                self.emit_hoisted_var_initializers(entry);
                let val = self.lower_expr(expr, entry)?;
                self.current_function
                    .set_terminator(entry, Terminator::Return { value: Some(val) });
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
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_params(param_names);
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore outer context.
        self.pop_function_context();

        // Emit FunctionRef constant in the outer block.
        let dest = self.alloc_value();
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: func_ref_const,
            },
        );
        Ok(dest)
    }

    fn lower_class_decl(
        &mut self,
        class_decl: &swc_ast::ClassDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let class_name = class_decl.ident.sym.to_string();

        // Find the constructor.
        let constructor = class_decl
            .class
            .body
            .iter()
            .find_map(|member| match member {
                swc_ast::ClassMember::Constructor(c) => Some(c),
                _ => None,
            });

        // Create the constructor function.
        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));

        // Register $this as the first param.
        let _ = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_decl.span(), msg))?;

        // Register explicit constructor params.
        let mut param_names = vec!["$this".to_string()];
        if let Some(ctor) = constructor {
            for param in &ctor.params {
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        self.scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_decl.span(), msg))?;
                        param_names.push(name);
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

        // Lower constructor body.
        let mut inner_flow = StmtFlow::Open(entry);
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
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
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_params(param_names);
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

        // For each Method member (non-constructor), create a function and set on prototype.
        for member in &class_decl.class.body {
            if let swc_ast::ClassMember::Method(method) = member {
                if !matches!(method.kind, swc_ast::MethodKind::Method) {
                    continue;
                }

                let method_name = match &method.key {
                    swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                    swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                    _ => continue,
                };

                // Create method function.
                let fn_name = format!("{}.{}", class_name, method_name);
                self.push_function_context(&fn_name, BasicBlockId(0));

                // Register $this as the first param.
                let _ = self
                    .scopes
                    .declare("$this", VarKind::Let, true)
                    .map_err(|msg| self.error(method.span, msg))?;

                let mut method_param_names = vec!["$this".to_string()];
                for param in &method.function.params {
                    if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                        let name = binding_ident.id.sym.to_string();
                        self.scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        method_param_names.push(name);
                    }
                }

                // Predeclare hoisted vars in method body.
                if let Some(body) = &method.function.body {
                    self.predeclare_block_stmts(&body.stmts)?;
                }

                let m_entry = BasicBlockId(0);
                self.emit_hoisted_var_initializers(m_entry);

                // Lower method body.
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

                // Finalize method function.
                let m_old_fn = std::mem::replace(
                    &mut self.current_function,
                    FunctionBuilder::new("", BasicBlockId(0)),
                );
                let m_blocks = m_old_fn.into_blocks();
                let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                m_ir_function.set_params(method_param_names);
                for b in m_blocks {
                    m_ir_function.push_block(b);
                }
                let m_function_id = self.module.push_function(m_ir_function);

                self.pop_function_context();

                // Create FunctionRef for method.
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

                // Set method on prototype.
                let m_key_const = self.module.add_constant(Constant::String(method_name));
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
                        object: proto_dest,
                        key: m_key_dest,
                        value: m_dest,
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

        // 创建构造函数
        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));

        let _ = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_expr.span(), msg))?;

        let mut param_names = vec!["$this".to_string()];
        if let Some(ctor) = constructor {
            for param in &ctor.params {
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        self.scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_expr.span(), msg))?;
                        param_names.push(name);
                    }
                }
            }

            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut inner_flow = StmtFlow::Open(entry);
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
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
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_params(param_names);
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

        // Methods
        for member in &class_expr.class.body {
            if let swc_ast::ClassMember::Method(method) = member {
                if !matches!(method.kind, swc_ast::MethodKind::Method) {
                    continue;
                }

                let method_name = match &method.key {
                    swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                    swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                    _ => continue,
                };

                let fn_name = format!("{}.{}", class_name, method_name);
                self.push_function_context(&fn_name, BasicBlockId(0));

                let _ = self
                    .scopes
                    .declare("$this", VarKind::Let, true)
                    .map_err(|msg| self.error(method.span, msg))?;

                let mut method_param_names = vec!["$this".to_string()];
                for param in &method.function.params {
                    if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                        let name = binding_ident.id.sym.to_string();
                        self.scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        method_param_names.push(name);
                    }
                }

                if let Some(body) = &method.function.body {
                    self.predeclare_block_stmts(&body.stmts)?;
                }

                let m_entry = BasicBlockId(0);
                self.emit_hoisted_var_initializers(m_entry);

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
                let m_blocks = m_old_fn.into_blocks();
                let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                m_ir_function.set_params(method_param_names);
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
                        object: proto_dest,
                        key: m_key_dest,
                        value: m_dest,
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
            swc_ast::Expr::Member(member) => self.lower_member_expr(member, block),
            swc_ast::Expr::This(_) => self.lower_this(block),
            swc_ast::Expr::New(new_expr) => self.lower_new_expr(new_expr, block),
            swc_ast::Expr::Class(class_expr) => self.lower_class_expr(class_expr, block),
            swc_ast::Expr::Update(update) => self.lower_update(update, block),
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
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
                        let key_str = match &kv.key {
                            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                            _ => {
                                return Err(
                                    self.error(kv.key.span(), "unsupported property key kind")
                                );
                            }
                        };
                        let key_const = self.module.add_constant(Constant::String(key_str));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        let val_dest = self.lower_expr(&kv.value, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val_dest,
                            },
                        );
                    }
                    swc_ast::Prop::Shorthand(ident) => {
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
                        let val_dest = self.lower_ident(ident, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val_dest,
                            },
                        );
                    }
                    _ => {
                        return Err(
                            self.error(prop.span(), "unsupported property kind in object literal")
                        );
                    }
                },
                swc_ast::PropOrSpread::Spread(_) => {
                    return Err(self.error(
                        prop.span(),
                        "spread in object literals is not yet supported",
                    ));
                }
            }
        }

        Ok(obj_dest)
    }

    fn lower_member_expr(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
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
            _ => return Err(self.error(member.span, "unsupported member property kind")),
        };

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object: obj_val,
                key,
            },
        );
        Ok(dest)
    }

    fn lower_this(&mut self, block: BasicBlockId) -> Result<ValueId, LoweringError> {
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

    fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
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

        // Get prototype from constructor.
        let proto_key_const = self
            .module
            .add_constant(Constant::String("prototype".to_string()));
        let proto_key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: proto_key,
                constant: proto_key_const,
            },
        );
        let proto_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: proto_val,
                object: callee_val,
                key: proto_key,
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

    // ── Identifiers ─────────────────────────────────────────────────────────

    fn lower_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let callee_val: ValueId;
        let this_val: ValueId;

        match &call.callee {
            swc_ast::Callee::Expr(expr) => {
                // 检测 MemberExpr 被调用者 → 提取 obj 作为 this
                if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
                    // obj.method() → obj 是 this，method 是 callee
                    // 检测 Object.defineProperty
                    if let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref() {
                        if &*obj_ident.sym == "Object" {
                            if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                                let builtin = match &*prop_ident.sym {
                                    "defineProperty" => Some(Builtin::DefineProperty),
                                    "getOwnPropertyDescriptor" => Some(Builtin::GetOwnPropDesc),
                                    _ => None,
                                };
                                if let Some(builtin) = builtin {
                                    // 性能优化：预分配容量避免循环中多次 reallocation
                                    let mut args = Vec::with_capacity(call.args.len());
                                    for arg in &call.args {
                                        let arg_val = self.lower_expr(&arg.expr, block)?;
                                        args.push(arg_val);
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
                            }
                        }
                    }
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
            _ => return Err(self.error(call.span, "unsupported callee type")),
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

    fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (scope_id, _kind) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(ident.span, msg))?;
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
                    _ => {
                        return Err(self.error(
                            assign.span,
                            "unsupported member property in assignment target",
                        ));
                    }
                };

                if assign.op == swc_ast::AssignOp::Assign {
                    // 简单赋值: obj.x = value
                    let value_val = self.lower_expr(assign.right.as_ref(), block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_val,
                            key,
                            value: value_val,
                        },
                    );
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

                // GetProp 读取当前值
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: loaded,
                        object: obj_val,
                        key,
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
                    Instruction::SetProp {
                        object: obj_val,
                        key,
                        value: dest,
                    },
                );

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
            _ => {
                return Err(self.error(
                    assign.left.span(),
                    "only simple identifier assignment targets are supported",
                ));
            }
        };

        // 性能优化：使用 lookup_for_assign 一次遍历完成 const 检查 + TDZ 检查 + scope 解析，
        // 避免 check_mutable and lookup 各自遍历 scope chain 的冗余。
        let (scope_id, _kind) = self
            .scopes
            .lookup_for_assign(&name)
            .map_err(|msg| self.error(assign.span, msg))?;
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

                Ok(dest)
            }
        }
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
    fn lower_comparison(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
        let rhs = self.lower_expr(bin.right.as_ref(), block)?;
        let dest = self.alloc_value();

        let op = match bin.op {
            swc_ast::BinaryOp::EqEq => CompareOp::Eq,
            swc_ast::BinaryOp::NotEq => CompareOp::NotEq,
            swc_ast::BinaryOp::EqEqEq => CompareOp::StrictEq,
            swc_ast::BinaryOp::NotEqEq => CompareOp::StrictNotEq,
            swc_ast::BinaryOp::Lt => CompareOp::Lt,
            swc_ast::BinaryOp::LtEq => CompareOp::LtEq,
            swc_ast::BinaryOp::Gt => CompareOp::Gt,
            swc_ast::BinaryOp::GtEq => CompareOp::GtEq,
            _ => unreachable!(),
        };

        self.current_function
            .append_instruction(block, Instruction::Compare { dest, op, lhs, rhs });
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

    fn predeclare_stmts(&mut self, stmts: &[swc_ast::ModuleItem]) -> Result<(), LoweringError> {
        for item in stmts {
            let swc_ast::ModuleItem::Stmt(stmt) = item else {
                continue;
            };
            self.predeclare_stmt_with_mode(stmt, LexicalMode::Include)?;
        }
        Ok(())
    }

    fn predeclare_block_stmts(&mut self, stmts: &[swc_ast::Stmt]) -> Result<(), LoweringError> {
        for stmt in stmts {
            self.predeclare_stmt_with_mode(stmt, LexicalMode::Include)?;
        }
        Ok(())
    }

    fn predeclare_stmt_with_mode(
        &mut self,
        stmt: &swc_ast::Stmt,
        mode: LexicalMode,
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
                        let name = match &declarator.name {
                            swc_ast::Pat::Ident(binding_ident) => binding_ident.id.sym.to_string(),
                            _ => continue,
                        };
                        if !matches!(kind, VarKind::Var) && matches!(mode, LexicalMode::Exclude) {
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
                _ => {}
            },
            swc_ast::Stmt::Block(block_stmt) => {
                for stmt in &block_stmt.stmts {
                    self.predeclare_stmt_with_mode(stmt, LexicalMode::Exclude)?;
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
                self.predeclare_stmt_with_mode(&for_stmt.body, LexicalMode::Exclude)?;
            }
            swc_ast::Stmt::ForIn(for_in) => {
                // Pre-declare the loop variable if it's a var declaration
                match &for_in.left {
                    swc_ast::ForHead::VarDecl(var_decl) => {
                        self.predeclare_var_decl(var_decl)?;
                    }
                    _ => {}
                }
                self.predeclare_stmt_with_mode(&for_in.body, LexicalMode::Exclude)?;
            }
            swc_ast::Stmt::ForOf(for_of) => {
                match &for_of.left {
                    swc_ast::ForHead::VarDecl(var_decl) => {
                        self.predeclare_var_decl(var_decl)?;
                    }
                    _ => {}
                }
                self.predeclare_stmt_with_mode(&for_of.body, LexicalMode::Exclude)?;
            }
            swc_ast::Stmt::Labeled(labeled) => {
                self.predeclare_stmt_with_mode(&labeled.body, mode)?;
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
            let name = match &declarator.name {
                swc_ast::Pat::Ident(binding_ident) => binding_ident.id.sym.to_string(),
                _ => continue,
            };
            let declared = matches!(kind, VarKind::Var);
            let scope_id = self
                .scopes
                .declare(&name, kind, declared)
                .map_err(|msg| self.error(var_decl.span, msg))?;
            if matches!(kind, VarKind::Var) {
                self.record_hoisted_var(scope_id, name);
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

fn is_console_log(call: &swc_ast::CallExpr) -> bool {
    let swc_ast::Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let swc_ast::Expr::Member(member) = callee_expr.as_ref() else {
        return false;
    };
    let swc_ast::Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    let swc_ast::MemberProp::Ident(property) = &member.prop else {
        return false;
    };

    object.sym == "console" && property.sym == "log"
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
