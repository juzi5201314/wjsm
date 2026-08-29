use super::*;

impl Lowerer {
    pub(crate) fn emit_pat_inits_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        param_ir_names: &[String],
        mut block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // 入口即 take：即使本次没有 rest 形参，也不会把过期的来源泄漏给后续函数。
        let rest_override = self.rest_args_source_override.take();
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
                // generator/async 函数 body：wrapper 已在真实调用帧收集 rest 实参并经
                // 续体槽位传入，直接解构该数组（body 的原生调用帧没有用户实参）。
                let rest_val = if let Some(source) = rest_override {
                    source
                } else {
                    let skip = regular_param_count;
                    let rest_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CollectRestArgs {
                            dest: rest_val,
                            skip,
                        },
                    );
                    rest_val
                };
                // rest 形参值是收集数组（永不 nullish），来源上下文惰性；
                // 其元素位置的嵌套对象模式按数组嵌套处理。
                block = self.lower_destructure_pattern(
                    &rest.arg,
                    rest_val,
                    block,
                    VarKind::Let,
                    &DestructureSource::NestedInArray,
                )?;
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
                    // NamedEvaluation：`f(x = <匿名函数定义>)` 按形参名命名。
                    self.stage_named_eval_for_binding(&assign.left, &assign.right);
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
                        // 带默认值的形参：V8 把形参初始化脱糖为三元表达式，
                        // 文案调用点为三连 "(intermediate value)"。
                        let default_source = DestructureSource::TopLevel(
                            DestructureCallsite::Text("(intermediate value)".repeat(3)),
                        );
                        block = self.lower_destructure_pattern(
                            &assign.left,
                            loaded,
                            store_block,
                            VarKind::Let,
                            &default_source,
                        )?;
                    }
                }
                _ => {
                    // 解构参数：无源码调用点，文案按运行期值渲染。
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    block = self.lower_destructure_pattern(
                        pat,
                        raw,
                        block,
                        VarKind::Let,
                        &DestructureSource::TopLevel(DestructureCallsite::RuntimeDefault),
                    )?;
                }
            }
            ir_name_idx += 1;
            let store_block = self.resolve_store_block(block);
            block = self.resolve_open_after_expr(block, store_block);
        }
        Ok(block)
    }

    /// 将解构 pattern 降低为一系列 IR 指令（GetProp/GetElem + StoreVar）。
    /// 递归处理嵌套的 Array/Object/Assign pattern。`source` 为对象模式
    /// RequireObjectCoercible 检查的值来源上下文（仅对象模式消费；标识符 /
    /// 数组模式忽略，数组元素的嵌套对象模式统一转为 NestedInArray）。
    pub(crate) fn lower_destructure_pattern(
        &mut self,
        pat: &swc_ast::Pat,
        src_val: ValueId,
        block: BasicBlockId,
        kind: VarKind,
        source: &DestructureSource,
    ) -> Result<BasicBlockId, LoweringError> {
        match pat {
            swc_ast::Pat::Ident(binding) => {
                let name = binding.id.sym.to_string();
                // 解析穿越 with 作用域（`var` 提升越过 with 体、赋值型解构）：
                // 写入按对象环境记录动态分派；声明型 let/const 绑定声明于
                // with 体内侧作用域，不会走到这里。
                let crossed = self.with_scopes_for_ident(&name);
                if !crossed.is_empty() {
                    let _ = self.scopes.mark_initialised(&name);
                    return self.lower_with_bare_write(&name, pat.span(), src_val, &crossed, block);
                }
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(pat.span(), msg))?;

                if matches!(kind, VarKind::Var) {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                } else {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                }

                // 脚本全局词法声明的绑定初始化：InitializeBinding（§9.1.1.4.4）
                // 写宿主全局声明式记录并解除 TDZ。赋值型解构不受此分流（走
                // store_binding_value → GlobalEnvSet 的 SetMutableBinding 语义）。
                if self.script_global_decl_init
                    && scope_id == 0
                    && matches!(
                        self.script_global_names.get(&name),
                        Some(ScriptGlobalKind::Lexical { .. })
                    )
                {
                    return Ok(self.emit_script_global_init_lex(block, &name, src_val));
                }

                let binding = CapturedBinding::new(name.clone(), scope_id);
                let store_block =
                    self.store_binding_value(block, &binding, src_val, pat.span(), true)?;
                let store_block =
                    self.append_eval_var_leak_if_needed(&name, kind, src_val, store_block)?;
                Ok(store_block)
            }
            swc_ast::Pat::Object(object_pat) => {
                self.lower_object_destructure(object_pat, src_val, block, kind, source)
            }
            swc_ast::Pat::Array(array_pat) => {
                self.lower_array_destructure(array_pat, src_val, block, kind)
            }
            swc_ast::Pat::Assign(assign_pat) => {
                // NamedEvaluation：解构默认值 `{ a: f = <匿名函数定义> }` 按目标标识符命名。
                self.stage_named_eval_for_binding(&assign_pat.left, &assign_pat.right);
                let resolved = self.lower_default_value_check(src_val, &assign_pat.right, block)?;
                let store_block = self.resolve_store_block(block);
                // 带默认值的子模式：无论默认是否被采用，V8 的文案位置都指向
                // 默认值表达式（顶层形态，调用点为默认表达式文本）。
                let default_source = DestructureSource::TopLevel(DestructureCallsite::Text(
                    render_destructure_callsite(&assign_pat.right),
                ));
                self.lower_destructure_pattern(
                    &assign_pat.left,
                    resolved,
                    store_block,
                    kind,
                    &default_source,
                )
            }
            swc_ast::Pat::Rest(_) => Err(self.error(
                pat.span(),
                "rest element must be used as a function parameter or inside array destructuring",
            )),
            // 解构赋值的成员目标（`({ a: o.x } = ..)` / `[o.x] = ..`）：
            // DestructuringAssignmentEvaluation 对 LeftHandSideExpression 目标
            // 执行 PutValue，strict 失败语义与普通成员赋值一致。
            swc_ast::Pat::Expr(expr) => self.lower_destructure_member_target(expr, src_val, block),
            swc_ast::Pat::Invalid(_) => Ok(block),
        }
    }

    /// 解构赋值中的成员/super 目标：PutValue(memberRef, src_val)。标识符
    /// 目标由 `Pat::Ident` 分支处理，此处只出现成员表达式类目标。
    fn lower_destructure_member_target(
        &mut self,
        expr: &swc_ast::Expr,
        src_val: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        if let swc_ast::Expr::SuperProp(super_prop) = expr {
            let mut current_block = block;
            let access = self.lower_super_prop_access(super_prop, &mut current_block)?;
            self.emit_super_prop_set(current_block, &access, src_val);
            current_block = self.resolve_store_block(current_block);
            return Ok(current_block);
        }
        let swc_ast::Expr::Member(member_expr) = expr else {
            return Err(self.error(
                expr.span(),
                "unsupported destructuring assignment target expression",
            ));
        };
        let mut current_block = block;
        let obj_val = self.lower_expr_then_continue(&member_expr.obj, &mut current_block)?;
        let result = match &member_expr.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let name = ident.sym.to_string();
                // `__proto__` 目标与普通赋值同口径：走 setPrototypeOf 语义。
                if name == "__proto__" {
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::ObjectSetPrototypeOf,
                            args: vec![obj_val, src_val],
                        },
                    );
                    return self.lower_value_exception_branch(current_block, dest);
                }
                let key_const = self.module.add_constant(Constant::String(name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                self.emit_set_prop(current_block, obj_val, key_val, src_val)
            }
            swc_ast::MemberProp::Computed(computed) => {
                let key_val = self.lower_expr_then_continue(&computed.expr, &mut current_block)?;
                self.emit_set_elem(current_block, obj_val, key_val, src_val)
            }
            swc_ast::MemberProp::PrivateName(name) => {
                let field_name =
                    self.resolve_private_storage_name(name.name.as_ref(), name.span)?;
                let key_const = self.module.add_constant(Constant::String(field_name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::PrivateSet,
                        args: vec![obj_val, key_val, src_val],
                    },
                );
                dest
            }
        };
        // PutValue 失败（strict TypeError）或 setter 抛出须传播。
        current_block = self.lower_value_exception_branch(current_block, result)?;
        Ok(current_block)
    }

    /// 对象解构: `{ prop1, prop2: alias, ...rest }`
    pub(crate) fn lower_object_destructure(
        &mut self,
        object_pat: &swc_ast::ObjectPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
        source: &DestructureSource,
    ) -> Result<BasicBlockId, LoweringError> {
        // RequireObjectCoercible（§13.15.5.2 步骤 1 / §8.6.2 步骤 1）：nullish
        // 值先于任何键求值 / 属性读取抛 TypeError；文案矩阵见
        // emit_object_coercible_check。首属性为简单键时由 GetProp 的 ToObject
        // TypeError 承担（与 V8 消除冗余检查一致）。
        let (checked_block, child_callsite) =
            self.emit_object_coercible_check(object_pat, src_val, block, source)?;
        block = checked_block;
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
                            latch: None,
                            latch_template: None,
                        },
                    );
                    // getter 可能抛出：异常须先于后续绑定/写入传播。
                    block = self.lower_value_exception_branch(block, dest)?;
                    // 属性值位置的嵌套模式：继承顶层调用点，外层键供空/计算
                    // 键嵌套模式的检查文案使用。
                    let nested_source = DestructureSource::NestedInObject {
                        outer_key: decl_coercible::render_prop_name(&kv.key),
                        callsite: child_callsite.clone(),
                    };
                    block = self.lower_destructure_pattern(
                        &kv.value,
                        dest,
                        block,
                        kind,
                        &nested_source,
                    )?;
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
                            latch: None,
                            latch_template: None,
                        },
                    );
                    // getter 可能抛出：异常须先于默认值判定/绑定传播。
                    block = self.lower_value_exception_branch(block, dest)?;

                    // 如果有默认值 { key = default }
                    if let Some(default_expr) = &assign.value {
                        // NamedEvaluation：`{ f = <匿名函数定义> }` 按绑定标识符命名。
                        if Self::is_anonymous_fn_definition(default_expr) {
                            self.named_eval_hint = Some(name.clone());
                        }
                        let resolved = self.lower_default_value_check(dest, default_expr, block)?;
                        block = self.resolve_store_block(block);
                        let scope_id = self
                            .scopes
                            .resolve_scope_id(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        // 脚本全局词法声明初始化：与 Pat::Ident 分支同口径走
                        // InitializeBinding（GlobalEnvInitLex）。
                        if self.script_global_decl_init
                            && scope_id == 0
                            && matches!(
                                self.script_global_names.get(&name),
                                Some(ScriptGlobalKind::Lexical { .. })
                            )
                        {
                            block = self.emit_script_global_init_lex(block, &name, resolved);
                        } else {
                            let binding = CapturedBinding::new(name.clone(), scope_id);
                            block = self.store_binding_value(
                                block,
                                &binding,
                                resolved,
                                assign.key.span(),
                                true,
                            )?;
                            block =
                                self.append_eval_var_leak_if_needed(&name, kind, resolved, block)?;
                        }
                    } else {
                        // 标识符目标不消费来源上下文。
                        block = self.lower_destructure_pattern(
                            &swc_ast::Pat::Ident(assign.key.clone()),
                            dest,
                            block,
                            kind,
                            &DestructureSource::NestedInArray,
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
                    // BindingRestProperty 目标是标识符（赋值型可为成员表达式），
                    // 不消费来源上下文。
                    block = self.lower_destructure_pattern(
                        &rest.arg,
                        rest_dest,
                        block,
                        kind,
                        &DestructureSource::NestedInArray,
                    )?;
                }
            }
            // 确保 block 指向当前可用的基本块（可能已被 lower_default_value_check 等终结）
            block = self.resolve_store_block(block);
        }
        Ok(block)
    }

    /// 数组解构: `[a, b, ...rest]`
    pub(crate) fn lower_array_destructure(
        &mut self,
        array_pat: &swc_ast::ArrayPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        // 1. 创建迭代器。GetIterator 自身的 abrupt 在保护区注册之前分叉：
        // 迭代器记录尚不存在，无 close 可做。
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![src_val],
            },
        );
        block = self.lower_value_exception_branch(block, iter_handle)?;

        // 迭代器保护区（§8.6.2 / §13.15.5.2 步骤 3）：元素初始化期间的任何
        // abrupt completion（默认值求值、成员 setter、嵌套 pattern、TDZ 写入
        // 等抛出）经 emit_throw_value / emit_unwind_for_abrupt 展开时发射
        // IteratorClose。步骤自身的 abrupt（next/done/value 抛出）与耗尽后的
        // abrupt 由宿主按迭代器记录的 [[Done]] 状态跳过 return() 调用——
        // 编译期无法区分运行期 [[Done]]，门禁归宿主所有。
        self.label_stack.push(LabelContext {
            label: None,
            kind: LabelKind::Destructuring,
            break_target: block,
            continue_target: None,
            iterator_to_close: Some(iter_handle),
            break_reached: false,
        });

        let mut saw_rest = false;

        for elem in array_pat.elems.iter() {
            let Some(elem) = elem else {
                // 空位：消耗一次迭代但不绑定。Elision 的 `? IteratorStep`
                // （§8.6.2）abrupt 时须传播——next()/done 读取抛出以异常值返回。
                let hole_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(hole_val),
                        builtin: Builtin::IteratorStepValue,
                        args: vec![iter_handle],
                    },
                );
                block = self.lower_value_exception_branch(block, hole_val)?;
                continue;
            };

            if let swc_ast::Pat::Rest(rest) = elem {
                saw_rest = true;
                block = self.lower_array_rest_destructure(iter_handle, &rest.arg, block, kind)?;
                block = self.resolve_store_block(block);
                break;
            }

            // 取下一个迭代值（已耗尽则返回 undefined）。IteratorStepValue 的
            // abrupt completion（next 调用 / done / value 读取抛出）以异常值
            // 返回，须先于默认值判定与绑定传播（§8.6.2 IteratorBindingInitialization）。
            let elem_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(elem_val),
                    builtin: Builtin::IteratorStepValue,
                    args: vec![iter_handle],
                },
            );
            block = self.lower_value_exception_branch(block, elem_val)?;

            if let swc_ast::Pat::Assign(assign) = elem {
                // NamedEvaluation：`[f = <匿名函数定义>]` 按目标标识符命名。
                self.stage_named_eval_for_binding(&assign.left, &assign.right);
                let resolved = self.lower_default_value_check(elem_val, &assign.right, block)?;
                block = self.resolve_store_block(block);
                // 带默认值的元素：V8 文案位置指向默认值表达式（顶层形态）。
                let default_source = DestructureSource::TopLevel(DestructureCallsite::Text(
                    render_destructure_callsite(&assign.right),
                ));
                block = self.lower_destructure_pattern(
                    &assign.left,
                    resolved,
                    block,
                    kind,
                    &default_source,
                )?;
            } else {
                block = self.lower_destructure_pattern(
                    elem,
                    elem_val,
                    block,
                    kind,
                    &DestructureSource::NestedInArray,
                )?;
            }
            block = self.resolve_store_block(block);
        }

        self.label_stack
            .pop()
            .expect("destructuring label context pushed above");

        // 无 rest 元素时关闭迭代器（正常完成，[[Done]] 门禁在宿主侧）
        if !saw_rest {
            block = self.emit_single_iterator_close_normal(block, iter_handle)?;
        }

        Ok(block)
    }

    /// 数组解构中的 rest 元素: `[...rest]`
    /// 从已有迭代器位置收集剩余元素到一个新数组
    pub(crate) fn lower_array_rest_destructure(
        &mut self,
        iter_handle: ValueId,
        rest_pat: &swc_ast::Pat,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<BasicBlockId, LoweringError> {
        // 创建结果数组
        let result_arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: result_arr,
                capacity: 0,
            },
        );

        // 循环收集剩余元素（与 for-of 相同的 header→body→header 结构）
        let header = self.current_function.new_block();
        let loop_body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: 检查 done。IteratorDone 对自定义迭代器惰性调用 next() 并
        // 缓存结果，step 错误（next 抛出 / done 读取抛出）以异常值返回；
        // 不检查会让异常值流入 Not/Branch 而循环永不终止（§8.6.2
        // BindingRestElement：abrupt 时置 [[Done]] 并向外抛，不做 IteratorClose）。
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );
        let condition_block = self.lower_value_exception_branch(header, done_val)?;
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            condition_block,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            condition_block,
            Terminator::Branch {
                condition: not_done,
                true_block: loop_body,
                false_block: exit,
            },
        );

        // body: 取值并前进（IteratorStepValue 一步完成，value getter 只求值
        // 一次）；value 读取抛出同样以异常值返回，须在 push 前分叉传播。
        let elem_val = self.alloc_value();
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: Some(elem_val),
                builtin: Builtin::IteratorStepValue,
                args: vec![iter_handle],
            },
        );
        let push_block = self.lower_value_exception_branch(loop_body, elem_val)?;
        self.current_function.append_instruction(
            push_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ArrayPush,
                args: vec![result_arr, elem_val],
            },
        );
        self.current_function
            .set_terminator(push_block, Terminator::Jump { target: header });

        // exit: 不做 IteratorClose——rest 收集只在 done 为 true 时结束，
        // 规范所有 IteratorClose 调用点都以 [[Done]] 为 false 为前提
        // （§13.15.5.2 步骤 6），耗尽后再调 return() 属可观察偏离；
        // 迭代器侧表项由 GC 存活扫描回收。

        // rest 目标可为嵌套解构模式（`[...[x, y]]` / `[...{ length }]`）：
        // 其绑定初始化产生的后继块必须回传，丢弃会让嵌套绑定代码悬挂在
        // 不可达分支、后续语句接回旧块（症状伪装成 TDZ / 全 undefined）。
        // rest 值是新建数组（永不 nullish），来源上下文惰性。
        let exit = self.lower_destructure_pattern(
            rest_pat,
            result_arr,
            exit,
            kind,
            &DestructureSource::NestedInArray,
        )?;

        Ok(self.resolve_store_block(exit))
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

        // then_block: 求值默认表达式。表达式可能在内部延续到新块（闭包物化、
        // 异常分叉等会发布 expr_merge_block 等延续），必须先 resolve 到真正的
        // 延续块再跳 merge——这同时消耗掉残留延续，防止其泄漏给调用方的
        // resolve_store_block 而把后续 store 误写进分支块（phi 支配性破坏）。
        // 默认值的 `? GetValue`（IteratorBindingInitialization 等）：表达式可
        // 抛时立即分叉传播，异常哨兵不得经 Phi 流入绑定值；数组解构场景下
        // 该分叉同时命中迭代器保护区的 IfAbruptCloseIterator。
        let mut then_current = then_block;
        let default_val = self.lower_call_operand_then_continue(default_expr, &mut then_current)?;
        let then_exit = self.resolve_store_block(then_current);
        self.current_function.set_terminator(
            then_exit,
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
                        predecessor: then_exit,
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
