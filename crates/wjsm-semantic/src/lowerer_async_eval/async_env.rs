use super::*;

impl Lowerer {
    /// 获取闭包创建点可见的环境。按迭代绑定位于独立子环境，函数级共享绑定
    /// 保持在稳定父环境中，避免把 `var` 或外层 `let` 错误复制为每轮私有值。
    pub(crate) fn ensure_shared_env(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        span: Span,
    ) -> Result<ValueId, LoweringError> {
        let has_iteration_capture = captured
            .iter()
            .any(|binding| self.iteration_env_for_binding(binding).is_some());
        if !has_iteration_capture {
            return self.ensure_function_shared_env(block, captured, span);
        }

        let stable_captures = captured
            .iter()
            .filter(|binding| self.iteration_env_for_binding(binding).is_none())
            .cloned()
            .collect::<Vec<_>>();
        let _ = self.ensure_function_shared_env(block, &stable_captures, span)?;
        let current_block = self.resolve_store_block(block);
        let env = self.load_current_iteration_env(current_block);
        // 内层 resolve 会 take 掉 `$shared_env` 慢路径的 merge；调用方还要用
        // 同一 continuation，否则 CreateClosure 会写回已终止的 branch block。
        self.expr_merge_block = Some(current_block);
        Ok(env)
    }

    /// 获取或创建当前函数调用帧的稳定共享环境。
    fn ensure_function_shared_env(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        _span: Span,
    ) -> Result<ValueId, LoweringError> {
        let existing = self.shared_env_stack.last().unwrap().clone();

        // ── 首次创建 ──
        if existing.is_none() {
            self.initialize_shared_env_slot();
            let env_val = self.create_shared_env_object(block, captured);
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: self.shared_env_ir_name(),
                    value: env_val,
                },
            );
            self.write_shared_env_bindings(block, env_val, captured, &Default::default());

