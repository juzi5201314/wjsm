use super::*;
use std::collections::{HashMap, HashSet};

/// 推迟发射 save/restore 的 suspend 记录
#[derive(Debug, Clone)]
pub(super) struct PendingSuspend {
    /// Suspend 指令所在的 block
    pub(super) suspend_block: BasicBlockId,
    /// resume 后执行起始 block
    pub(super) resume_block: BasicBlockId,
    /// 该 suspend 点可见的所有绑定（async_visible_binding_names 结果）
    pub(super) visible_bindings: Vec<String>,
}

/// 构建 CFG：返回 successors、predecessors 映射。
/// Suspend block 的逻辑 successor 是 resume_block，而不是 terminator 的 Jump 目标。
fn build_cfg(
    blocks: &[BasicBlock],
    pending_suspends: &[PendingSuspend],
) -> (Vec<Vec<BasicBlockId>>, Vec<Vec<BasicBlockId>>) {
    let block_count = blocks.len();
    let suspend_to_resume: HashMap<BasicBlockId, BasicBlockId> = pending_suspends
        .iter()
        .map(|pending| (pending.suspend_block, pending.resume_block))
        .collect();

    let mut successors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];
    let mut predecessors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];

    for block in blocks {
        let bid = block.id();
        let targets: Vec<BasicBlockId> = if let Some(&resume) = suspend_to_resume.get(&bid) {
            vec![resume]
        } else {
            match block.terminator() {
                Terminator::Jump { target } => vec![*target],
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => vec![*true_block, *false_block],
                Terminator::Switch {
                    cases,
                    default_block,
                    ..
                } => {
                    let mut targets = Vec::with_capacity(cases.len() + 1);
                    targets.extend(cases.iter().map(|case| case.target));
                    targets.push(*default_block);
                    targets
                }
                Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {
                    Vec::new()
                }
            }
        };

        let bid_index = bid.0 as usize;
        for target in targets {
            successors[bid_index].push(target);
            predecessors[target.0 as usize].push(bid);
        }
    }

    (successors, predecessors)
}

/// 计算每个 block 的 use 和 def 集合，只考虑用户变量，排除 async 内部绑定。
/// 闭包捕获会在 ensure_shared_env 中先降低为 LoadVar，因此 CreateClosure 本身无需额外建模。
fn compute_use_def(blocks: &[BasicBlock]) -> (Vec<HashSet<String>>, Vec<HashSet<String>>) {
    let mut use_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];
    let mut def_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];

    for block in blocks {
        let bid = block.id().0 as usize;
        let mut local_def: HashSet<String> = HashSet::new();

        for instr in block.instructions() {
            match instr {
                Instruction::LoadVar { name, .. } => {
                    if !Lowerer::is_async_internal_binding(name) && !local_def.contains(name) {
                        use_sets[bid].insert(name.clone());
                    }
                }
                Instruction::StoreVar { name, .. }
                    if !Lowerer::is_async_internal_binding(name) => {
                        local_def.insert(name.clone());
                        def_sets[bid].insert(name.clone());
                    }
                _ => {}
            }
        }
    }

    (use_sets, def_sets)
}

