use super::*;

impl Lowerer {
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

    /// 将 async continuation 的 IR 绑定名还原为 CapturedBinding。
    fn captured_binding_from_ir_name(ir_name: &str) -> Option<CapturedBinding> {
        let (scope, name) = ir_name.split_once('.')?;
        let scope_id = scope.strip_prefix('$')?.parse().ok()?;
        Some(CapturedBinding::new(name, scope_id))
    }

    /// 共享 env 中的可变捕获在 suspend 期间可能被微任务（如 ReadableStream pull）更新；
    /// 不得用 continuation 快照覆盖，resume 后应从 $shared_env 或局部槽读取最新值。
    fn async_continuation_should_save_binding(&self, ir_name: &str) -> bool {
        let Some(binding) = Self::captured_binding_from_ir_name(ir_name) else {
            return true;
        };
        !self.is_shared_binding(&binding)
    }

    fn async_live_shared_env_binding(
        &self,
        visible_bindings: &[String],
        successors: &[BasicBlockId],
        live_in: &[std::collections::HashSet<String>],
    ) -> Option<String> {
        self.shared_env_stack.last()?.as_ref()?;
        let has_live_shared_binding = visible_bindings.iter().any(|name| {
            let Some(binding) = Self::captured_binding_from_ir_name(name) else {
                return false;
            };
            self.is_shared_binding(&binding)
                && successors
                    .iter()
                    .any(|successor| live_in[successor.0 as usize].contains(name))
        });
        has_live_shared_binding.then(|| self.shared_env_ir_name())
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
            let (use_sets, def_sets) = compute_use_def(blocks, self.module.constants());
            let live_in = compute_liveness(blocks, &successors, &use_sets, &def_sets);
            (successors, live_in)
        };

        for suspend in &pending {
            let suspend_successors = &successors[suspend.suspend_block.0 as usize];
            let mut live_bindings: Vec<String> = suspend
                .visible_bindings
                .iter()
                .filter(|name| self.async_continuation_should_save_binding(name))
                .filter(|name| {
                    suspend_successors
                        .iter()
                        .any(|successor| live_in[successor.0 as usize].contains(*name))
                })
                .cloned()
                .collect();

            if let Some(shared_env) = self.async_live_shared_env_binding(
                &suspend.visible_bindings,
                suspend_successors,
                &live_in,
            ) {
                live_bindings.push(shared_env);
            }

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
}