            let mut name_set = std::collections::HashSet::new();
            for binding in captured {
                name_set.insert(binding.clone());
            }
            // bb0 创建的 env dominate 所有后续 block，无需运行时分支检查
            let dominates = block == BasicBlockId(0);
            *self.shared_env_stack.last_mut().unwrap() =
                Some((env_val, name_set, block, dominates));
            return Ok(env_val);
        }

        let (existing_env_val, existing_names, last_write_block, dominates) = existing.unwrap();

        // ── 快速路径 A：同一 block 内顺序执行，env 一定已存在 ──
        if block == last_write_block {
            self.write_shared_env_bindings(block, existing_env_val, captured, &existing_names);
            if let Some((_, names, _, _)) = self.shared_env_stack.last_mut().unwrap() {
                for binding in captured {
                    names.insert(binding.clone());
                }
            }
            return Ok(existing_env_val);
        }

        // ── 快速路径 B：env 在 bb0 创建，dominate 所有后续 block，只需 LoadVar +追加绑定 ──
        if dominates {
            let loaded_env = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: loaded_env,
                    name: self.shared_env_ir_name(),
                },
            );
            self.write_shared_env_bindings(block, loaded_env, captured, &existing_names);
            if let Some((value, names, write_block, _)) = self.shared_env_stack.last_mut().unwrap()
            {
                *value = loaded_env;
                *write_block = block;
                for binding in captured {
                    names.insert(binding.clone());
                }
            }
            return Ok(loaded_env);
        }

        // ── 慢路径：不同 block 且 env 不 dominate，需要运行时检查 env 是否已初始化 ──
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
        if let Some((value, names, write_block, dom)) = self.shared_env_stack.last_mut().unwrap() {
            *value = env_val;
            *write_block = merge;
            // merge block 不 dominate 后续（可能有不经过此 merge 的路径）
            *dom = false;
            for binding in captured {
                names.insert(binding.clone());
            }
        }
        self.expr_merge_block = Some(merge);

        Ok(env_val)
    }

    pub(crate) fn iteration_env_for_binding(
        &self,
        binding: &CapturedBinding,
    ) -> Option<&IterationEnvFrame> {
        let function_scope_id = self.current_function_scope_id();
        self.iteration_env_stack.iter().rev().find(|frame| {
            frame.function_scope_id == function_scope_id && frame.bindings.contains(binding)
        })
    }

    fn current_iteration_env(&self) -> Option<&IterationEnvFrame> {
        let function_scope_id = self.current_function_scope_id();
        self.iteration_env_stack
            .iter()
            .rev()
            .find(|frame| frame.function_scope_id == function_scope_id)
    }

    pub(crate) fn prepare_iteration_env(
        &mut self,
        block: BasicBlockId,
        bindings: Vec<CapturedBinding>,
    ) -> Result<(BasicBlockId, IterationEnvFrame), LoweringError> {
        let parent_ir_name = if let Some(parent) = self.current_iteration_env() {
            parent.ir_name.clone()
        } else {
            let _ = self.ensure_function_shared_env(block, &[], DUMMY_SP)?;
            self.shared_env_ir_name()
        };
        let block = self.resolve_store_block(block);
        let name = format!("$iteration_env.{}", self.next_temp);
        self.next_temp += 1;
        let scope_id = self
            .scopes
            .declare(&name, VarKind::Let, true)
            .map_err(|message| self.error(DUMMY_SP, message))?;
        let frame = IterationEnvFrame {
            function_scope_id: self.current_function_scope_id(),
            bindings,
            ir_name: format!("${scope_id}.{name}"),
            parent_ir_name,
        };
        Ok((block, frame))
    }

    fn create_iteration_env_object(
        &mut self,
        block: BasicBlockId,
        frame: &IterationEnvFrame,
    ) -> ValueId {
        let parent = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: parent,
                name: frame.parent_ir_name.clone(),
            },
        );
        let env = self.alloc_value();
        let capacity = u32::try_from(frame.bindings.len())
            .expect("iteration binding count fits object capacity");
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env,
                capacity,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: env,
                value: parent,
            },
        );
        env
    }

    fn store_iteration_env(
        &mut self,
        block: BasicBlockId,
        frame: &IterationEnvFrame,
        env: ValueId,
    ) {
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: frame.ir_name.clone(),
                value: env,
            },
        );
    }

    pub(crate) fn initialize_iteration_env(
        &mut self,
        block: BasicBlockId,
        frame: &IterationEnvFrame,
        copy_previous: bool,
    ) {
        let previous_env = copy_previous.then(|| {
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: value,
                    name: frame.ir_name.clone(),
                },
            );
            value
        });
        let env = self.create_iteration_env_object(block, frame);
        for binding in &frame.bindings {
            let value = self.alloc_value();
            let load = if let Some(previous_env) = previous_env {
                let key = self.append_env_key_const(block, binding);
                Instruction::GetProp {
                    dest: value,
                    object: previous_env,
                    key,
                }
            } else if self.binding_in_tdz(binding) {
                // 首个迭代 env 在循环头声明执行前创建：TDZ 绑定写入哨兵，
                // 声明执行时经 store_binding_value 覆盖。
                Instruction::Const {
                    dest: value,
                    constant: self.module.add_constant(Constant::Uninitialized),
                }
            } else {
                Instruction::LoadVar {
                    dest: value,
                    name: binding.var_ir_name(),
                }
            };
            self.current_function.append_instruction(block, load);
            let key = self.append_env_key_const(block, binding);
            self.emit_set_prop(block, env, key, value);
        }
        self.store_iteration_env(block, frame, env);
    }

    pub(crate) fn initialize_empty_iteration_env(
        &mut self,
        block: BasicBlockId,
        frame: &IterationEnvFrame,
    ) {
        let env = self.create_iteration_env_object(block, frame);
        self.store_iteration_env(block, frame, env);
    }

    pub(crate) fn load_current_iteration_env(&mut self, block: BasicBlockId) -> ValueId {
        let ir_name = self
            .current_iteration_env()
            .expect("iteration env stack underflow")
            .ir_name
            .clone();
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env,
                name: ir_name,
            },
        );
        env
    }

    pub(crate) fn load_iteration_env_for_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> ValueId {
        let ir_name = self
            .iteration_env_for_binding(binding)
            .expect("iteration binding without environment")
            .ir_name
            .clone();
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env,
                name: ir_name,
            },
        );
        env
    }

    pub(crate) fn load_iteration_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> ValueId {
        let env = self.load_iteration_env_for_binding(block, binding);
        let key = self.append_env_key_const(block, binding);
        let value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: value,
                object: env,
                key,
            },
        );
        value
    }

    fn create_shared_env_object(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
    ) -> ValueId {
        let own_binding_count = captured
            .iter()
            .filter(|binding| self.binding_belongs_to_current_function(binding))
            .count();
        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env_val,
                capacity: own_binding_count as u32,
            },
        );
        let parent_env = self.load_env_object(block);
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: env_val,
                value: parent_env,
            },
        );
        env_val
    }

    fn write_shared_env_bindings(
        &mut self,
        block: BasicBlockId,
        env_val: ValueId,
        captured: &[CapturedBinding],
        existing_names: &std::collections::HashSet<CapturedBinding>,
    ) {
        for binding in captured {
            if existing_names.contains(binding)
                || !self.binding_belongs_to_current_function(binding)
            {
                continue;
            }
            // 箭头帧不持有词法 this 的自有副本：owner 是最近的非箭头函数，
            // 其值沿 env 原型链可达；写自有副本会遮蔽 owner，使后续
            // BindThisValue 重绑（super() 返回对象）无法被内层闭包看到。
            if binding.is_lexical_this() && self.is_arrow {
                continue;
            }
            let current_val = self.load_value_for_shared_env_binding(block, binding);
            let key_val = self.append_env_key_const(block, binding);
            self.emit_set_prop(block, env_val, key_val, current_val);
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
        if binding.is_lexical_this() {
            if self.is_arrow {
                // 箭头帧的词法 this 本身来自外层：沿 env 原型链读取并继续
                // 向外登记捕获，保证最近的非箭头 owner 把 this 写入其 env。
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
            // 非箭头帧：this 存于本函数声明的 scoped 槽（`$N.$this`）。
            // 此前误用无前缀 `$this`，与函数体内 `$N.$this` 读取指向不同槽，
            // 触发后端 canonical-this 改名后直接 this 访问读到未初始化值。
            let current_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: current_val,
                    name: self.this_var_ir_name(),
                },
            );
            return current_val;
        }
        if self.binding_belongs_to_current_function(binding) {
            // 闭包先于声明创建（前向引用）：绑定仍处 TDZ，局部槽尚无有效值，
            // 快照写入未初始化哨兵；声明执行时 store_binding_value 同步覆盖 env。
            if self.binding_in_tdz(binding) {
                let sentinel = self.module.add_constant(Constant::Uninitialized);
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: current_val,
                        constant: sentinel,
                    },
                );
                return current_val;
            }
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

    pub(crate) fn resolve_env_binding_owner(
        &mut self,
        block: BasicBlockId,
        start_env: ValueId,
        binding: &CapturedBinding,
    ) -> (BasicBlockId, ValueId) {
        let key = self.append_env_key_const(block, binding);
        let header = self.current_function.new_block();
        let own_block = self.current_function.new_block();
        let parent_block = self.current_function.new_block();
        let done = self.current_function.new_block();
        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        let current_env = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Phi {
                dest: current_env,
                sources: vec![PhiSource {
                    predecessor: block,
                    value: start_env,
                }],
            },
        );
        let owns_binding = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(owns_binding),
                builtin: Builtin::ObjectHasOwn,
                args: vec![current_env, key],
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: owns_binding,
                true_block: own_block,
                false_block: parent_block,
            },
        );

        self.current_function
            .set_terminator(own_block, Terminator::Jump { target: done });

        let parent_env = self.alloc_value();
        self.current_function.append_instruction(
            parent_block,
            Instruction::CallBuiltin {
                dest: Some(parent_env),
                builtin: Builtin::ObjectGetPrototypeOf,
                args: vec![current_env],
            },
        );
        let parent_missing = self.alloc_value();
        self.current_function.append_instruction(
            parent_block,
            Instruction::Unary {
                dest: parent_missing,
                op: UnaryOp::IsNullish,
                value: parent_env,
            },
        );
        self.current_function.set_terminator(
            parent_block,
            Terminator::Branch {
                condition: parent_missing,
                true_block: own_block,
                false_block: header,
            },
        );
        let Some(Instruction::Phi { sources, .. }) = self
            .current_function
            .block_mut(header)
            .and_then(|block| block.instructions_mut().first_mut())
        else {
            unreachable!("env owner loop header must start with phi")
        };
        sources.push(PhiSource {
            predecessor: parent_block,
            value: parent_env,
        });

        let owner = self.alloc_value();
        self.current_function.append_instruction(
            done,
            Instruction::Phi {
                dest: owner,
                sources: vec![PhiSource {
                    predecessor: own_block,
                    value: current_env,
                }],
            },
        );
        (done, owner)
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
            // 派生构造器体内存在箭头 super() 时，this 的规范存储是共享 env
            // （见 lower_class_body 的入口登记）：箭头帧的 BindThisValue 重绑
            // 写 env，构造器帧必须同样经 env 读取才能观察到重绑后的 this。
            Ok(self.emit_read_ctor_this(block))
        }
    }

    /// 读取当前非箭头帧的 this：`ctor_this_via_env` 时经共享 env 读取，
    /// 否则读本地 scoped `$this` 槽。两条路径都是直线指令，不引入控制流。
    pub(crate) fn emit_read_ctor_this(&mut self, block: BasicBlockId) -> ValueId {
        if self.ctor_this_via_env {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: env_val,
                    name: self.shared_env_ir_name(),
                },
            );
            let key_val = self.append_env_key_const(block, &CapturedBinding::lexical_this());
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest,
                    object: env_val,
                    key: key_val,
                },
            );
            return dest;
        }
        let name = self.this_var_ir_name();
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::LoadVar { dest, name });
        dest
    }

    /// 当前帧 this 的本地变量 IR 名：函数上下文声明过 `$this` 时为
    /// `${scope}.$this`，否则（模块/脚本主函数）为无前缀 `$this`。
    pub(crate) fn this_var_ir_name(&self) -> String {
        match self.scopes.lookup("$this") {
            Ok((scope_id, _)) => format!("${scope_id}.$this"),
            Err(_) => "$this".to_string(),
        }
    }
}