/// 标准后向迭代 liveness 分析，返回每个 block 入口处的 live_in 集合。
fn compute_liveness(
    blocks: &[BasicBlock],
    successors: &[Vec<BasicBlockId>],
    use_sets: &[HashSet<String>],
    def_sets: &[HashSet<String>],
) -> Vec<HashSet<String>> {
    let block_count = blocks.len();
    let mut live_in: Vec<HashSet<String>> = vec![HashSet::new(); block_count];
    let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); block_count];

    loop {
        let mut changed = false;

        for block in blocks.iter().rev() {
            let bid = block.id().0 as usize;

            let mut new_live_out: HashSet<String> = HashSet::new();
            for &successor in &successors[bid] {
                new_live_out.extend(live_in[successor.0 as usize].iter().cloned());
            }

            if new_live_out != live_out[bid] {
                live_out[bid] = new_live_out;
                changed = true;
            }

            let mut new_live_in = use_sets[bid].clone();
            for var in &live_out[bid] {
                if !def_sets[bid].contains(var) {
                    new_live_in.insert(var.clone());
                }
            }

            if new_live_in != live_in[bid] {
                live_in[bid] = new_live_in;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    live_in
}

impl Lowerer {
    /// 获取或创建当前外层函数的共享 env 对象，并确保所有捕获变量都已写入。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，保证可变捕获变量的修改对所有闭包可见。
    pub(crate) fn ensure_shared_env(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        _span: Span,
    ) -> Result<ValueId, LoweringError> {
        let existing_env_val = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(value, _)| *value);
        let existing_names = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(_, names)| names.clone())
            .unwrap_or_default();

        if existing_env_val.is_none() {
            self.initialize_shared_env_slot();
            let env_val = self.create_shared_env_object(block, captured);
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: self.shared_env_ir_name(),
                    value: env_val,
                },
            );
            self.write_shared_env_bindings(block, env_val, captured, &existing_names);

            let mut name_set = std::collections::HashSet::new();
            for binding in captured {
                name_set.insert(binding.clone());
            }
            *self.shared_env_stack.last_mut().unwrap() = Some((env_val, name_set));
            return Ok(env_val);
        }

        let branch_block = if self.current_function.block(block).is_some_and(|candidate| {
            candidate
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        }) {
            let next = self.current_function.new_block();
            self.current_function
                .set_terminator(block, Terminator::Jump { target: next });
            next
        } else {
            block
        };

        let loaded_env = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::LoadVar {
                dest: loaded_env,
                name: self.shared_env_ir_name(),
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let env_missing = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Compare {
                dest: env_missing,
                op: CompareOp::StrictEq,
                lhs: loaded_env,
                rhs: undef_val,
            },
        );

        let create_block = self.current_function.new_block();
        let existing_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: env_missing,
                true_block: create_block,
                false_block: existing_block,
            },
        );

        let mut create_bindings = existing_names.iter().cloned().collect::<Vec<_>>();
        create_bindings.sort_by_key(CapturedBinding::env_key);
        for binding in captured {
            if !create_bindings.contains(binding) {
                create_bindings.push(binding.clone());
            }
        }
        let created_env = self.create_shared_env_object(create_block, &create_bindings);
        self.current_function.append_instruction(
            create_block,
            Instruction::StoreVar {
                name: self.shared_env_ir_name(),
                value: created_env,
            },
        );
        self.write_shared_env_bindings(
            create_block,
            created_env,
            &create_bindings,
            &Default::default(),
        );
        self.current_function
            .set_terminator(create_block, Terminator::Jump { target: merge });

        self.write_shared_env_bindings(existing_block, loaded_env, captured, &existing_names);
        self.current_function
            .set_terminator(existing_block, Terminator::Jump { target: merge });

        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: env_val,
                sources: vec![
                    PhiSource {
                        predecessor: create_block,
                        value: created_env,
                    },
                    PhiSource {
                        predecessor: existing_block,
                        value: loaded_env,
                    },
                ],
            },
        );
        self.current_function.append_instruction(
            merge,
            Instruction::StoreVar {
                name: self.shared_env_ir_name(),
                value: env_val,
            },
        );
        if let Some((value, names)) = self.shared_env_stack.last_mut().unwrap() {
            *value = env_val;
            for binding in captured {
                names.insert(binding.clone());
            }
        }
        self.expr_merge_block = Some(merge);

        Ok(env_val)
    }

    fn create_shared_env_object(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
    ) -> ValueId {
        if captured
            .iter()
            .any(|binding| !self.binding_belongs_to_current_function(binding))
        {
            self.load_env_object(block)
        } else {
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

    fn write_shared_env_bindings(
        &mut self,
        block: BasicBlockId,
        env_val: ValueId,
        captured: &[CapturedBinding],
        existing_names: &std::collections::HashSet<CapturedBinding>,
    ) {
        for binding in captured {
            if existing_names.contains(binding) {
                continue;
            }
            let current_val = self.load_value_for_shared_env_binding(block, binding);
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
    }

    fn load_value_for_shared_env_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> ValueId {
        if binding.is_lexical_new_target() {
            if self.is_arrow {
                self.record_capture(binding.clone());
                let env_val = self.load_env_object(block);
                let key_val = self.append_env_key_const(block, binding);
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: current_val,
                        object: env_val,
                        key: key_val,
                    },
                );
                return current_val;
            }
            let dummy_const = self.module.add_constant(Constant::Undefined);
            let dummy_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: dummy_val,
                    constant: dummy_const,
                },
            );
            let current_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(current_val),
                    builtin: Builtin::NewTarget,
                    args: vec![dummy_val],
                },
            );
            return current_val;
        }
        if self.binding_belongs_to_current_function(binding) {
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
        }
    }

    pub(crate) fn lower_super_prop(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if !self.eval_scope_record && !self.super_allowed {
            return Err(self.error(super_prop.span, "super is only valid inside methods"));
        }

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
            self.current_function
                .append_instruction(block, Instruction::GetSuperBase { dest: base_val });
        }

        // 2. super 属性访问必须以当前 this 作为 receiver（访问器与方法 this 绑定依赖它）。
        let this_val = self.lower_this(block)?;
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
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ReflectGet,
                        args: vec![base_val, key_dest, this_val],
                    },
                );
                Ok(dest)
            }
            swc_ast::SuperProp::Computed(computed) => {
                let key_val = self.lower_expr(&computed.expr, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ReflectGet,
                        args: vec![base_val, key_val, this_val],
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
        // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
        self.resolve_pending_suspends();
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
        let mut wrapper_ir = Function::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0));
        wrapper_ir.set_has_eval(wrapper_has_eval);
        wrapper_ir.set_params(self.async_main_param_ir_names.clone());
        for b in wrapper_blocks {
            wrapper_ir.push_block(b);
        }
        self.module.push_function(wrapper_ir);

        Ok(())
    }

    pub(crate) fn is_async_internal_binding(name: &str) -> bool {
        let binding_name = if let Some((scope, binding)) = name.split_once('.') {
            if scope.len() > 1
                && scope.starts_with('$')
                && scope[1..].bytes().all(|byte| byte.is_ascii_digit())
            {
                binding
            } else {
                name
            }
        } else {
            name
        };

        matches!(
            binding_name,
            "$env"
                | "$this"
                | "$state"
                | "$resume_val"
                | "$is_rejected"
                | "$promise"
                | "$closure_env"
                | "$generator"
        ) || binding_name.starts_with("$tmp.")
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

    /// 函数体 lowering 完成后，根据后向 liveness 分析补发 suspend save/restore。
    pub(crate) fn resolve_pending_suspends(&mut self) {
        if self.pending_suspends.is_empty() {
            return;
        }

        let pending = std::mem::take(&mut self.pending_suspends);
        let (successors, live_in) = {
            let blocks = self.current_function.blocks();
            let (successors, _predecessors) = build_cfg(blocks, &pending);
            let (use_sets, def_sets) = compute_use_def(blocks);
            let live_in = compute_liveness(blocks, &successors, &use_sets, &def_sets);
            (successors, live_in)
        };

        for suspend in &pending {
            let suspend_successors = &successors[suspend.suspend_block.0 as usize];
            let live_bindings: Vec<String> = suspend
                .visible_bindings
                .iter()
                .filter(|name| {
                    suspend_successors
                        .iter()
                        .any(|successor| live_in[successor.0 as usize].contains(*name))
                })
                .cloned()
                .collect();

            self.insert_save_before_suspend(suspend.suspend_block, &live_bindings);
            self.insert_restore_at_start(suspend.resume_block, &live_bindings);
        }
    }

    /// 在指定 block 的 Suspend 指令之前插入 save 指令序列。
    fn insert_save_before_suspend(&mut self, block_id: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let Some((suspend_idx, instruction_count)) =
            self.current_function.block(block_id).and_then(|block| {
                let suspend_idx = block
                    .instructions()
                    .iter()
                    .position(|instr| matches!(instr, Instruction::Suspend { .. }))?;
                Some((suspend_idx, block.instructions().len()))
            })
        else {
            return;
        };
        assert_eq!(
            suspend_idx + 1,
            instruction_count,
            "suspend block {block_id} must not contain instructions after Suspend"
        );

        let continuation = self.alloc_value();
        let mut save_instrs = Vec::with_capacity(1 + bindings.len() * 3);
        save_instrs.push(Instruction::LoadVar {
            dest: continuation,
            name: format!("${}.$env", self.async_env_scope_id),
        });

        for binding in bindings {
            let slot = self.async_binding_slot(binding);
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            let value = self.alloc_value();

            save_instrs.push(Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            });
            save_instrs.push(Instruction::LoadVar {
                dest: value,
                name: binding.clone(),
            });
            save_instrs.push(Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![continuation, slot_val, value],
            });
        }

        let Some(block) = self.current_function.block_mut(block_id) else {
            return;
        };
        block
            .instructions_mut()
            .splice(suspend_idx..suspend_idx, save_instrs);
    }

    /// 在指定 resume block 开头插入 restore 指令序列。
    fn insert_restore_at_start(&mut self, block_id: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let continuation = self.alloc_value();
        let mut restore_instrs = Vec::with_capacity(1 + bindings.len() * 3);
        restore_instrs.push(Instruction::LoadVar {
            dest: continuation,
            name: format!("${}.$env", self.async_env_scope_id),
        });

        for binding in bindings {
            let Some(&slot) = self.captured_var_slots.get(binding) else {
                continue;
            };
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            let value = self.alloc_value();

            restore_instrs.push(Instruction::Const {
                dest: slot_val,
                constant: slot_const,
            });
            restore_instrs.push(Instruction::CallBuiltin {
                dest: Some(value),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![continuation, slot_val],
            });
            restore_instrs.push(Instruction::StoreVar {
                name: binding.clone(),
                value,
            });
        }

        let Some(block) = self.current_function.block_mut(block_id) else {
            return;
        };
        block.instructions_mut().splice(0..0, restore_instrs);
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
        let visible_bindings = self.async_visible_binding_names();

        // 推迟 save/restore —— 由 resolve_pending_suspends 在函数体 lowering 完成后统一处理
        self.pending_suspends.push(PendingSuspend {
            suspend_block: block,
            resume_block,
            visible_bindings,
        });

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
        self.await_continue_block = Some(continue_block);

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
            let visible_bindings = self.async_visible_binding_names();

            self.pending_suspends.push(PendingSuspend {
                suspend_block: block,
                resume_block,
                visible_bindings,
            });
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
                    Builtin::WeakRefConstructor
                        | Builtin::FinalizationRegistryConstructor
                        | Builtin::HeadersConstructor
                        | Builtin::RequestConstructor
                        | Builtin::ResponseConstructor
                        | Builtin::AbortControllerConstructor
                        | Builtin::ReadableStreamConstructor
                        | Builtin::WritableStreamConstructor
                        | Builtin::TransformStreamConstructor
                        | Builtin::CountQueuingStrategyConstructor
                        | Builtin::ByteLengthQueuingStrategyConstructor
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
                        | Builtin::SharedArrayBufferConstructor
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
                        | Builtin::BigInt64ArrayConstructor
                        | Builtin::BigUint64ArrayConstructor
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
            let mut call_block = block;
            let callback_val = self.lower_expr_then_continue(&first_arg.expr, &mut call_block)?;

            let resolve_fn = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(resolve_fn),
                    builtin: Builtin::PromiseCreateResolveFunction,
                    args: vec![promise_val],
                },
            );

            let reject_fn = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(reject_fn),
                    builtin: Builtin::PromiseCreateRejectFunction,
                    args: vec![promise_val],
                },
            );

            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );

            self.current_function.append_instruction(
                call_block,
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
        if matches!(builtin, Builtin::JsonParse) {
            let is_exc = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::IsException {
                    dest: is_exc,
                    value: dest,
                },
            );
            let continue_block = self.current_function.new_block();
            let exc_block = self.current_function.new_block();
            self.current_function.set_terminator(
                call_block,
                Terminator::Branch {
                    condition: is_exc,
                    true_block: exc_block,
                    false_block: continue_block,
                },
            );
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
            self.expr_merge_block = Some(continue_block);
            return Ok(dest);
        }
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
