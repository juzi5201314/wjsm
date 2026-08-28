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
                // NamedEvaluation（§14.3.1.2 / §14.3.2.1）：`let f = <匿名函数定义>`
                // 按绑定标识符命名；解构声明目标不触发。
                self.stage_named_eval_for_binding(&declarator.name, init);
                let value = self.lower_expr(init, block)?;
                block = self.resolve_store_block(block);
                // 初始化器位于声明语句的顶层表达式位置：若它可能返回 TAG_EXCEPTION
                // （调用 / 成员读取 / new / `in` 等），插入异常检查分叉，使
                // `let x = throws()` 之类在 try/catch 中可被捕获，而非令 TAG_EXCEPTION
                // 流入 StoreVar 后触发 native pending-exception invariant。
                if self.expr_can_throw(init) {
                    block = self.lower_value_exception_branch(block, value)?;
                }
                // 声明语句的 pattern 目标是 InitializeBinding：脚本全局词法名
                // 须经 GlobalEnvInitLex 解除 TDZ（区别于赋值的 SetMutableBinding）。
                let saved_decl_init = self.script_global_decl_init;
                self.script_global_decl_init = !matches!(kind, VarKind::Var);
                let destructured =
                    self.lower_destructure_pattern(&declarator.name, value, block, kind);
                self.script_global_decl_init = saved_decl_init;
                block = destructured?;
                // 若为简单 ident = new TypedArrayConstructor(...)，记录绑定类型
                if let swc_ast::Pat::Ident(binding) = &declarator.name {
                    let name = binding.id.sym.to_string();
                    if (is_array_constructor_expr(init)
                        || (is_array_from_of_call(init) && self.scopes.lookup("Array").is_err()))
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.array_bindings.insert((scope_id, name.clone()));
                    }
                    if is_typedarray_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.typedarray_bindings.insert((scope_id, name.clone()));
                    }
                    // 只登记 const：不可重新赋值，证明在整个作用域内恒成立。
                    if kind == crate::scope::VarKind::Const
                        && self.is_string_producing_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.string_bindings.insert((scope_id, name.clone()));
                    }
                    if kind == crate::scope::VarKind::Let
                        && self.is_string_producing_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.maybe_string_bindings.insert((scope_id, name.clone()));
                    }
                    if is_sharedarraybuffer_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.sab_bindings.insert((scope_id, name.clone()));
                    }
                    if is_dataview_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.dataview_bindings.insert((scope_id, name.clone()));
                    }
                    if is_map_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.map_bindings.insert((scope_id, name.clone()));
                    }
                    if is_set_constructor_expr(init)
                        && let Ok((scope_id, _)) = self.scopes.lookup(&name)
                    {
                        self.set_bindings.insert((scope_id, name.clone()));
                    }
                    // const + 字面量初始化 → 记录为可折叠常量（捕获读取直接内联）。
                    // 仅限简单 ident 绑定（非解构）与纯字面量（Num/Str/Bool/Null），
                    // 保证折叠值恒定且无副作用；TDZ 安全见 load_captured_binding 注释。
                    if matches!(kind, VarKind::Const) {
                        self.record_const_literal_binding(&name, init);
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
                let saved_decl_init = self.script_global_decl_init;
                self.script_global_decl_init = true;
                let destructured =
                    self.lower_destructure_pattern(&declarator.name, undef_val, block, kind);
                self.script_global_decl_init = saved_decl_init;
                block = destructured?;
            }
        }

        Ok(StmtFlow::Open(block))
    }

    /// 记录 `const X = <字面量>` 绑定（IR 名 → 常量池 id），供闭包捕获读取折叠。
    ///
    /// 仅接受纯字面量（Num/Str/Bool/Null）：值恒定、无副作用，折叠后语义不变。
    /// 键为 `$<scope_id>.<name>`（与 `CapturedBinding::var_ir_name` 同构），
    /// 不同作用域同名绑定互不干扰。
    fn record_const_literal_binding(&mut self, name: &str, init: &swc_ast::Expr) {
        let literal = match init {
            swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) => Constant::Number(num.value),
            swc_ast::Expr::Lit(swc_ast::Lit::Str(string)) => {
                Constant::String(string.value.to_string_lossy().into_owned())
            }
            swc_ast::Expr::Lit(swc_ast::Lit::Bool(b)) => Constant::Bool(b.value),
            swc_ast::Expr::Lit(swc_ast::Lit::Null(_)) => Constant::Null,
            _ => return,
        };
        // 绑定刚声明（destructure 已 mark_initialised）；resolve_scope_id 不做 TDZ 检查。
        let Ok(scope_id) = self.scopes.resolve_scope_id(name) else {
            return;
        };
        self.module_const_literals
            .insert(format!("${scope_id}.{name}"), literal);
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
            false,
        )
    }

    /// 普通（非 async / 非 generator）函数的参数构建：sloppy 简单参数列表
    /// 允许重复形参名（§15.2.1 早错误仅约束严格代码与非简单列表）。
    pub(crate) fn build_plain_function_param_ir_names(
        &mut self,
        function: &swc_ast::Function,
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        let allow_duplicates = !self.strict_mode
            && !function.body.as_ref().is_some_and(|body| {
                crate::lowerer_with::strict_check::stmts_have_use_strict(&body.stmts)
            })
            && function
                .params
                .iter()
                .all(|param| matches!(param.pat, swc_ast::Pat::Ident(_)));
        self.build_param_ir_names_impl(
            function
                .params
                .iter()
                .map(|p| &p.pat)
                .collect::<Vec<_>>()
                .as_slice(),
            env_scope_id,
            this_scope_id,
            allow_duplicates,
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
            false,
        )
    }

    pub(crate) fn build_param_ir_names_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
        allow_duplicates: bool,
    ) -> Result<Vec<String>, LoweringError> {
        let mut ir_names: Vec<String> = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        for (index, pat) in pats.iter().enumerate() {
            match pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    // sloppy 简单参数列表的重复形参（后者胜）：除最后一次出现
                    // 外重命名为临时槽——实参仍按位置写入各自槽位，函数体内的
                    // 名字解析到最后一次绑定，与规范"绑定被同名后参覆盖"一致。
                    let duplicated_later = allow_duplicates
                        && pats[index + 1..].iter().any(|later| {
                            matches!(later, swc_ast::Pat::Ident(other) if other.id.sym == binding.id.sym)
                        });
                    if duplicated_later {
                        let temp = self.alloc_temp_name();
                        let scope_id = self
                            .scopes
                            .declare(&temp, VarKind::Let, true)
                            .map_err(|msg| self.error(binding.span(), msg))?;
                        ir_names.push(format!("${scope_id}.{temp}"));
                        continue;
                    }
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
        self.emit_field_init_common(block, key_dest, init_value, is_private)
    }

    /// 公有实例字段（静态属性名）。计算键不走此路径：其键在类定义期求值一次
    /// 并经合成词法绑定传入构造器（见 `emit_instance_initializers`），此处
    /// `lower_prop_name` 只会发射 Const，不产生控制流。
    pub(crate) fn emit_field_init_with_key(
        &mut self,
        block: BasicBlockId,
        key: &swc_ast::PropName,
        init_value: Option<&swc_ast::Expr>,
    ) -> Result<BasicBlockId, LoweringError> {
        let key_dest = self.lower_prop_name(key, block)?;
        // NamedEvaluation（ClassFieldDefinitionEvaluation）：静态键字段的
        // 匿名函数定义初始化器按键名命名。
        if init_value.is_some_and(Self::is_anonymous_fn_definition)
            && let Some(name) = Self::static_prop_name_text(key)
        {
            self.named_eval_hint = Some(name);
        }
        self.emit_field_init_common(block, key_dest, init_value, false)
    }

    /// DefineField（ES §7.3.33）：初始化器求值异常先传播，随后
    /// CreateDataPropertyOrThrow 定义自有数据属性（原型链 setter 不触发）。
    /// 初始化器可含调用/控制流，块须线程化推进。
    pub(crate) fn emit_field_init_common(
        &mut self,
        block: BasicBlockId,
        key_dest: ValueId,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        // 调用方是否已按静态键 / 私有名暂存 NamedEvaluation 提示（提示在
        // 初始化器降级时被消费，须在此先行记录）。
        let staged_named_eval = self.named_eval_hint.is_some();
        let mut block = block;
        let init_val = self.lower_field_init_value(&mut block, init_value)?;
        // NamedEvaluation：计算键字段的匿名函数定义以实际键值运行时命名
        // （私有字段恒由调用方以 `#name` 提示命名，不走运行时键）。
        if !is_private
            && !staged_named_eval
            && init_value.is_some_and(Self::is_anonymous_fn_definition)
        {
            self.emit_runtime_set_function_name(block, init_val, key_dest, AccessorPrefix::None);
        }
        // 字段定义目标是当前绑定的 this：super() 返回对象重绑后即该对象。
        let this_val = self.emit_read_ctor_this(block);
        if is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![this_val, key_dest, init_val],
                },
            );
            return Ok(self.resolve_store_block(block));
        }
        let result = self.emit_create_data_property(block, this_val, key_dest, init_val);
        self.lower_value_exception_branch(block, result)
    }

    /// 字段初始化器求值：无初始化器时按规范取 undefined；有初始化器时求值并
    /// 推进 block，其异常在字段定义之前传播。
    pub(crate) fn lower_field_init_value(
        &mut self,
        block: &mut BasicBlockId,
        init_value: Option<&swc_ast::Expr>,
    ) -> Result<ValueId, LoweringError> {
        let Some(value) = init_value else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_dest = self.alloc_value();
            self.current_function.append_instruction(
                *block,
                Instruction::Const {
                    dest: ud_dest,
                    constant: ud_const,
                },
            );
            return Ok(ud_dest);
        };
        let value_dest = self.lower_expr_then_continue(value, block)?;
        if self.expr_can_throw(value) {
            *block = self.lower_value_exception_branch(*block, value_dest)?;
        }
        Ok(value_dest)
    }

    /// TypeScript 参数属性 `constructor(private x)`：把形参值写入 `this.<name>`。
    ///
    /// `fields` 的每项为 `(构造器形参绑定, 字段名)`。调用点必须保证 `this` 已
    /// 可用（派生类需在 `super()` 之后），且按 TS 语义排在实例字段初始化器
    /// 之前。形参读取与标识符读取同判定：属于本帧且未进共享 env 时直接
    /// LoadVar，否则（箭头 super() 站点、形参被闭包捕获）走捕获链读取。
    pub(crate) fn emit_param_prop_fields(
        &mut self,
        mut block: BasicBlockId,
        fields: &[(CapturedBinding, String)],
    ) -> Result<BasicBlockId, LoweringError> {
        for (binding, field_name) in fields {
            let value = if !self.binding_belongs_to_current_function(binding)
                || self.is_shared_binding(binding)
            {
                let value = self.load_captured_binding(block, binding)?;
                block = self.resolve_store_block(block);
                value
            } else {
                let value = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: value,
                        name: binding.var_ir_name(),
                    },
                );
                value
            };
            let this_val = self.emit_read_ctor_this(block);
            let key_const = self
                .module
                .add_constant(Constant::String(field_name.clone()));
            let key_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_dest,
                    constant: key_const,
                },
            );
            self.emit_set_prop(block, this_val, key_dest, value);
            block = self.resolve_store_block(block);
        }
        Ok(block)
    }

    pub(crate) fn emit_private_method_bind(
        &mut self,
        block: BasicBlockId,
        target_val: ValueId,
        field_name: &str,
        function_value: ValueId,
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
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::PrivateSet,
                args: vec![target_val, key_dest, function_value],
            },
        );
    }

    /// 在实例/构造器上绑定私有访问器槽（getter/setter 各可选）。
    pub(crate) fn emit_private_accessor_bind(
        &mut self,
        block: BasicBlockId,
        target_val: ValueId,
        field_name: &str,
        getter: Option<ValueId>,
        setter: Option<ValueId>,
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
        let undefined = self.module.add_constant(Constant::Undefined);
        let getter_value = if let Some(value) = getter {
            value
        } else {
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: value,
                    constant: undefined,
                },
            );
            value
        };
        let setter_value = if let Some(value) = setter {
            value
        } else {
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: value,
                    constant: undefined,
                },
            );
            value
        };
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::PrivateAccessorBind,
                args: vec![target_val, key_dest, getter_value, setter_value],
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
        // 入口即 take：即使后续任一守卫提前返回，也不会把过期的来源泄漏给
        // 之后降级的其他函数。
        let args_override = self.arguments_source_override.take();
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

        // generator/async 函数 body：wrapper 已在真实调用帧物化 arguments 对象并经
        // 续体槽位传入，这里直接绑定同一对象——每次调用恰好一个 arguments、
        // 携带真实实参，且 callee 指向用户可见的 wrapper 函数。
        //
        // [[ParameterMap]] 必须在 body 里重建：wrapper 的形参存储随 wrapper 帧退出即
        // 失效，body 每次 resume 从续体槽位恢复自己的形参副本，映射只有锚在 body 的
        // 共享 env 上才是活的。
        if let Some(source) = args_override {
            let store_block = self.resolve_store_block(block);
            self.current_function.append_instruction(
                store_block,
                Instruction::StoreVar {
                    name: ir_name,
                    value: source,
                },
            );
            if self.scopes.mark_initialised("arguments").is_err() {
                // 已初始化过，无需处理
            }
            // 映射不能就地建：此处仍在状态机 dispatch 之前，而 dispatch 会把
            // `$shared_env` 槽清成 undefined。改为登记待办，由调用方在 body 的
            // 用户代码入口块（每次 resume 都会重新走到的 state 分支）兑现。
            self.pending_arguments_param_map = Some(source);
            return Ok(self.resolve_store_block(block));
        }

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
        // ES §10.2.11 步骤 22：严格模式或非 simple parameter list 一律建 unmapped
        // arguments 对象（callee 为抛错访问器，且没有 [[ParameterMap]]）。
        let needs_mapped = !self.strict_mode
            && !self.is_arrow
            && !self.is_method
            && self.arguments_simple_params.is_some();

        let fn_name = self.current_function.name().to_string();
        let mapped_self_binding = if needs_mapped {
            self.scopes
                .lookup(&fn_name)
                .ok()
                .map(|(scope_id, _)| CapturedBinding::new(&fn_name, scope_id))
        } else {
            None
        };

        // D5: 精确发 Const — mapped && 无 binding → FunctionRef；mapped && 有 binding →
        // 从 env 读出闭包值。两种形态都作为 args[2] 传给 builtin，由 builtin 按
        // §10.2.1.1 的 `{[[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true}`
        // 定义 callee；早先的「先建对象再 SetProp 回填」会把 callee 建成可枚举属性。
        let func_ref_val = if needs_mapped {
            let val = self.alloc_value();
            if let Some(binding) = mapped_self_binding.as_ref() {
                let env_val = self.load_env_object(block);
                let env_key_val = self.append_env_key_const(block, binding);
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: val,
                        object: env_val,
                        key: env_key_val,
                    },
                );
            } else {
                let function_id = wjsm_ir::FunctionId(self.module.functions().len() as u32);
                let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: val,
                        constant: func_ref_const,
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

        if self.scopes.mark_initialised("arguments").is_err() {
            // Already initialised, that's fine
        }
        if needs_mapped {
            let after_map = self.emit_arguments_param_map(store_block, arguments_obj)?;
            return Ok(self.resolve_store_block(after_map));
        }
        Ok(self.resolve_store_block(block))
    }

    /// 兑现 generator/async body 延迟登记的 [[ParameterMap]]。调用点必须是状态机
    /// dispatch 之后的用户代码入口块，那里 `$shared_env` 才由 body 自己掌管。
    pub(crate) fn emit_pending_arguments_param_map(
        &mut self,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let Some(arguments_obj) = self.pending_arguments_param_map.take() else {
            return Ok(block);
        };
        self.emit_arguments_param_map(block, arguments_obj)
    }

    /// 为 mapped arguments 对象装上 [[ParameterMap]]（ES §10.4.4）。
    ///
    /// 形参与 `arguments[i]` 的双向 live binding 需要一个「调用帧内稳定、宿主可寻址」
    /// 的形参存储：这里把 simple parameter list 的形参一次性搬进本函数的共享 env
    /// 对象，此后函数体对形参的读写都走 env 的 GetProp/SetProp，宿主再把
    /// `arguments[i]` 建成指向同一 (env, key) 的访问器对。generator/async body 同样
    /// 命中此路径——它们的共享 env 跨 suspend 存活，映射因此在 resume 之间保持有效。
    fn emit_arguments_param_map(
        &mut self,
        block: BasicBlockId,
        arguments_obj: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        if self.strict_mode || self.is_arrow || self.is_method {
            return Ok(block);
        }
        let Some(names) = self.arguments_simple_params.clone() else {
            return Ok(block);
        };
        if names.is_empty() {
            return Ok(block);
        }
        // 形参绑定必须都解析得到，否则映射会指向错误的槽位——宁可不建映射。
        //
        // §10.4.4 CreateMappedArgumentsObject 步骤 21 自后向前扫描并维护 mappedNames：
        // 同名形参（`function f(a, a)`）只有最后一次出现进 parameter map，靠前的下标
        // 退化为普通数据属性。这里对靠前的重复项发 undefined 占位，宿主据此跳过。
        let mut mapped_names = std::collections::HashSet::new();
        let mut bindings = Vec::with_capacity(names.len());
        for name in names.iter().rev() {
            if !mapped_names.insert(name.as_str()) {
                bindings.push(None);
                continue;
            }
            let Ok((scope_id, _)) = self.scopes.lookup(name) else {
                return Ok(block);
            };
            bindings.push(Some(CapturedBinding::new(name, scope_id)));
        }
        bindings.reverse();
        let shared: Vec<CapturedBinding> = bindings.iter().flatten().cloned().collect();
        if shared.is_empty() {
            return Ok(block);
        }
        let span = swc_core::common::DUMMY_SP;
        let env_val = self.ensure_shared_env(block, &shared, span)?;
        let mut block = self.resolve_store_block(block);
        let mut args = Vec::with_capacity(bindings.len() + 2);
        args.push(arguments_obj);
        args.push(env_val);
        for binding in &bindings {
            args.push(match binding {
                Some(binding) => self.append_env_key_const(block, binding),
                None => self.alloc_undefined_value(block),
            });
        }
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::BindArgumentsParamMap,
                args,
            },
        );
        block = self.resolve_store_block(block);
        Ok(block)
    }
}
