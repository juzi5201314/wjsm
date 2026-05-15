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
    pub(crate) fn lower_stmt(
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

    pub(crate) fn lower_expr_stmt(
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

    pub(crate) fn lower_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let _ = self.lower_call_expr(call, block)?;
        Ok(self.resolve_store_block(block))
    }

    // ── Blocks ──────────────────────────────────────────────────────────────

    pub(crate) fn lower_block_stmt(
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

    pub(crate) fn lower_if(
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

    pub(crate) fn merge_eval_completion_after_if(
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

    pub(crate) fn lower_while(
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

    pub(crate) fn lower_do_while(
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

    pub(crate) fn lower_for(
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

    pub(crate) fn lower_for_in(
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

    pub(crate) fn lower_for_of(
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

    pub(crate) fn lower_for_await_of(
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
            break_target: exit,
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

        Ok(StmtFlow::Open(exit))
    }

    /// Lower the LHS of a for...in or for...of loop.
    /// Supports: simple identifier, or var declaration with single binding identifier.
    pub(crate) fn lower_for_in_of_lhs(
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
                        Err(self.error(
                            pat.span(),
                            "destructuring patterns in for...in/for...of are not yet supported",
                        ))
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

    pub(crate) fn lower_break(
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
        let mut iterator_cleanups = self.iterator_cleanups_crossing(target_index);
        if let Some(iter) = self.label_stack[target_index].iterator_to_close {
            iterator_cleanups.push(iter);
        }

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

    pub(crate) fn lower_continue(
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

    pub(crate) fn find_nearest_break_context_index(&self, span: Span) -> Result<usize, LoweringError> {
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

    pub(crate) fn find_nearest_continue_context_index(&self, span: Span) -> Result<usize, LoweringError> {
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

    pub(crate) fn find_label_context_index(
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

    pub(crate) fn iterator_cleanups_crossing(&self, target_index: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(target_index + 1)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    pub(crate) fn iterator_cleanups_from_depth(&self, depth: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(depth)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    pub(crate) fn active_iterator_cleanups(&self) -> Vec<ValueId> {
        self.iterator_cleanups_from_depth(0)
    }

    pub(crate) fn emit_iterator_closes(&mut self, block: BasicBlockId, iterators: &[ValueId]) {
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

    pub(crate) fn lower_labeled(
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

    pub(crate) fn lower_return(
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

    pub(crate) fn lower_switch(
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
    pub(crate) fn lower_binary_op_with_const(
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

    pub(crate) fn extract_constant_from_expr(
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

    pub(crate) fn emit_async_reject(&mut self, block: BasicBlockId, reason: ValueId) {
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

    pub(crate) fn emit_throw_value(
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

    pub(crate) fn lower_throw(
        &mut self,
        throw_stmt: &swc_ast::ThrowStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let value = self.lower_expr(&throw_stmt.arg, block)?;
        self.emit_throw_value(block, value)
    }

    // ── try / catch / finally ───────────────────────────────────────────────

    pub(crate) fn lower_try(
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
                // Lower catch clause: bind parameter if present
                self.scopes.push_scope(ScopeKind::Block);
                if let Some(param) = &catch.param {
                    match param {
                        swc_ast::Pat::Ident(binding) => {
                            let name = binding.id.sym.to_string();
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
                            for name in &names {
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
                        for name in &names {
                            self.scopes
                                .declare(name, VarKind::Let, true)
                                .map_err(|msg| self.error(param.span(), msg))?;
                        }
                        self.lower_destructure_pattern(param, exc_val, catch_entry, VarKind::Let)?;
                    }
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

    pub(crate) fn lower_block_body(
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

    pub(crate) fn lower_empty(&self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
        Ok(flow)
    }

    pub(crate) fn lower_debugger(&mut self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
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

    pub(crate) fn lower_with(
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
}
