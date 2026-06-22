use super::*;

impl Lowerer {
    pub(crate) fn emit_pat_inits_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        param_ir_names: &[String],
        mut block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // param_ir_names[0] = $env, [1] = $this, [2..] = user params (excluding rest)
        let mut ir_name_idx: usize = 2;
        let mut regular_param_count: u32 = 0;
        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(_) = pat {
                break;
            }
            regular_param_count += 1;
        }

        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(rest) = pat {
                let skip = regular_param_count;
                let rest_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CollectRestArgs {
                        dest: rest_val,
                        skip,
                    },
                );
                block = self.lower_destructure_pattern(&rest.arg, rest_val, block, VarKind::Let)?;
                block = self.resolve_store_block(block);
                break;
            }

            let ir_name = &param_ir_names[ir_name_idx];
            match pat {
                swc_ast::Pat::Ident(_) => {
                    // 简单参数无默认值：值已在 local 中，无需操作
                }
                swc_ast::Pat::Assign(assign) => {
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    let resolved = self.lower_default_value_check(raw, &assign.right, block)?;
                    let store_block = self.resolve_store_block(block);
                    self.current_function.append_instruction(
                        store_block,
                        Instruction::StoreVar {
                            name: ir_name.clone(),
                            value: resolved,
                        },
                    );

                    if !matches!(&*assign.left, swc_ast::Pat::Ident(_)) {
                        let loaded = self.alloc_value();
                        self.current_function.append_instruction(
                            store_block,
                            Instruction::LoadVar {
                                dest: loaded,
                                name: ir_name.clone(),
                            },
                        );
                        block = self.lower_destructure_pattern(
                            &assign.left,
                            loaded,
                            store_block,
                            VarKind::Let,
                        )?;
                    }
                }
                _ => {
                    // 解构参数
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    block = self.lower_destructure_pattern(pat, raw, block, VarKind::Let)?;
                }
            }
            ir_name_idx += 1;
            let store_block = self.resolve_store_block(block);
            block = self.resolve_open_after_expr(block, store_block);
        }
        Ok(block)
    }

    /// 将解构 pattern 降低为一系列 IR 指令（对象用 GetProp；数组用 iterator 协议）。
    /// 递归处理嵌套的 Array/Object/Assign pattern。
    pub(crate) fn lower_destructure_pattern(
        &mut self,
        pat: &swc_ast::Pat,
        src_val: ValueId,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        match pat {
            swc_ast::Pat::Ident(binding) => {
                let name = binding.id.sym.to_string();
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(pat.span(), msg))?;
                let ir_name = format!("${scope_id}.{name}");

                if matches!(kind, VarKind::Var) {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                } else {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                }

                let store_block = self.resolve_store_block(block);
                self.current_function.append_instruction(
                    store_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: src_val,
                    },
                );
                let store_block =
                    self.append_eval_var_leak_if_needed(&name, kind, src_val, store_block)?;
                Ok(store_block)
            }
            swc_ast::Pat::Object(object_pat) => {
                self.lower_object_destructure(object_pat, src_val, block, kind)
            }
            swc_ast::Pat::Array(array_pat) => {
                self.lower_array_destructure(array_pat, src_val, block, kind)
            }
            swc_ast::Pat::Assign(assign_pat) => {
                let resolved = self.lower_default_value_check(src_val, &assign_pat.right, block)?;
                let store_block = self.resolve_store_block(block);
                self.lower_destructure_pattern(&assign_pat.left, resolved, store_block, kind)
            }
            swc_ast::Pat::Rest(_) => Err(self.error(
                pat.span(),
                "rest element must be used as a function parameter or inside array destructuring",
            )),
            swc_ast::Pat::Expr(_) | swc_ast::Pat::Invalid(_) => Ok(block),
        }
    }

    /// 对象解构: `{ prop1, prop2: alias, ...rest }`
    pub(crate) fn lower_object_destructure(
        &mut self,
        object_pat: &swc_ast::ObjectPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        let mut excluded_keys = Vec::new();
        for prop in &object_pat.props {
            match prop {
                swc_ast::ObjectPatProp::KeyValue(kv) => {
                    // { key: pattern } 或 { [computed]: pattern }
                    let key_val = self.lower_prop_name(&kv.key, block)?;
                    block = self.resolve_store_block(block);
                    excluded_keys.push(key_val);
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );
                    block = self.lower_destructure_pattern(&kv.value, dest, block, kind)?;
                }
                swc_ast::ObjectPatProp::Assign(assign) => {
                    // { key } 等价于 { key: key }
                    let name = assign.key.id.sym.to_string();
                    let key_const = self.module.add_constant(Constant::String(name.clone()));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    excluded_keys.push(key_val);
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );

                    // 如果有默认值 { key = default }
                    if let Some(default_expr) = &assign.value {
                        let resolved = self.lower_default_value_check(dest, default_expr, block)?;
                        block = self.resolve_store_block(block);
                        let scope_id = self
                            .scopes
                            .resolve_scope_id(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::StoreVar {
                                name: ir_name,
                                value: resolved,
                            },
                        );
                        block =
                            self.append_eval_var_leak_if_needed(&name, kind, resolved, block)?;
                    } else {
                        block = self.lower_destructure_pattern(
                            &swc_ast::Pat::Ident(assign.key.clone()),
                            dest,
                            block,
                            kind,
                        )?;
                    }
                }
                swc_ast::ObjectPatProp::Rest(rest) => {
                    // { ...rest } — 使用 ObjectRest builtin，并排除前面已绑定的属性键。
                    let rest_dest = self.alloc_value();
                    let excluded_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::NewArray {
                            dest: excluded_val,
                            capacity: excluded_keys.len() as u32,
                        },
                    );
                    for key_val in &excluded_keys {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::ArrayPush,
                                args: vec![excluded_val, *key_val],
                            },
                        );
                    }
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(rest_dest),
                            builtin: Builtin::ObjectRest,
                            args: vec![src_val, excluded_val],
                        },
                    );
                    block = self.lower_destructure_pattern(&rest.arg, rest_dest, block, kind)?;
                }
            }
            // 确保 block 指向当前可用的基本块（可能已被 lower_default_value_check 等终结）
            block = self.resolve_store_block(block);
        }
        Ok(block)
    }

    /// 数组解构: `[a, b, ...rest]` — 使用 GetIterator（`IteratorFrom`）逐步取值。
    pub(crate) fn lower_array_destructure(
        &mut self,
        array_pat: &swc_ast::ArrayPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![src_val],
            },
        );

        let mut saw_rest = false;

        for elem in array_pat.elems.iter() {
            let Some(elem) = elem else {
                block = self.lower_array_destructure_skip_hole(block, iter_handle)?;
                block = self.resolve_store_block(block);
                continue;
            };

            if let swc_ast::Pat::Rest(rest) = elem {
                saw_rest = true;
                block =
                    self.lower_array_rest_destructure(iter_handle, &rest.arg, block, kind)?;
                block = self.resolve_store_block(block);
                break;
            }

            block = self.lower_array_destructure_element(elem, iter_handle, block, kind)?;
            block = self.resolve_store_block(block);
        }

        if !saw_rest {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::IteratorClose,
                    args: vec![iter_handle],
                },
            );
        }

        Ok(block)
    }

    /// 数组解构中的空位: 消耗一次 iterator 步进但不绑定。
    fn lower_array_destructure_skip_hole(
        &mut self,
        block: BasicBlockId,
        iter_handle: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        Ok(block)
    }

    /// 绑定数组解构中的一个元素（含 `[a = default]`），按 iterator 协议取值。
    fn lower_array_destructure_element(
        &mut self,
        elem: &swc_ast::Pat,
        iter_handle: ValueId,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );

        let exhausted_block = self.current_function.new_block();
        let active_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: done_val,
                true_block: exhausted_block,
                false_block: active_block,
            },
        );

        let raw_elem_val = self.alloc_value();
        self.current_function.append_instruction(
            active_block,
            Instruction::CallBuiltin {
                dest: Some(raw_elem_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );
        self.current_function.append_instruction(
            active_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );

        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            exhausted_block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );

        self.current_function.set_terminator(
            active_block,
            Terminator::Jump {
                target: continue_block,
            },
        );
        self.current_function.set_terminator(
            exhausted_block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        let merged_elem_val = self.alloc_value();
        self.current_function.append_instruction(
            continue_block,
            Instruction::Phi {
                dest: merged_elem_val,
                sources: vec![
                    PhiSource {
                        predecessor: active_block,
                        value: raw_elem_val,
                    },
                    PhiSource {
                        predecessor: exhausted_block,
                        value: undef_val,
                    },
                ],
            },
        );

        if let swc_ast::Pat::Assign(assign) = elem {
            let with_default = self.lower_default_value_check(
                merged_elem_val,
                &assign.right,
                continue_block,
            )?;
            self.lower_destructure_pattern(&assign.left, with_default, continue_block, kind)
        } else {
            self.lower_destructure_pattern(elem, merged_elem_val, continue_block, kind)
        }
    }

    /// 数组解构中的 rest 元素: `[...rest]`，从当前 iterator 位置收集剩余元素。
    pub(crate) fn lower_array_rest_destructure(
        &mut self,
        iter_handle: ValueId,
        rest_pat: &swc_ast::Pat,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        let result_arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: result_arr,
                capacity: 0,
            },
        );

        let header = self.current_function.new_block();
        let loop_body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

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
                true_block: loop_body,
                false_block: exit,
            },
        );

        let elem_val = self.alloc_value();
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: Some(elem_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ArrayPush,
                args: vec![result_arr, elem_val],
            },
        );
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(loop_body, Terminator::Jump { target: header });

        self.current_function.append_instruction(
            exit,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_handle],
            },
        );

        let _ = self.lower_destructure_pattern(rest_pat, result_arr, exit, kind)?;
        Ok(exit)
    }


    /// 默认值检查: `x = default`
    /// 语义：如果 value === undefined，使用 default 表达式；否则保留原值。
    pub(crate) fn lower_default_value_check(
        &mut self,
        value: ValueId,
        default_expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Compare { op: StrictEq, lhs: value, rhs: Undefined }
        let undef_cid = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_cid,
            },
        );
        let is_undef = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: is_undef,
                op: CompareOp::StrictEq,
                lhs: value,
                rhs: undef_val,
            },
        );

        // Branch
        let then_block = self.current_function.new_block();
        let else_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_undef,
                true_block: then_block,
                false_block: else_block,
            },
        );

        // then_block: 求值默认表达式
        let default_val = self.lower_expr(default_expr, then_block)?;
        self.current_function.set_terminator(
            then_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // else_block: 保留原值
        self.current_function.set_terminator(
            else_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // merge_block: Phi
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: then_block,
                        value: default_val,
                    },
                    PhiSource {
                        predecessor: else_block,
                        value,
                    },
                ],
            },
        );

        Ok(result)
    }
}
