use super::*;

impl Lowerer {
    pub(crate) fn append_eval_var_leak_if_needed(
        &mut self,
        name: &str,
        kind: VarKind,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // 只有声明于 eval 顶层变量环境的 var 绑定才写回调用方作用域记录
        // （§19.2.1.3 EvalDeclarationInstantiation 的 varEnv 是 eval 的变量
        // 环境）。eval 代码内定义的函数（含箭头、类静态块）自带
        // VariableEnvironment，其内部声明的 var 是函数局部绑定，绝不能外泄；
        // 而嵌套函数对 eval 顶层 var 的闭包赋值仍须回写，保持记录与静态
        // 绑定同步。
        if self.eval_var_writes_to_scope
            && matches!(kind, VarKind::Var)
            && self.eval_binding_is_top_level(name)
        {
            return self.append_eval_env_write(name, value, block);
        }
        Ok(block)
    }

    /// 绑定是否声明于 eval 顶层（不在任何嵌套函数上下文内）。
    fn eval_binding_is_top_level(&self, name: &str) -> bool {
        let Some(outer_fn_scope) = self.function_scope_id_stack.first().copied() else {
            // 尚未进入任何嵌套函数：声明/写点即 eval 顶层。
            return true;
        };
        match self.scopes.resolve_scope_id(name) {
            Ok(scope_id) => {
                scope_id != outer_fn_scope
                    && !self.scopes.is_strict_ancestor(outer_fn_scope, scope_id)
            }
            Err(_) => false,
        }
    }

