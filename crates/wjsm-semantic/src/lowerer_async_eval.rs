use super::*;

impl Lowerer {
    /// 获取或创建当前外层函数的共享 env 对象，并确保所有捕获变量都已写入。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，保证可变捕获变量的修改对所有闭包可见。
    pub(crate) fn ensure_shared_env(
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

    pub(crate) fn lower_super_prop(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 1. GetSuperBase: 从 home_object 的 proto 读取基类原型
        let base_val = self.alloc_value();
        if self.eval_scope_record {
            let env = self.load_eval_scope_env(block);
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(base_val),
                    builtin: Builtin::EvalSuperBase,
                    args: vec![env],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::GetSuperBase { dest: base_val },
            );
        }

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

    pub(crate) fn lower_this(&mut self, block: BasicBlockId) -> Result<ValueId, LoweringError> {
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

    pub(crate) fn init_async_continuation_slots(
        &mut self,
        param_ir_names: &[String],
        first_param_slot: u32,
    ) {
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
    pub(crate) fn init_async_main_context(
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
    pub(crate) fn finalize_async_main(&mut self) -> Result<(), LoweringError> {
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

    pub(crate) fn is_async_internal_binding(name: &str) -> bool {
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

    pub(crate) fn async_visible_binding_names(&self) -> Vec<String> {
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

    pub(crate) fn async_binding_slot(&mut self, ir_name: &str) -> u32 {
        if let Some(slot) = self.captured_var_slots.get(ir_name) {
            return *slot;
        }
        let slot = self.async_next_continuation_slot;
        self.async_next_continuation_slot += 1;
        self.captured_var_slots.insert(ir_name.to_string(), slot);
        slot
    }

    pub(crate) fn emit_save_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
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

    pub(crate) fn emit_restore_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
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

    pub(crate) fn lower_await_expr(
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

    pub(crate) fn lower_yield_expr(
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

    pub(crate) fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref() {
            if ident.sym == "Promise" && self.scopes.lookup(&ident.sym).is_err() {
                return Ok((self.lower_new_promise(new_expr, block)?, block));
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
                return Ok((dest, block));
            }
            // WeakRef / FinalizationRegistry constructors (can throw — need exception checking)
            if self.scopes.lookup(&ident.sym).is_err()
                && let Some(builtin) = builtin_from_global_ident(&ident.sym)
                && matches!(
                    builtin,
                    Builtin::WeakRefConstructor | Builtin::FinalizationRegistryConstructor
                )
            {
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
                        self.current_function
                            .append_instruction(block, Instruction::Const { dest, constant: c });
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
                // Exception check
                let is_exc = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::IsException {
                        dest: is_exc,
                        value: dest,
                    },
                );
                let continue_block = self.current_function.new_block();
                let exc_block = self.current_function.new_block();
                self.current_function.set_terminator(
                    block,
                    Terminator::Branch {
                        condition: is_exc,
                        true_block: exc_block,
                        false_block: continue_block,
                    },
                );
                // Exception path: unwrap and throw
                let thrown_val = self.alloc_value();
                self.current_function.append_instruction(
                    exc_block,
                    Instruction::CallBuiltin {
                        dest: Some(thrown_val),
                        builtin: Builtin::ExceptionValue,
                        args: vec![dest],
                    },
                );
                self.emit_throw_value(exc_block, thrown_val)?;
                return Ok((dest, continue_block));
            }
            // Error constructors: new Error(msg), new TypeError(msg), etc.
            if self.scopes.lookup(&ident.sym).is_err()
                && let Some(builtin) = builtin_from_global_ident(&ident.sym)
                && matches!(
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
                )
            {
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
                        self.current_function
                            .append_instruction(block, Instruction::Const { dest, constant: c });
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
                return Ok((dest, block));
            }
        }

        let mut call_block = block;
        let callee_val = self.lower_expr_then_continue(&new_expr.callee, &mut call_block)?;

        // Create new object.
        let obj_val = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
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
            call_block,
            Instruction::CallBuiltin {
                dest: Some(proto_val),
                builtin: Builtin::GetPrototypeFromConstructor,
                args: vec![callee_val],
            },
        );

        // Set __proto__ on the new object directly via SetProto.
        self.current_function.append_instruction(
            call_block,
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
                let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
                arg_vals.push(arg_val);
            }
        }

        // Call the constructor with the new object as `this`.
        self.current_function.append_instruction(
            call_block,
            Instruction::ConstructCall {
                callee: callee_val,
                this_val: obj_val,
                args: arg_vals,
            },
        );

        Ok((obj_val, call_block))
    }

    pub(crate) fn lower_new_promise(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::NewPromise { dest: promise_val });

        if let Some(args) = &new_expr.args
            && let Some(first_arg) = args.first()
        {
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

        Ok(promise_val)
    }

    // ── Identifiers ─────────────────────────────────────────────────────────

    pub(crate) fn lower_host_builtin_call_expr(
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
        let mut call_block = block;
        for arg in &call.args {
            let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
            args.push(arg_val);
        }

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin,
                args,
            },
        );
        self.expr_merge_block = Some(call_block);
        Ok(dest)
    }

    /// 处理动态 import() 调用
    pub(crate) fn lower_dynamic_import_call(
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
    pub(crate) fn find_dynamic_import_target(
        &self,
        current_module_id: wjsm_ir::ModuleId,
        specifier: &str,
    ) -> Option<wjsm_ir::ModuleId> {
        self.dynamic_import_specifier_map
            .get(&(current_module_id, specifier.to_string()))
            .copied()
    }
}
