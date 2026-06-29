use super::*;

impl Lowerer {
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

    pub(crate) fn predeclare_stmts(
        &mut self,
        stmts: &[swc_ast::ModuleItem],
    ) -> Result<(), LoweringError> {
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

    pub(crate) fn predeclare_block_stmts(
        &mut self,
        stmts: &[swc_ast::Stmt],
    ) -> Result<(), LoweringError> {
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
                        Self::extract_pat_bindings(
                            std::slice::from_ref(&declarator.name),
                            &mut names,
                        );
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
                        .declare(&name, VarKind::Let, false)
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
                        Self::extract_pat_bindings(
                            std::slice::from_ref(&declarator.name),
                            &mut names,
                        );
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
                if let Some(init) = &for_stmt.init
                    && let swc_ast::VarDeclOrExpr::VarDecl(var_decl) = init
                {
                    self.predeclare_var_decl(var_decl)?;
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
                if let swc_ast::ForHead::VarDecl(var_decl) = &for_in.left {
                    self.predeclare_var_decl(var_decl)?;
                }
                self.predeclare_stmt_with_mode_and_eval_strings(
                    &for_in.body,
                    LexicalMode::Exclude,
                    eval_string_bindings,
                )?;
            }
            swc_ast::Stmt::ForOf(for_of) => {
                if let swc_ast::ForHead::VarDecl(var_decl) = &for_of.left {
                    self.predeclare_var_decl(var_decl)?;
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
                        if name == "arguments" && self.scopes.lookup("arguments").is_ok() {
                            continue;
                        }
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
                    if let Some(param) = &catch.param {
                        self.scopes.push_scope(ScopeKind::Block);
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(std::slice::from_ref(param), &mut names);
                        for name in names {
                            let _scope_id = self
                                .scopes
                                .declare(&name, VarKind::Let, false)
                                .map_err(|msg| self.error(param.span(), msg))?;
                        }
                        for stmt in &catch.body.stmts {
                            self.predeclare_stmt_with_mode_and_eval_strings(
                                stmt,
                                LexicalMode::Exclude,
                                eval_string_bindings,
                            )?;
                        }
                        self.scopes.pop_scope();
                    } else {
                        self.scopes.push_scope(ScopeKind::Block);
                        for stmt in &catch.body.stmts {
                            self.predeclare_stmt_with_mode_and_eval_strings(
                                stmt,
                                LexicalMode::Exclude,
                                eval_string_bindings,
                            )?;
                        }
                        self.scopes.pop_scope();
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

    pub(crate) fn predeclare_var_decl(
        &mut self,
        var_decl: &swc_ast::VarDecl,
    ) -> Result<(), LoweringError> {
        let kind = match var_decl.kind {
            swc_ast::VarDeclKind::Var => VarKind::Var,
            swc_ast::VarDeclKind::Let => VarKind::Let,
            swc_ast::VarDeclKind::Const => VarKind::Const,
        };
        for declarator in &var_decl.decls {
            let mut names = Vec::new();
            Self::extract_pat_bindings(std::slice::from_ref(&declarator.name), &mut names);
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
    /// Check if a block has been terminated by lower_expr (e.g. ternary/short-circuit).
    /// If so, find the merge block where subsequent instructions should go.
    /// Also checks for eval_continue_block set by lower_direct_eval_call.
    pub(crate) fn resolve_store_block(&mut self, block: BasicBlockId) -> BasicBlockId {
        // eval 异常检查分叉后，后续代码应插入到 continue block
        // new_expr (WeakRef/FR constructor) 异常检查分叉后，后续代码应插入到 continue block
        if let Some(cont) = self.new_expr_continue_block.take() {
            return cont;
        }

        if let Some(cont) = self.await_continue_block.take() {
            return cont;
        }

        if let Some(cont) = self.eval_continue_block.take() {
            return cont;
        }

        if let Some(cont) = self.expr_merge_block.take() {
            return cont;
        }

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
                        .is_some_and(|candidate| {
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
                        .is_some_and(|candidate| {
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
    pub(crate) fn lower_pending_finalizers(
        &mut self,
        block: BasicBlockId,
    ) -> Result<StmtFlow, LoweringError> {
        self.lower_pending_finalizers_after(block, 0)
    }

    pub(crate) fn lower_pending_finalizers_after(
        &mut self,
        block: BasicBlockId,
        keep_len: usize,
    ) -> Result<StmtFlow, LoweringError> {
        let saved = self.active_finalizers.clone();
        let keep_len = keep_len.min(saved.len());
        if saved.len() <= keep_len {
            return Ok(StmtFlow::Open(block));
        }

        // abrupt completion 会按从内到外的顺序执行 finally。
        // 降低某个 finally 时，只把“更外层”的 finalizer 保留在 active 栈里，
        // 这样 finally 内部的 return/throw/break/continue 能继续展开剩余外层 finally，
        // 而不是因为当前批量展开把 active_finalizers 清空而跳过它们。
        let mut pending = saved[keep_len..].to_vec();
        let mut flow = StmtFlow::Open(block);

        while let Some(finalizer) = pending.pop() {
            let mut active = saved[..keep_len].to_vec();
            active.extend(pending.iter().cloned());
            self.active_finalizers = active;
            flow = self.lower_block_body(&finalizer.block, flow)?;
            if matches!(flow, StmtFlow::Terminated) {
                break;
            }
        }

        self.active_finalizers = saved;
        Ok(flow)
    }

    pub(crate) fn error(&self, span: Span, message: impl Into<String>) -> LoweringError {
        LoweringError::Diagnostic(Diagnostic::with_source_context(
            span.lo.0,
            span.hi.0,
            message,
            self.diagnostic_source.clone(),
            self.diagnostic_filename.clone(),
        ))
    }
}