    /// 标识符读取入口：解析穿越 with 作用域时先走对象环境记录动态分派
    /// （§9.1.1.2.1），全链未命中回退静态解析；无 with 时零成本直达静态路径。
    pub(crate) fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
        if !crossed.is_empty() {
            return self.lower_with_ident_read(ident, &crossed, block);
        }
        self.lower_ident_static(ident, block)
    }

    pub(crate) fn lower_ident_static(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let module_id = self.current_module_id;

        // 命名空间局部（`import * as ns`）按 (导入方模块, local) 查找，避免跨模块同名覆盖（#44）。
        if let Some(mid) = module_id
            && let Some(ns_obj) = self
                .static_namespace_import_objects
                .get(&(mid, name.clone()))
                .copied()
        {
            return Ok(ns_obj);
        }

        // 命名导入别名按 (导入方模块, local) 查找。读取时复用 lower_ident 对捕获/共享 env
        // 的同一套判定：仅当绑定逃逸出当前函数或已进入共享 env 时才走 env 取值路径，
        // 否则直接 LoadVar。这样既保证被改写导出对导入方可见（live binding，#45），
        // 又不会在共享 env 从未创建时误读未初始化槽。
        // 循环导入下源绑定可能仍处 TDZ（声明晚于本读取降级）：live binding 读取
        // 须经 TdzCheck，声明执行前访问按规范抛 ReferenceError。
        if let Some(mid) = module_id
            && let Some(alias_ir_name) = self.import_aliases.get(&(mid, name.clone())).cloned()
        {
            let binding = crate::lowerer_modules::parse_ir_name_to_binding(&alias_ir_name);
            let in_tdz = self.binding_in_tdz(&binding);
            if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                let value = self.load_captured_binding(block, &binding)?;
                if in_tdz {
                    let current_block = self.resolve_store_block(block);
                    let (checked, continue_block) =
                        self.emit_tdz_check(current_block, value, &name)?;
                    self.expr_merge_block = Some(continue_block);
                    return Ok(checked);
                }
                return Ok(value);
            }
            if in_tdz {
                // 捆绑后模块顶层直线读取循环导入的未初始化绑定：捆绑执行顺序
                // 等于降级顺序，读取必然先于声明执行，发射哨兵 + TdzCheck。
                let sentinel = self.module.add_constant(Constant::Uninitialized);
                let sentinel_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: sentinel_val,
                        constant: sentinel,
                    },
                );
                let (checked, continue_block) = self.emit_tdz_check(block, sentinel_val, &name)?;
                self.expr_merge_block = Some(continue_block);
                return Ok(checked);
            }
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest,
                    name: alias_ir_name,
                },
            );
            return Ok(dest);
        }

        // 脚本全局绑定：读经 GlobalEnvGet（声明式记录 TDZ → 全局对象属性），
        // 宿主全局环境记录是唯一权威（间接 eval / vm / Function 共享真值）。
        if self.script_global_kind_for(&name).is_some() {
            return self.lower_script_global_read(block, &name, false);
        }

        // eval 作用域桥接优先：自由变量（含 builtin / eval 标识符）走 EvalGetBinding，
        // 由 runtime 经 sandbox → realm.global 原型链解析（多 realm 正确性）。
        if self.eval_scope_bridge_active() && self.scopes.lookup(&name).is_err() {
            return self.lower_eval_env_read(&name, block);
        }

        if name == "eval" && self.scopes.lookup("eval").is_err() {
            let constant = self.module.add_constant(Constant::NativeCallableEval);
            let dest = self.alloc_value();
            self.current_function
                .append_instruction(block, Instruction::Const { dest, constant });
            return Ok(dest);
        }

        let (scope_id, _kind) = match self.lookup_binding_for_read(&name) {
            Ok(found) => found,
            Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
                // 变量查找失败 → 从全局对象按名读取属性
                // 全局对象已在模块初始化阶段通过 CreateGlobalObject 创建并存入 $0.$global
                let global_obj = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: global_obj,
                        name: "$0.$global".to_string(),
                    },
                );
                let key_const = self.module.add_constant(Constant::String(name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: global_obj,
                        key: key_val,
                    },
                );
                return Ok(dest);
            }
            Err(msg) => {
                // 跨函数前向引用（延迟执行的函数体读取后声明的 let/const/class）：
                // 静态无法判定调用是否先于声明执行，改为运行时 TdzCheck。
                if let Some((scope_id, _)) = self.runtime_tdz_binding(&name) {
                    return self.lower_tdz_checked_read(block, &name, scope_id);
                }
                // 脚本模式：未声明名延迟到运行时全局解析（eval/vm 可能已创建
                // 隐式全局；确实缺失时由宿主抛 "x is not defined"）。
                if msg.starts_with("undeclared identifier")
                    && self.script_global_dynamic_free_name(&name)
                {
                    return self.lower_script_global_read(block, &name, false);
                }
                return Err(self.error(ident.span, msg));
            }
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if self.iteration_env_for_binding(&binding).is_some() {
            return Ok(self.load_iteration_binding(block, &binding));
        }
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            return self.load_captured_binding(block, &binding);
        }

        // 局部变量：直接 LoadVar
        let ir_name = format!("${scope_id}.{name}");
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: ir_name,
            },
        );
        Ok(dest)
    }

    // ── Assignments ─────────────────────────────────────────────────────────

    pub(crate) fn lower_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // NamedEvaluation（§13.15.2）：`x = <匿名函数定义>`（含 &&= / ||= / ??=
        // 的赋值分支）按目标标识符命名；成员 / 解构目标与算术复合赋值不触发。
        // 提示由右值降级时的 `lower_expr` 入口消费，覆盖所有下游赋值路径
        // （本地 / 捕获 / eval env / with）。
        if let swc_ast::AssignTarget::Simple(swc_ast::SimpleAssignTarget::Ident(target)) =
            &assign.left
            && matches!(
                assign.op,
                swc_ast::AssignOp::Assign
                    | swc_ast::AssignOp::AndAssign
                    | swc_ast::AssignOp::OrAssign
                    | swc_ast::AssignOp::NullishAssign
            )
            && Self::is_anonymous_fn_definition(assign.right.as_ref())
        {
            self.named_eval_hint = Some(target.id.sym.to_string());
        }
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left
            && let swc_ast::SimpleAssignTarget::SuperProp(super_prop) = simple
        {
            return self.lower_assign_super_prop(assign, block, super_prop);
        }

        // Handle member expression assignment targets (e.g. obj.prop = value).
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left
            && let swc_ast::SimpleAssignTarget::Member(member_expr) = simple
        {
            let mut current_block = block;
            let obj_val = self.lower_expr_then_continue(&member_expr.obj, &mut current_block)?;
            let key = match &member_expr.prop {
                swc_ast::MemberProp::Ident(ident) => {
                    let name = ident.sym.to_string();
                    // __proto__ 赋值是 Object.setPrototypeOf 的语法糖（spec:
                    // __proto__ 是 Object.prototype 上的 accessor，setter 调用
                    // setPrototypeOf）。直接发射 CallBuiltin(ObjectSetPrototypeOf)
                    // 而非 SetProp，确保原型真正被设置（含循环检测、可扩展性检查）。
                    if name == "__proto__" && assign.op == swc_ast::AssignOp::Assign {
                        let value_val = self
                            .lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::ObjectSetPrototypeOf,
                                args: vec![obj_val, value_val],
                            },
                        );
                        let continue_block =
                            self.lower_value_exception_branch(current_block, dest)?;
                        self.expr_merge_block = Some(continue_block);
                        return Ok(value_val);
                    }
                    let key_const = self.module.add_constant(Constant::String(name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    key_dest
                }
                swc_ast::MemberProp::Computed(computed) => {
                    self.lower_expr_then_continue(&computed.expr, &mut current_block)?
                }
                swc_ast::MemberProp::PrivateName(name) => {
                    let field_name =
                        self.resolve_private_storage_name(name.name.as_ref(), name.span)?;
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    if assign.op == swc_ast::AssignOp::Assign {
                        let value_val = self
                            .lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::PrivateSet,
                                args: vec![obj_val, key_dest, value_val],
                            },
                        );
                        self.expr_merge_block = Some(current_block);
                        return Ok(value_val);
                    }
                    let old_val = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(old_val),
                            builtin: Builtin::PrivateGet,
                            args: vec![obj_val, key_dest],
                        },
                    );
                    let rhs_val =
                        self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                    let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                        self.error(assign.span, "unsupported compound assignment operator")
                    })?;
                    let result = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Binary {
                            dest: result,
                            op: bin_op,
                            lhs: old_val,
                            rhs: rhs_val,
                        },
                    );
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::PrivateSet,
                            args: vec![obj_val, key_dest, result],
                        },
                    );
                    self.expr_merge_block = Some(current_block);
                    return Ok(result);
                }
            };

            let is_computed = matches!(&member_expr.prop, swc_ast::MemberProp::Computed(_));
            if assign.op == swc_ast::AssignOp::Assign {
                // 简单赋值: obj.x = value 或 arr[computed] = value
                let value_val =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let result = match &member_expr.prop {
                    swc_ast::MemberProp::Computed(_) => {
                        self.emit_set_elem(current_block, obj_val, key, value_val)
                    }
                    _ => self.emit_set_prop(current_block, obj_val, key, value_val),
                };
                self.expr_merge_block = Some(current_block);
                return Ok(result);
            }

            // 逻辑复合赋值需要短路求值，走专用路径
            if matches!(
                assign.op,
                swc_ast::AssignOp::AndAssign
                    | swc_ast::AssignOp::OrAssign
                    | swc_ast::AssignOp::NullishAssign
            ) {
                return self.lower_logical_assign_member(assign, current_block, obj_val, key);
            }

            // 算术/位运算复合赋值
            let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                self.error(assign.span, "unsupported compound assignment operator")
            })?;

            // 用 GetElem/GetProp 读取当前值（取决于是否为 computed 成员）
            let loaded = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                if is_computed {
                    Instruction::GetElem {
                        dest: loaded,
                        object: obj_val,
                        index: key,
                    }
                } else {
                    Instruction::GetProp {
                        dest: loaded,
                        object: obj_val,
                        key,
                    }
                },
            );

            let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Binary {
                    dest,
                    op: bin_op,
                    lhs: loaded,
                    rhs,
                },
            );

            let result = if is_computed {
                self.emit_set_elem(current_block, obj_val, key, dest)
            } else {
                self.emit_set_prop(current_block, obj_val, key, dest)
            };
            self.expr_merge_block = Some(current_block);

            return Ok(result);
        }

        let name = match &assign.left {
            swc_ast::AssignTarget::Simple(simple) => match simple {
                swc_ast::SimpleAssignTarget::Ident(binding_ident) => {
                    // 解析穿越 with 作用域：赋值须按对象环境记录动态分派。
                    let crossed = self.with_scopes_for_ident(binding_ident.id.sym.as_ref());
                    if !crossed.is_empty() {
                        return self.lower_with_ident_assign(
                            assign,
                            &binding_ident.id,
                            &crossed,
                            block,
                        );
                    }
                    binding_ident.id.sym.to_string()
                }
                _ => {
                    return Err(self.error(
                        assign.left.span(),
                        "only simple identifier assignment targets are supported",
                    ));
                }
            },
            swc_ast::AssignTarget::Pat(pat) => {
                if assign.op != swc_ast::AssignOp::Assign {
                    return Err(self.error(
                        assign.span,
                        "compound assignment with destructuring is not supported",
                    ));
                }
                let mut current_block = block;
                let value =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let ir_pat = swc_ast::Pat::from(pat.clone());
                let continuation =
                    self.lower_destructure_pattern(&ir_pat, value, current_block, VarKind::Let)?;
                self.expr_merge_block = Some(continuation);
                return Ok(value);
            }
        };

        // 脚本全局绑定：写经 GlobalEnvSet（TDZ / const TypeError / strict 未
        // 解析名 ReferenceError 均为运行时语义，绕过编译期 const/TDZ 拒绝）。
        if self.script_global_kind_for(&name).is_some() {
            return self.lower_assign_script_global(assign, block, &name);
        }

        // 性能优化：使用 lookup_for_assign 一次遍历完成 const 检查 + TDZ 检查 + scope 解析，
        // 避免独立的 const 检查与 lookup 各自遍历 scope chain 的冗余。
        let (scope_id, kind) = match self.lookup_binding_for_assign(&name) {
            Ok(found) => found,
            Err(msg)
                if self.script_mode
                    && !self.eval_scope_bridge_active()
                    && msg.starts_with("undeclared identifier") =>
            {
                // 脚本模式未声明名赋值：sloppy 创建隐式全局属性，strict 由宿主
                // 在运行时抛 ReferenceError（含 builtin 全局名的重写）。
                return self.lower_assign_script_global(assign, block, &name);
            }
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                if assign.op == swc_ast::AssignOp::Assign && is_builtin_global(&name) {
                    // 对 builtin global 的赋值 → 写入 $0.$global 属性
                    let global_obj = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: global_obj,
                            name: "$0.$global".to_string(),
                        },
                    );
                    let key_const = self.module.add_constant(Constant::String(name));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                    let result = self.emit_set_prop(block, global_obj, key_val, rhs);
                    return Ok(result);
                }
                if self.strict_mode && !self.eval_scope_bridge_active() {
                    // strict script/module: 对未声明变量赋值 → ReferenceError。
                    // 右值不再降级，清除已暂存的 NamedEvaluation 提示防止泄漏。
                    self.named_eval_hint = None;
                    let msg_const = self.module.add_constant(Constant::String(format!(
                        "assignment to undeclared variable '{name}'"
                    )));
                    let msg_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: msg_val,
                            constant: msg_const,
                        },
                    );
                    let error_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(error_val),
                            builtin: Builtin::ReferenceErrorConstructor,
                            args: vec![msg_val],
                        },
                    );
                    // 创建 dummy 值（在 throw 终止块之前分配）
                    let dummy = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: dummy,
                            constant: self.module.add_constant(Constant::Undefined),
                        },
                    );
                    self.emit_throw_value(block, error_val)?;
                    // emit_throw_value 已终止块；返回的 dummy 不会被使用
                    return Ok(dummy);
                }
                return self.lower_assign_eval_env(assign, block, &name);
            }
            Err(msg) => {
                // 跨函数前向赋值：写入前经 TdzCheck 校验绑定已初始化
                //（const 的重赋值错误在 lookup_for_assign 中先行返回，不会到达此处）。
                if let Some((scope_id, kind)) = self.runtime_tdz_binding(&name)
                    && !matches!(kind, VarKind::Const)
                {
                    let binding = CapturedBinding::new(name.clone(), scope_id);
                    return self.lower_assign_captured_checked(assign, block, &binding, true);
                }
                return Err(self.error(assign.span, msg));
            }
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if !self.binding_belongs_to_current_function(&binding)
            || self.is_shared_binding(&binding)
            || self.iteration_env_for_binding(&binding).is_some()
        {
            return self.lower_assign_captured(assign, block, &binding);
        }

        let ir_name = format!("${scope_id}.{name}");

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                // RHS 可能通过闭包捕获把本绑定标记为 shared；必须同步写 shared_env。
                // 否则 `let o; o = new Foo(() => o)` 只 StoreVar，闭包读到的仍是 undefined。
                let store_block =
                    self.store_binding_value(block, &binding, rhs, assign.span, true)?;
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, rhs, store_block)?;
                // 钉死后续语句入口，避免 new 的 continue 块与 Jump 把 store 落到死块。
                self.expr_merge_block = Some(after_write_block);
                // 更新 Array 绑定跟踪：arr = [...] / new Array(...) / Array.from/of -> 标记；
                // arr = 其他 -> 取消标记。
                if is_array_constructor_expr(assign.right.as_ref())
                    || (is_array_from_of_call(assign.right.as_ref())
                        && self.scopes.lookup("Array").is_err())
                {
                    self.array_bindings.insert((scope_id, name.clone()));
                } else {
                    self.array_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 TypedArray 绑定跟踪：arr = new Int32Array -> 标记；arr = 其他 -> 取消标记
                if is_typedarray_constructor_expr(assign.right.as_ref()) {
                    self.typedarray_bindings.insert((scope_id, name.clone()));
                } else {
                    self.typedarray_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 SharedArrayBuffer 绑定跟踪（与 TypedArray 平行）
                if is_sharedarraybuffer_constructor_expr(assign.right.as_ref()) {
                    self.sab_bindings.insert((scope_id, name.clone()));
                } else {
                    self.sab_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 DataView 绑定跟踪（专用宿主导入调用约定）。
                if is_dataview_constructor_expr(assign.right.as_ref()) {
                    self.dataview_bindings.insert((scope_id, name.clone()));
                } else {
                    self.dataview_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 Map/Set 绑定跟踪（原型方法直连优化）。
                if is_map_constructor_expr(assign.right.as_ref()) {
                    self.map_bindings.insert((scope_id, name.clone()));
                } else {
                    self.map_bindings.remove(&(scope_id, name.clone()));
                }
                if is_set_constructor_expr(assign.right.as_ref()) {
                    self.set_bindings.insert((scope_id, name.clone()));
                } else {
                    self.set_bindings.remove(&(scope_id, name.clone()));
                }
                Ok(rhs)
            }
            op => {
                // 逻辑复合赋值需要短路求值，走专用路径
                if matches!(
                    op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self.lower_logical_assign(assign, block, ir_name);
                }

                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: loaded,
                        name: ir_name.clone(),
                    },
                );

                // RHS 可能通过构造器/new 内联分裂块，lower_expr_then_continue 会自动
                // 追踪所有续接块并更新 current_block，确保后续的 Binary + StoreVar
                // 插入到 RHS 求值完成后的正确续接块，而非被分裂前的原始块。
                let mut current_block = block;
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;

                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );

                self.current_function.append_instruction(
                    current_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: dest,
                    },
                );
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, dest, current_block)?;
                // 钉死后续语句入口（与简单赋值路径一致）；无论 current_block 是否
                // 与原始 block 相同，调用方都需要知道最终插入点，否则循环的步进指令
                // 会落回原始 block，导致步进与累加之间的 CFG 边缺失。
                self.expr_merge_block = Some(after_write_block);

                Ok(dest)
            }
        }
    }

    pub(crate) fn store_binding_value(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
        value: ValueId,
        span: Span,
        _sync_existing_env: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        // 脚本全局绑定没有 `$0.*` 槽：一切写入按 SetMutableBinding 路由到
        // 宿主全局环境记录（声明初始化已由调用方经 GlobalEnvInitLex 分流）。
        if binding.scope_id == Some(0) && self.script_global_names.contains_key(&binding.name) {
            return self.emit_script_global_set(block, &binding.name, value);
        }
        let mut store_block = self.resolve_store_block(block);
        self.current_function.append_instruction(
            store_block,
            Instruction::StoreVar {
                name: binding.var_ir_name(),
                value,
            },
        );
        if self.iteration_env_for_binding(binding).is_some() {
            let env = self.load_iteration_env_for_binding(store_block, binding);
            let key = self.append_env_key_const(store_block, binding);
            self.emit_set_prop(store_block, env, key, value);
        } else if self.is_shared_binding(binding) {
            let env_val =
                self.ensure_shared_env(store_block, std::slice::from_ref(binding), span)?;
            store_block = self.resolve_store_block(store_block);
            let key_val = self.append_env_key_const(store_block, binding);
            self.emit_set_prop(store_block, env_val, key_val, value);
        }
        Ok(store_block)
    }
}
