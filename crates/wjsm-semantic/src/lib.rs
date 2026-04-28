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

    /// Check that this variable is not `const` before reassignment.
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

    fn block_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.iter_mut().find(|b| b.id() == id)
    }

    fn block(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.iter().find(|b| b.id() == id)
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
        }
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
                let term = self
                    .current_function
                    .block(block)
                    .map(|b| b.terminator().clone());
                if let Some(Terminator::Unreachable) = term {
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
                swc_ast::Decl::Var(var_decl) => self.lower_var_decl(var_decl, flow),
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
                    return Err(self.error(call.span(), "unsupported call expression"));
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

        let target = if let Some(label) = &break_stmt.label {
            self.find_label(&label.sym.to_string(), Some(label.span))?
        } else {
            self.find_nearest_break_target()?
        };

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
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

        let target = if let Some(label) = &continue_stmt.label {
            let ctx = self.find_label_context(&label.sym.to_string())?;
            ctx.continue_target.ok_or_else(|| {
                self.error(
                    continue_stmt.span(),
                    format!("cannot continue to non-loop label `{}`", label.sym),
                )
            })?
        } else {
            self.find_nearest_continue_target()?
        };

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
                self.current_function
                    .set_terminator(after_finally, Terminator::Jump { target });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    fn find_nearest_break_target(&self) -> Result<BasicBlockId, LoweringError> {
        // We need a raw error span, so we pass an error constructed at the call site instead.
        for ctx in self.label_stack.iter().rev() {
            match ctx.kind {
                LabelKind::Loop | LabelKind::Switch | LabelKind::Block => {
                    return Ok(ctx.break_target);
                }
            }
        }
        // This error is caught by the caller
        Err(LoweringError::Diagnostic(Diagnostic::new(
            0,
            0,
            "break outside of loop or switch",
        )))
    }

    fn find_nearest_continue_target(&self) -> Result<BasicBlockId, LoweringError> {
        for ctx in self.label_stack.iter().rev() {
            if let Some(target) = ctx.continue_target {
                return Ok(target);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            0,
            0,
            "continue outside of loop",
        )))
    }

    fn find_label(
        &self,
        name: &str,
        _error_span: Option<Span>,
    ) -> Result<BasicBlockId, LoweringError> {
        for ctx in self.label_stack.iter().rev() {
            if ctx.label.as_deref() == Some(name) {
                return Ok(ctx.break_target);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            0,
            0,
            format!("unknown label `{name}`"),
        )))
    }

    fn find_label_context(&self, name: &str) -> Result<&LabelContext, LoweringError> {
        for ctx in self.label_stack.iter().rev() {
            if ctx.label.as_deref() == Some(name) {
                return Ok(ctx);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            0,
            0,
            format!("unknown label `{name}`"),
        )))
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
        let mut cases: Vec<SwitchCaseTarget> = Vec::new();
        let mut case_blocks: Vec<BasicBlockId> = Vec::new();
        let default_block = self.current_function.new_block();
        let mut found_default = false;

        // Generate a case block for each case
        for case in &switch_stmt.cases {
            if case.test.is_none() {
                // default case — we already allocated default_block
                found_default = true;
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

        // Set switch terminator at the discriminant block
        let default_target = if found_default { default_block } else { exit };

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

        // Default case body
        if found_default {
            let mut default_flow = StmtFlow::Open(default_block);
            // Find the default case in the switch
            for case in &switch_stmt.cases {
                if case.test.is_none() {
                    for stmt in &case.cons {
                        default_flow = self.lower_stmt(stmt, default_flow)?;
                    }
                    break;
                }
            }
            let _ = self
                .current_function
                .ensure_jump_or_terminated(default_flow, exit);
        }

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
            _ => Err(LoweringError::Diagnostic(Diagnostic::new(
                0,
                0,
                "switch case must be a literal",
            ))),
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
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    // ── Identifiers ─────────────────────────────────────────────────────────

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

        self.scopes
            .check_mutable(&name)
            .map_err(|msg| self.error(assign.span, msg))?;

        let (scope_id, _kind) = self
            .scopes
            .lookup(&name)
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
                self.current_function.append_instruction(
                    block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );

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
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ConsoleLog, // placeholder — will be proper bitwise builtins later
                        args: vec![lhs, rhs],
                    },
                );
                // For now, return an error
                Err(self.error(
                    bin.span(),
                    format!("unsupported binary operator `{}`", binary_op_name(bin.op)),
                ))
            }
            other => Err(self.error(
                bin.span(),
                format!("unsupported binary operator `{}`", binary_op_name(other)),
            )),
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
            TypeOf => Err(self.error(unary.span(), "typeof operator is not yet supported")),
            _ => Err(self.error(unary.span(), format!("unsupported unary operator"))),
        }
    }

    // ── Ternary conditional ─────────────────────────────────────────────────

    fn lower_cond(
        &mut self,
        cond: &swc_ast::CondExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
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

        let finalizers: Vec<_> = self.active_finalizers.iter().rev().cloned().collect();
        let saved = std::mem::take(&mut self.active_finalizers);
        let mut flow = StmtFlow::Open(block);
        for finalizer in &finalizers {
            flow = self.lower_block_body(finalizer, flow)?;
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

fn binary_op_name(op: swc_ast::BinaryOp) -> &'static str {
    match op {
        swc_ast::BinaryOp::EqEq => "==",
        swc_ast::BinaryOp::NotEq => "!=",
        swc_ast::BinaryOp::EqEqEq => "===",
        swc_ast::BinaryOp::NotEqEq => "!==",
        swc_ast::BinaryOp::Lt => "<",
        swc_ast::BinaryOp::LtEq => "<=",
        swc_ast::BinaryOp::Gt => ">",
        swc_ast::BinaryOp::GtEq => ">=",
        swc_ast::BinaryOp::LShift => "<<",
        swc_ast::BinaryOp::RShift => ">>",
        swc_ast::BinaryOp::ZeroFillRShift => ">>>",
        swc_ast::BinaryOp::Add => "+",
        swc_ast::BinaryOp::Sub => "-",
        swc_ast::BinaryOp::Mul => "*",
        swc_ast::BinaryOp::Div => "/",
        swc_ast::BinaryOp::Mod => "%",
        swc_ast::BinaryOp::BitOr => "|",
        swc_ast::BinaryOp::BitXor => "^",
        swc_ast::BinaryOp::BitAnd => "&",
        swc_ast::BinaryOp::LogicalOr => "||",
        swc_ast::BinaryOp::LogicalAnd => "&&",
        swc_ast::BinaryOp::In => "in",
        swc_ast::BinaryOp::InstanceOf => "instanceof",
        swc_ast::BinaryOp::Exp => "**",
        swc_ast::BinaryOp::NullishCoalescing => "??",
    }
}
