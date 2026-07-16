use super::*;

mod async_fn_decls;
mod async_gen_fn_decls;
mod fn_decls;
mod gen_fn_decls;

impl Lowerer {
    /// 构建 async/async-generator 函数的状态分发 switch
    fn emit_async_dispatch_switch(
        &mut self,
        state_scope_id: usize,
        dispatch_block: BasicBlockId,
        body_entry: BasicBlockId,
    ) {
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
    }

    /// 将 wrapper 函数存储到外层作用域（处理闭包创建）
    fn store_wrapper_in_outer_scope(
        &mut self,
        flow: StmtFlow,
        name: &str,
        wrapper_fn_id: FunctionId,
        captured: &[CapturedBinding],
        span: swc_core::common::Span,
    ) -> Result<StmtFlow, LoweringError> {
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

        let mut store_block = outer_block;
        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(outer_block, captured, span)?;
            let closure_block = self.resolve_store_block(outer_block);
            store_block = closure_block;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                closure_block,
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
            .lookup(name)
            .map_err(|msg| self.error(span, msg))?;
        let store_block = self.store_function_decl_callee(
            store_block,
            name,
            scope_id,
            callee_val,
            wrapper_fn_id,
        )?;

        Ok(StmtFlow::Open(store_block))
    }

    /// 声明 async 续体的公共作用域变量
    /// ($env, $this, $state, $resume_val, $is_rejected, $promise, $closure_env)
    #[allow(clippy::type_complexity)]
    fn declare_async_continuation_scopes(
        &mut self,
        span: swc_core::common::Span,
    ) -> Result<(usize, usize, usize, usize, usize, usize, usize), LoweringError> {
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
        Ok((
            env_scope_id,
            this_scope_id,
            state_scope_id,
            resume_val_scope_id,
            is_rejected_scope_id,
            promise_scope_id,
            closure_env_scope_id,
        ))
    }
}
