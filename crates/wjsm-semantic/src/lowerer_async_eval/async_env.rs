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
            let name = match self.scopes.lookup("$this") {
                Ok((scope_id, _)) => format!("${scope_id}.$this"),
                Err(_) => "$this".to_string(),
            };
            let dest = self.alloc_value();
            self.current_function
                .append_instruction(block, Instruction::LoadVar { dest, name });
            Ok(dest)
        }
    }
}
