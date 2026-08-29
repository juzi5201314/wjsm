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
    pub(crate) fn store_wrapper_in_outer_scope(
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
            // eval 桥下 wrapper 同样物化闭包，见 lower_fn_decl 的同型分支。
            if let Some((closure_val, closure_block)) =
                self.materialize_eval_bridge_closure(outer_block, wrapper_ref_val)
            {
                store_block = closure_block;
                closure_val
            } else {
                wrapper_ref_val
            }
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

    /// wrapper 侧：把 `emit_arguments_init` 在真实调用帧物化好的 arguments 对象
    /// 保存到续体的固定槽位（紧随形参槽之后），供 generator/async body 绑定同一对象。
    /// 形参绑定名为 `arguments` 时按规范无隐式 arguments 对象，跳过。
    pub(crate) fn emit_wrapper_arguments_slot_save(
        &mut self,
        params: &[swc_ast::Param],
        block: BasicBlockId,
        cont_val: ValueId,
        args_object_slot: u32,
    ) {
        if Self::detect_param_arguments(params) {
            return;
        }
        let Ok((args_scope_id, _)) = self.scopes.lookup("arguments") else {
            return;
        };
        let args_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: args_val,
                name: format!("${args_scope_id}.arguments"),
            },
        );
        let slot_const = self
            .module
            .add_constant(Constant::Number(args_object_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot_val, args_val],
            },
        );
    }

    /// body 侧：从续体固定槽位取出 wrapper 物化的 arguments 对象，
    /// 设为 `emit_arguments_init` 的绑定来源（override 在其入口被 take 消费）。
    pub(crate) fn set_arguments_source_from_slot(
        &mut self,
        params: &[swc_ast::Param],
        block: BasicBlockId,
        cont_val: ValueId,
        args_object_slot: u32,
    ) {
        if Self::detect_param_arguments(params) {
            return;
        }
        let slot_const = self
            .module
            .add_constant(Constant::Number(args_object_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        let args_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(args_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot_val],
            },
        );
        self.arguments_source_override = Some(args_from_cont);
    }

    /// wrapper 侧：在真实调用帧收集 rest 实参数组并保存到续体固定槽位，
    /// 供 generator/async body 解构（body 的原生调用帧没有用户实参可收集）。
    pub(crate) fn emit_wrapper_rest_args_slot_save(
        &mut self,
        params: &[swc_ast::Param],
        block: BasicBlockId,
        cont_val: ValueId,
        rest_args_slot: u32,
    ) {
        if !params
            .iter()
            .any(|p| matches!(p.pat, swc_ast::Pat::Rest(_)))
        {
            return;
        }
        let skip = Self::count_regular_params(params);
        let rest_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CollectRestArgs {
                dest: rest_val,
                skip,
            },
        );
        let slot_const = self
            .module
            .add_constant(Constant::Number(rest_args_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot_val, rest_val],
            },
        );
    }

    /// body 侧：从续体固定槽位取出 wrapper 收集的 rest 实参数组，
    /// 设为 `emit_pat_inits_impl` 的解构来源（override 在其入口被 take 消费）。
    pub(crate) fn set_rest_args_source_from_slot(
        &mut self,
        params: &[swc_ast::Param],
        block: BasicBlockId,
        cont_val: ValueId,
        rest_args_slot: u32,
    ) {
        if !params
            .iter()
            .any(|p| matches!(p.pat, swc_ast::Pat::Rest(_)))
        {
            return;
        }
        let slot_const = self
            .module
            .add_constant(Constant::Number(rest_args_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        let rest_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(rest_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot_val],
            },
        );
        self.rest_args_source_override = Some(rest_from_cont);
    }

    /// 在 async 函数族 body/wrapper 的函数上下文内应用方法 super 绑定：
    /// 方法体内 super 合法；类方法额外继承静态 home object，使嵌套箭头函数
    /// 沿词法链取得同一 [[HomeObject]]。
    pub(crate) fn apply_method_super_binding(&mut self, binding: MethodSuperBinding) {
        match binding {
            MethodSuperBinding::None => {}
            MethodSuperBinding::Static(home) => {
                self.super_allowed = true;
                self.lexical_home_object = Some(home);
            }
            MethodSuperBinding::ClosureEnv => {
                self.super_allowed = true;
            }
        }
    }

    /// wrapper 侧：把方法闭包 env 上的 `home` 绑定转存为续体对象的自有属性。
    /// body 的 activation env 是续体对象，运行时 GetSuperBase 的回退路径按
    /// activation env 的自有 `home` 属性解析 super base。
    pub(crate) fn emit_wrapper_home_transfer(&mut self, block: BasicBlockId, cont_val: ValueId) {
        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env_val,
                name: "$env".to_string(),
            },
        );
        let home_key = self.emit_string_const(block, "home");
        let home_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: home_val,
                object: env_val,
                key: home_key,
                latch: None,
                latch_template: None,
            },
        );
        self.emit_set_prop(block, cont_val, home_key, home_val);
    }

    /// wrapper 侧：把调用时的原始 `this` 保存到续体固定槽位。
    /// async 函数族 body 的 `$this` 形参被 resume 值复用，原始 `this` 必须经槽位传递。
    pub(crate) fn emit_wrapper_this_slot_save(
        &mut self,
        block: BasicBlockId,
        cont_val: ValueId,
        this_slot: u32,
        wrapper_this_scope_id: usize,
    ) {
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: this_val,
                name: format!("${wrapper_this_scope_id}.$this"),
            },
        );
        let slot_const = self.module.add_constant(Constant::Number(this_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot_val, this_val],
            },
        );
    }

    /// body 侧：`$this` 形参承载 resume 值，先复制到 `$resume_val`（调用方已完成），
    /// 再从续体槽位恢复原始 `this` 写回 `$this`，使函数体内的 `this` 表达式取到正确值。
    pub(crate) fn emit_body_this_restore_from_slot(
        &mut self,
        block: BasicBlockId,
        cont_val: ValueId,
        this_slot: u32,
        this_scope_id: usize,
    ) {
        let slot_const = self.module.add_constant(Constant::Number(this_slot as f64));
        let slot_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            },
        );
        let original_this = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(original_this),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot_val],
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: format!("${this_scope_id}.$this"),
                value: original_this,
            },
        );
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
