use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, Instruction, Module, Program,
    Terminator, ValueId,
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
        // Walk up the scope chain to find the variable.
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
    ///
    /// Errors: undeclared variable, or TDZ access.
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
    /// Used during declaration lowering where we know the var is being initialised.
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
        }
    }

    fn lower_module(mut self, module: &swc_ast::Module) -> Result<Program, LoweringError> {
        let mut function = wjsm_ir::Function::new("main", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0), Terminator::Return { value: None });

        // Pre-scan: hoist variable declarations so let/const are in TDZ.
        self.predeclare_stmts(&module.body)?;
        self.emit_hoisted_var_initializers(&mut entry);

        for item in &module.body {
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => self.lower_stmt(stmt, &mut entry)?,
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

        function.push_block(entry);
        self.module.push_function(function);
        Ok(self.module)
    }

    fn lower_stmt(
        &mut self,
        stmt: &swc_ast::Stmt,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => {
                match expr_stmt.expr.as_ref() {
                    swc_ast::Expr::Call(call) => self.lower_console_log_stmt(call, block)?,
                    expr => {
                        let _ = self.lower_expr(expr, block)?;
                    }
                }
                Ok(())
            }
            swc_ast::Stmt::Decl(decl) => match decl {
                swc_ast::Decl::Var(var_decl) => self.lower_var_decl(var_decl, block),
                _ => Err(self.error(
                    stmt.span(),
                    format!("unsupported declaration kind `{}`", decl_kind(decl)),
                )),
            },
            swc_ast::Stmt::Block(block_stmt) => self.lower_block_stmt(block_stmt, block),
            _ => Err(self.error(
                stmt.span(),
                format!("unsupported statement kind `{}`", stmt_kind(stmt)),
            )),
        }
    }

    /// Lower `{ ... }` — creates a new block scope, lowers stmts, then pops.
    fn lower_block_stmt(
        &mut self,
        block_stmt: &swc_ast::BlockStmt,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
        self.scopes.push_scope(ScopeKind::Block);
        self.predeclare_block_stmts(&block_stmt.stmts)?;
        for stmt in &block_stmt.stmts {
            self.lower_stmt(stmt, block)?;
        }
        self.scopes.pop_scope();
        Ok(())
    }

    /// Pre-scan statements to hoist variable declarations.
    ///
    /// - `let`/`const` are pre-declared in the current scope (TDZ).
    /// - `var` is pre-declared in the nearest function scope (initialised).
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
                            _ => continue, // destructuring not handled in pre-scan
                        };
                        if !matches!(kind, VarKind::Var) && matches!(mode, LexicalMode::Exclude) {
                            continue;
                        }

                        // var is hoisted as initialised; let/const enter TDZ.
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
            _ => {}
        }
        Ok(())
    }

    fn record_hoisted_var(&mut self, scope_id: usize, name: String) {
        // 使用 HashSet 进行 O(1) 重复检测。
        if !self.hoisted_vars_set.insert((scope_id, name.clone())) {
            return;
        }

        self.hoisted_vars.push(HoistedVar { scope_id, name });
    }

    fn emit_hoisted_var_initializers(&mut self, block: &mut BasicBlock) {
        if self.hoisted_vars.is_empty() {
            return;
        }

        let undef = self.module.add_constant(Constant::Undefined);
        let value = self.alloc_value();
        block.push_instruction(Instruction::Const {
            dest: value,
            constant: undef,
        });

        for var in &self.hoisted_vars {
            let name = format!("${}.{}", var.scope_id, var.name);
            block.push_instruction(Instruction::StoreVar { name, value });
        }
    }

    /// Lower `var x = ...` / `let x = ...` / `const x = ...`.
    fn lower_var_decl(
        &mut self,
        var_decl: &swc_ast::VarDecl,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
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

            // Variable is already pre-declared; just get the scope-qualified name.
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
                block.push_instruction(Instruction::StoreVar {
                    name: ir_name,
                    value,
                });
            } else {
                if matches!(kind, VarKind::Const) {
                    // SWC should reject `const x;` at parse time, but guard anyway.
                    return Err(self.error(var_decl.span, "const declarations must be initialised"));
                }
                if matches!(kind, VarKind::Var) {
                    // var 在预扫描时已标记为 initialised，此处调用是 no-op。
                    // 但保持代码一致性，仍然调用 mark_initialised。
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(var_decl.span, msg))?;
                    continue;
                }

                // `let x;` — initialise with undefined at its declaration point.
                let undef = self.module.add_constant(Constant::Undefined);
                let dest = self.alloc_value();
                block.push_instruction(Instruction::Const {
                    dest,
                    constant: undef,
                });
                self.scopes
                    .mark_initialised(&name)
                    .map_err(|msg| self.error(var_decl.span, msg))?;
                block.push_instruction(Instruction::StoreVar {
                    name: ir_name,
                    value: dest,
                });
            }
        }

        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        match expr {
            swc_ast::Expr::Bin(bin) => self.lower_binary(bin, block),
            swc_ast::Expr::Lit(lit) => self.lower_literal(lit, block),
            swc_ast::Expr::Ident(ident) => self.lower_ident(ident, block),
            swc_ast::Expr::Assign(assign) => self.lower_assign(assign, block),
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    /// Lower a variable reference: `x` → `LoadVar { dest, name: "x" }`.
    fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (scope_id, _kind) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(ident.span, msg))?;
        let ir_name = format!("${scope_id}.{name}");

        let dest = self.alloc_value();
        block.push_instruction(Instruction::LoadVar {
            dest,
            name: ir_name,
        });
        Ok(dest)
    }

    /// Lower assignment: `x = expr` → `StoreVar { name: "x", value }`.
    /// Compound: `x += expr` → load x, compute, store x.
    fn lower_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: &mut BasicBlock,
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
                // Plain assignment: x = rhs
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                block.push_instruction(Instruction::StoreVar {
                    name: ir_name,
                    value: rhs,
                });
                Ok(rhs)
            }
            op => {
                // 复合赋值：x += rhs
                //
                // ECMAScript 规范（EvaluateStringOrNumericBinaryExpression）要求：
                // 1. 先读取左操作数的旧值（LoadVar）
                // 2. 再求值右操作数（rhs）
                // 3. 执行二元运算
                // 4. 将结果写回左操作数（StoreVar）
                //
                // 这个顺序不可交换：如果先求值 rhs 再读取旧值，
                // 当 rhs 有副作用修改了 x 时语义会出错。
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                let loaded = self.alloc_value();
                block.push_instruction(Instruction::LoadVar {
                    dest: loaded,
                    name: ir_name.clone(),
                });

                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();
                block.push_instruction(Instruction::Binary {
                    dest,
                    op: bin_op,
                    lhs: loaded,
                    rhs,
                });

                block.push_instruction(Instruction::StoreVar {
                    name: ir_name,
                    value: dest,
                });

                Ok(dest)
            }
        }
    }

    fn lower_console_log_stmt(
        &mut self,
        call: &swc_ast::CallExpr,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
        if !is_console_log(call) {
            return Err(self.error(call.span(), "unsupported call expression"));
        }

        let first_arg = call
            .args
            .first()
            .ok_or_else(|| self.error(call.span(), "console.log requires at least 1 argument"))?;

        let value = self.lower_expr(first_arg.expr.as_ref(), block)?;
        block.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ConsoleLog,
            args: vec![value],
        });
        Ok(())
    }

    fn lower_binary(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
        let rhs = self.lower_expr(bin.right.as_ref(), block)?;
        let dest = self.alloc_value();
        let op = match bin.op {
            swc_ast::BinaryOp::Add => BinaryOp::Add,
            swc_ast::BinaryOp::Sub => BinaryOp::Sub,
            swc_ast::BinaryOp::Mul => BinaryOp::Mul,
            swc_ast::BinaryOp::Div => BinaryOp::Div,
            _ => {
                return Err(self.error(
                    bin.span(),
                    format!("unsupported binary operator `{}`", binary_op_name(bin.op)),
                ));
            }
        };

        block.push_instruction(Instruction::Binary { dest, op, lhs, rhs });
        Ok(dest)
    }

    fn lower_literal(
        &mut self,
        lit: &swc_ast::Lit,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        let constant = match lit {
            swc_ast::Lit::Num(num) => Constant::Number(num.value),
            swc_ast::Lit::Str(string) => {
                Constant::String(string.value.to_string_lossy().into_owned())
            }
            _ => {
                return Err(self.error(
                    lit.span(),
                    format!("unsupported literal kind `{}`", literal_kind(lit)),
                ));
            }
        };

        let constant = self.module.add_constant(constant);
        let dest = self.alloc_value();
        block.push_instruction(Instruction::Const { dest, constant });
        Ok(dest)
    }

    fn alloc_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn error(&self, span: Span, message: impl Into<String>) -> LoweringError {
        LoweringError::Diagnostic(Diagnostic::new(span.lo.0, span.hi.0, message))
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
            end,
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

// ── Kind strings ────────────────────────────────────────────────────────

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
