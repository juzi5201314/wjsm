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

impl Lowerer {
    pub(crate) fn lower_unary(
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

    pub(crate) fn lower_update(
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

    pub(crate) fn lower_cond(
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

    pub(crate) fn lower_seq(
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

    pub(crate) fn lower_literal(
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

    pub(crate) fn load_bool_constant(&mut self, val: bool, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(val));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    // ── Flow helper ─────────────────────────────────────────────────────────

    pub(crate) fn ensure_open(&self, flow: StmtFlow) -> Result<BasicBlockId, LoweringError> {
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
    pub(crate) fn extract_pat_bindings(pats: &[swc_ast::Pat], result: &mut Vec<String>) {
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

    pub(crate) fn predeclare_stmts(&mut self, stmts: &[swc_ast::ModuleItem]) -> Result<(), LoweringError> {
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

    pub(crate) fn predeclare_block_stmts(&mut self, stmts: &[swc_ast::Stmt]) -> Result<(), LoweringError> {
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

    pub(crate) fn predeclare_stmt_with_mode_and_eval_strings(
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
                match &for_in.left {
                    swc_ast::ForHead::VarDecl(var_decl) => {
                        self.predeclare_var_decl(var_decl)?;
                    }
                    swc_ast::ForHead::Pat(pat) => {
                        if !matches!(**pat, swc_ast::Pat::Ident(_)) {
                            return Err(self.error(
                                pat.span(),
                                "destructuring patterns in for...in/for...of are not yet supported",
                            ));
                        }
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
                    swc_ast::ForHead::Pat(pat) => {
                        if !matches!(**pat, swc_ast::Pat::Ident(_)) {
                            return Err(self.error(
                                pat.span(),
                                "destructuring patterns in for...in/for...of are not yet supported",
                            ));
                        }
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
            swc_ast::Stmt::Try(try_stmt) => {
                for stmt in &try_stmt.block.stmts {
                    self.predeclare_stmt_with_mode_and_eval_strings(
                        stmt,
                        LexicalMode::Exclude,
                        eval_string_bindings,
                    )?;
                }
                if let Some(catch) = &try_stmt.handler {
                    for stmt in &catch.body.stmts {
                        self.predeclare_stmt_with_mode_and_eval_strings(
                            stmt,
                            LexicalMode::Exclude,
                            eval_string_bindings,
                        )?;
                    }
                }
                if let Some(finally) = &try_stmt.finalizer {
                    for stmt in &finally.stmts {
                        self.predeclare_stmt_with_mode_and_eval_strings(
                            stmt,
                            LexicalMode::Exclude,
                            eval_string_bindings,
                        )?;
                    }
                }
            }

            _ => {}
        }
        Ok(())
    }

    pub(crate) fn predeclare_var_decl(&mut self, var_decl: &swc_ast::VarDecl) -> Result<(), LoweringError> {
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

    pub(crate) fn record_hoisted_var(&mut self, scope_id: usize, name: String) {
        if !self.hoisted_vars_set.insert((scope_id, name.clone())) {
            return;
        }
        self.hoisted_vars.push(HoistedVar { scope_id, name });
    }

    pub(crate) fn emit_hoisted_var_initializers(&mut self, block: BasicBlockId) {
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

    pub(crate) fn alloc_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    pub(crate) fn alloc_temp_name(&mut self) -> String {
        let name = format!("$tmp.{}", self.next_temp);
        self.next_temp += 1;
        name
    }

    /// Check if a block has been terminated by lower_expr (e.g. ternary/short-circuit).
    /// If so, find the merge block where subsequent instructions should go.
    /// Check if a block has been terminated by lower_expr (e.g. ternary/short-circuit).
    /// If so, find the merge block where subsequent instructions should go.
    pub(crate) fn resolve_store_block(&self, block: BasicBlockId) -> BasicBlockId {
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
    pub(crate) fn resolve_open_after_expr(
        &self,
        _original_block: BasicBlockId,
        store_block: BasicBlockId,
    ) -> BasicBlockId {
        store_block
    }
    pub(crate) fn lower_pending_finalizers(&mut self, block: BasicBlockId) -> Result<StmtFlow, LoweringError> {
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

    pub(crate) fn error(&self, span: Span, message: impl Into<String>) -> LoweringError {
        LoweringError::Diagnostic(Diagnostic::new(span.lo.0, span.hi.0, message))
    }
}
