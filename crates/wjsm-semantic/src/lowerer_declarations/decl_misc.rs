use super::*;

impl Lowerer {
    pub(crate) fn lower_debugger(&mut self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::Debugger,
                args: vec![],
            },
        );
        Ok(StmtFlow::Open(block))
    }

    pub(crate) fn lower_with(
        &self,
        _with_stmt: &swc_ast::WithStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let _block = self.ensure_open(flow)?;
        Err(self.error(
            _with_stmt.span(),
            "with statement is not supported in strict/static scope mode",
        ))
    }

    // ── Variable declarations ───────────────────────────────────────────────

    pub(crate) fn lower_var_decl(
        &mut self,
        var_decl: &swc_ast::VarDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let mut block = self.ensure_open(flow)?;
        let kind = match var_decl.kind {
            swc_ast::VarDeclKind::Var => VarKind::Var,
            swc_ast::VarDeclKind::Let => VarKind::Let,
            swc_ast::VarDeclKind::Const => VarKind::Const,
        };

        for declarator in &var_decl.decls {
            if let Some(init) = &declarator.init {
                let value = self.lower_expr(init, block)?;
                block = self.resolve_store_block(block);
                // 初始化器位于声明语句的顶层表达式位置：若它可能返回 TAG_EXCEPTION
                // （调用 / 成员读取 / new / `in` 等），插入异常检查分叉，使
                // `let x = throws()` 之类在 try/catch 中可被捕获，而非令 TAG_EXCEPTION
                // 流入 StoreVar 后触发 WASM unreachable。
                if self.expr_can_throw(init) && self.expr_exception_fork_allowed() {
                    block = self.lower_value_exception_branch(block, value)?;
                }
                block = self.lower_destructure_pattern(&declarator.name, value, block, kind)?;
                // 若为简单 ident = new TypedArrayConstructor(...)，记录绑定类型
                if let swc_ast::Pat::Ident(binding) = &declarator.name {
                    let name = binding.id.sym.to_string();
                    if is_array_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.array_bindings.insert((scope_id, name.clone()));
                    }
                    if is_typedarray_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.typedarray_bindings.insert((scope_id, name.clone()));
                    }
                    if is_sharedarraybuffer_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.sab_bindings.insert((scope_id, name.clone()));
                    }
                    if is_dataview_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.dataview_bindings.insert((scope_id, name));
                    }
                }
            } else {
                if matches!(kind, VarKind::Const) {
                    return Err(self.error(var_decl.span, "const declarations must be initialised"));
                }
                if matches!(kind, VarKind::Var) {
                    // var without init: already initialised in pre-scan, skip
                    let mut names = Vec::new();
                    Self::extract_pat_bindings(std::slice::from_ref(&declarator.name), &mut names);
                    for name in names {
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(var_decl.span, msg))?;
                    }
                    continue;
                }

                // `let x;`（非解构）或 `let [a, b];` — 初始化为 undefined
                let undef_cid = self.module.add_constant(Constant::Undefined);
                let undef_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_cid,
                    },
                );
                block = self.lower_destructure_pattern(&declarator.name, undef_val, block, kind)?;
            }
        }

        Ok(StmtFlow::Open(block))
    }

    // ── Destructuring pattern lowering ──────────────────────────────────────

    /// 构建函数参数的 param_ir_names 并声明变量。
    ///   - 简单参数 (x): 直接使用变量名
    ///   - 简单参数 + 默认值 (x = 1): 直接使用变量名
    ///   - 解构参数 ({a}) / 解构+默认值 ([a] = [1]): 使用临时变量名
    pub(crate) fn build_param_ir_names(
        &mut self,
        params: &[swc_ast::Param],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    /// 为箭头函数的参数（Vec<Pat>）构建 param_ir_names.
    pub(crate) fn build_arrow_param_ir_names(
        &mut self,
        params: &[swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    pub(crate) fn build_param_ir_names_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        let mut ir_names: Vec<String> = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        for pat in pats {
            match pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(binding.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{name}"));
                }
                swc_ast::Pat::Assign(assign) => match &*assign.left {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(binding.span(), msg))?;
                        ir_names.push(format!("${scope_id}.{name}"));
                    }
                    _ => {
                        let temp = self.alloc_temp_name();
                        let scope_id = self
                            .scopes
                            .declare(&temp, VarKind::Let, true)
                            .map_err(|msg| self.error(assign.span, msg))?;
                        ir_names.push(format!("${scope_id}.{temp}"));
                        let mut nested = Vec::new();
                        Self::extract_pat_bindings(&[*assign.left.clone()], &mut nested);
                        for n in &nested {
                            self.scopes
                                .declare(n, VarKind::Let, true)
                                .map_err(|msg| self.error(assign.span, msg))?;
                        }
                    }
                },
                swc_ast::Pat::Rest(rest) => {
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[*rest.arg.clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
                _ => {
                    let temp = self.alloc_temp_name();
                    let scope_id = self
                        .scopes
                        .declare(&temp, VarKind::Let, true)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{temp}"));
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[(*pat).clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
            }
        }

        Ok(ir_names)
    }

    /// 在函数体入口生成参数初始化代码（默认值 + 解构）。
    pub(crate) fn emit_param_inits(
        &mut self,
        params: &[swc_ast::Param],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    pub(crate) fn emit_arrow_param_inits(
        &mut self,
        pats: &[swc_ast::Pat],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            pats.iter().collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    pub(crate) fn emit_field_init(
        &mut self,
        block: BasicBlockId,
        this_scope_id: usize,
        field_name: &str,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        let key_const = self
            .module
            .add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        self.emit_field_init_common(block, this_scope_id, key_dest, init_value, is_private)
    }

    /// 公有实例字段（含计算属性名 `[expr]`）。
    pub(crate) fn emit_field_init_with_key(
        &mut self,
        block: BasicBlockId,
        this_scope_id: usize,
        key: &swc_ast::PropName,
        init_value: Option<&swc_ast::Expr>,
    ) -> Result<BasicBlockId, LoweringError> {
        let key_dest = self.lower_prop_name(key, block)?;
        self.emit_field_init_common(block, this_scope_id, key_dest, init_value, false)
    }

    fn emit_field_init_common(
        &mut self,
        block: BasicBlockId,
        this_scope_id: usize,
        key_dest: ValueId,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: this_val,
                name: format!("${this_scope_id}.$this"),
            },
        );
        let init_val = if let Some(value) = init_value {
            self.lower_expr(value, block)?
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: ud_dest,
                    constant: ud_const,
                },
            );
            ud_dest
        };
        if is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![this_val, key_dest, init_val],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: this_val,
                    key: key_dest,
                    value: init_val,
                },
            );
        }
        Ok(self.resolve_store_block(block))
    }

    pub(crate) fn emit_static_field_init(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        field_name: &str,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<(), LoweringError> {
        let key_const = self
            .module
            .add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        self.emit_static_field_init_common(block, ctor_dest, key_dest, init_value, is_private)
    }

    /// 公有静态字段（含计算属性名 `[expr]`）。
    pub(crate) fn emit_static_field_init_with_key(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        key: &swc_ast::PropName,
        init_value: Option<&swc_ast::Expr>,
    ) -> Result<(), LoweringError> {
        let key_dest = self.lower_prop_name(key, block)?;
        self.emit_static_field_init_common(block, ctor_dest, key_dest, init_value, false)
    }

    fn emit_static_field_init_common(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        key_dest: ValueId,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<(), LoweringError> {
        let init_val = if let Some(value) = init_value {
            self.lower_expr(value, block)?
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: ud_dest,
                    constant: ud_const,
                },
            );
            ud_dest
        };
        if is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![ctor_dest, key_dest, init_val],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: ctor_dest,
                    key: key_dest,
                    value: init_val,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn emit_private_method_bind(
        &mut self,
        block: BasicBlockId,
        target_val: ValueId,
        field_name: &str,
        func_id: FunctionId,
    ) {
        let key_const = self
            .module
            .add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        let fn_dest = self.alloc_value();
        let fn_ref_const = self.module.add_constant(Constant::FunctionRef(func_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: fn_dest,
                constant: fn_ref_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::PrivateSet,
                args: vec![target_val, key_dest, fn_dest],
            },
        );
    }

    /// 在实例/构造器上绑定私有访问器槽（getter/setter 各可选）。
    pub(crate) fn emit_private_accessor_bind(
        &mut self,
        block: BasicBlockId,
        target_val: ValueId,
        field_name: &str,
        getter_id: Option<FunctionId>,
        setter_id: Option<FunctionId>,
    ) {
        let key_const = self
            .module
            .add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let getter_dest = self.alloc_value();
        if let Some(gid) = getter_id {
            let fn_ref = self.module.add_constant(Constant::FunctionRef(gid));
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: getter_dest,
                    constant: fn_ref,
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: getter_dest,
                    constant: undef_const,
                },
            );
        }
        let setter_dest = self.alloc_value();
        if let Some(sid) = setter_id {
            let fn_ref = self.module.add_constant(Constant::FunctionRef(sid));
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: setter_dest,
                    constant: fn_ref,
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: setter_dest,
                    constant: undef_const,
                },
            );
        }
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::PrivateAccessorBind,
                args: vec![target_val, key_dest, getter_dest, setter_dest],
            },
        );
    }

    /// 发射隐式 `arguments` 对象的物化代码（`CollectRestArgs` + `CreateMappedArgumentsObject`）。
    ///
    /// `references_arguments`：调用方扫描函数 AST（形参 + 体，穿透嵌套箭头、止于嵌套普通函数）
    /// 得出的"是否引用 arguments"标志。为 `false` 时跳过物化——这是"arguments 惰性消除"优化的核心，
    /// 使未引用 `arguments` 的普通函数恢复为 no-GC（省去两条 may-GC 指令）。详见
    /// `Lowerer::function_references_arguments`。
    ///
    /// **正确性**：mapped arguments 仅可通过 `arguments` 绑定观测，无观测者 ⇒ 消除无行为差异。
    /// 若标志误判为 `false` 而体内实际引用了 `arguments`，后续 body 降级 `lookup("arguments")`
    /// 会失败并编译报错（fail-loud），不会静默错误编译。
    pub(crate) fn emit_arguments_init(
        &mut self,
        block: BasicBlockId,
        references_arguments: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        if self.scopes.current_function_has_param_arguments() {
            return Ok(block);
        }
        // 惰性消除：函数未引用 arguments → 不物化（恢复 no-GC）。
        if !references_arguments {
            return Ok(block);
        }
        let scope_id = match self.scopes.declare("arguments", VarKind::Let, true) {
            Ok(id) => {
                self.scopes
                    .set_implicit_arguments("arguments")
                    .map_err(|msg| LoweringError::Diagnostic(Diagnostic::new(0, 0, msg)))?;
                id
            }
            Err(_) => {
                if let Ok((sid, _)) = self.scopes.lookup("arguments") {
                    sid
                } else {
                    return Ok(block);
                }
            }
        };
        let ir_name = format!("${scope_id}.arguments");

        // 1) Collect all arguments into an array
        let args_array = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CollectRestArgs {
                dest: args_array,
                skip: 0,
            },
        );

        let param_count = self.arguments_param_count as f64;
        let param_count_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: param_count_val,
                constant: self.module.add_constant(Constant::Number(param_count)),
            },
        );

        let arguments_obj = self.alloc_value();
        let needs_mapped = !self.strict_mode && !self.is_arrow && !self.is_method;

        let fn_name = self.current_function.name().to_string();
        let mapped_self_binding = if needs_mapped {
            self.scopes
                .lookup(&fn_name)
                .ok()
                .map(|(scope_id, _)| CapturedBinding::new(&fn_name, scope_id))
        } else {
            None
        };

        // D5: 精确发 Const — mapped && 无 binding → FunctionRef；mapped && 有 binding → undefined；unmapped → 不发
        let func_ref_val = if needs_mapped {
            let val = self.alloc_value();
            if mapped_self_binding.is_none() {
                let function_id = wjsm_ir::FunctionId(self.module.functions().len() as u32);
                let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: val,
                        constant: func_ref_const,
                    },
                );
            } else {
                let undef_const = self.module.add_constant(Constant::Undefined);
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: val,
                        constant: undef_const,
                    },
                );
            }
            val
        } else {
            // unmapped 不吃 func_ref_val，但为保持签名一致仍传 undefined（IR 层不会读）
            let val = self.alloc_value();
            let undef_const = self.module.add_constant(Constant::Undefined);
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: undef_const,
                },
            );
            val
        };

        if needs_mapped {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(arguments_obj),
                    builtin: Builtin::CreateMappedArgumentsObject,
                    args: vec![args_array, param_count_val, func_ref_val],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(arguments_obj),
                    builtin: Builtin::CreateUnmappedArgumentsObject,
                    args: vec![args_array, param_count_val],
                },
            );
        }
        let store_block = self.resolve_store_block(block);
        self.current_function.append_instruction(
            store_block,
            Instruction::StoreVar {
                name: ir_name,
                value: arguments_obj,
            },
        );

        if let Some(binding) = mapped_self_binding {
            let patch_block = self.resolve_store_block(store_block);
            let env_val = self.load_env_object(patch_block);
            let env_key_val = self.append_env_key_const(patch_block, &binding);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                patch_block,
                Instruction::GetProp {
                    dest: closure_val,
                    object: env_val,
                    key: env_key_val,
                },
            );
            let callee_key = self.alloc_value();
            self.current_function.append_instruction(
                patch_block,
                Instruction::Const {
                    dest: callee_key,
                    constant: self
                        .module
                        .add_constant(Constant::String("callee".to_string())),
                },
            );
            self.current_function.append_instruction(
                patch_block,
                Instruction::SetProp {
                    object: arguments_obj,
                    key: callee_key,
                    value: closure_val,
                },
            );
            return Ok(self.resolve_store_block(patch_block));
        }

        if self.scopes.mark_initialised("arguments").is_err() {
            // Already initialised, that's fine
        }
        Ok(self.resolve_store_block(block))
    }
}
