use super::*;

impl Lowerer {
    /// 共享类体降级：构造器函数、原型对象、超类链、方法/访问器/静态块/字段、装饰器。
    ///
    /// 调用方负责类名词法作用域的 push/pop 与最终名字绑定。
    /// 返回 (最终 block, 构造器值)。
    pub(super) fn lower_class_body(
        &mut self,
        class_name: &str,
        class: &swc_ast::Class,
        class_span: Span,
        decorator_name: Option<&str>,
        block: BasicBlockId,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let constructor = class.body.iter().find_map(|member| match member {
            swc_ast::ClassMember::Constructor(c) => Some(c),
            _ => None,
        });

        self.push_class_private_name_scope(&class.body);
        let private_members = self.collect_class_private_members(class_name, &class.body)?;

        // ── 构造器 IR 函数 ──
        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(Self::PENDING_CTOR_FUNCTION_ID, false);
        self.super_call_allowed = class.super_class.is_some();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_span, msg))?;

        let mut param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        if let Some(ctor) = constructor {
            for param in &ctor.params {
                if let swc_ast::ParamOrTsParamProp::Param(p) = param
                    && let swc_ast::Pat::Ident(binding_ident) = &p.pat
                {
                    let name = binding_ident.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(class_span, msg))?;
                    param_ir_names.push(format!("${scope_id}.{name}"));
                }
            }
            if class.super_class.is_some()
                && let Some(body) = &ctor.body
                && let Some(span) = first_pre_super_this_or_super_span(body)
            {
                return Err(self.error(
                    span,
                    "derived constructor cannot access this or super before super()",
                ));
            }
            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        if constructor.is_none() && class.super_class.is_some() {
            let callee = self.alloc_value();
            self.current_function.append_instruction(
                field_block,
                Instruction::GetSuperConstructor { dest: callee },
            );
            let this_val = self.alloc_value();
            self.current_function.append_instruction(
                field_block,
                Instruction::LoadVar {
                    dest: this_val,
                    name: format!("${this_scope_id}.$this"),
                },
            );
            self.current_function.append_instruction(
                field_block,
                Instruction::SuperCall {
                    dest: None,
                    callee,
                    this_val,
                    args: Vec::new(),
                    forward_args: true,
                },
            );
            field_block = self.resolve_store_block(field_block);
        }
        let defer_instance_initializers = constructor.is_some() && class.super_class.is_some();
        if !defer_instance_initializers {
            field_block = self.emit_instance_initializers(
                field_block,
                this_scope_id,
                &class.body,
                &private_members,
            )?;
        }

        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if let Some(_ctor) = constructor {
            let ctor_params_len = constructor
                .map(|c| {
                    c.params
                        .iter()
                        .filter(|p| matches!(p, swc_ast::ParamOrTsParamProp::Param(_)))
                        .count()
                })
                .unwrap_or(0) as u32;
            self.arguments_param_count = ctor_params_len;
            let ctor_refs_args = constructor.is_some_and(Self::ctor_references_arguments);
            let args_block = self.emit_arguments_init(
                match inner_flow {
                    StmtFlow::Open(b) => b,
                    _ => entry,
                },
                ctor_refs_args,
            )?;
            self.eval_caller_has_arguments = if let Some(c) = constructor {
                c.params
                    .iter()
                    .filter_map(|p| match p {
                        swc_ast::ParamOrTsParamProp::Param(param) => Some(&param.pat),
                        _ => None,
                    })
                    .any(|pat| {
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(std::slice::from_ref(pat), &mut names);
                        names.iter().any(|n| n == "arguments")
                    })
                    || self.scopes.lookup("arguments").is_ok()
            } else {
                self.scopes.lookup("arguments").is_ok()
            };
            inner_flow = StmtFlow::Open(args_block);
        }
        if let Some(ctor) = constructor
            && let Some(body) = &ctor.body
        {
            let mut deferred_instance_initializers_emitted = false;
            for stmt in &body.stmts {
                // unreachable code 合法，跳过不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
                if defer_instance_initializers
                    && !deferred_instance_initializers_emitted
                    && stmt_is_direct_super_call(stmt)
                    && let StmtFlow::Open(b) = inner_flow
                {
                    inner_flow = StmtFlow::Open(self.emit_instance_initializers(
                        b,
                        this_scope_id,
                        &class.body,
                        &private_members,
                    )?);
                    deferred_instance_initializers_emitted = true;
                }
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) =
            self.span_to_source_span(constructor.map(|c| c.span()).unwrap_or_else(|| class_span))
        {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let ctor_captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&ctor_captured));
        for blk in blocks {
            ir_function.push_block(blk);
        }
        let ctor_function_id = self.module.push_function(ir_function);
        if let Some(function) = self.module.function_mut(ctor_function_id) {
            function.home_object = Some(HomeObject::Prototype(ctor_function_id));
        }
        self.patch_pending_ctor_home_object_references(ctor_function_id);
        self.patch_private_member_home_objects(ctor_function_id, &private_members);
        self.pop_function_context();

        // ── 物化构造器 + 创建原型 ──
        let ctor_dest = self.materialize_constructor_value(
            block,
            ctor_function_id,
            &ctor_captured,
            class_span,
        )?;

        let proto_dest = self.alloc_value();
        let method_count = class
            .body
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method)
                )
            })
            .count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        if let Some(super_class) = &class.super_class {
            let super_ctor = self.lower_expr(super_class, block)?;
            let proto_key_dest = self.emit_string_const(block, "prototype");
            let super_proto = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(super_proto),
                    builtin: Builtin::ReflectGet,
                    args: vec![super_ctor, proto_key_dest, super_ctor],
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProto {
                    object: proto_dest,
                    value: super_proto,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProto {
                    object: ctor_dest,
                    value: super_ctor,
                },
            );
            let super_key_dest = self.emit_string_const(block, "__super_constructor__");
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: ctor_dest,
                    key: super_key_dest,
                    value: super_ctor,
                },
            );
        }

        // ── 成员处理 ──
        let mut block = block;
        let mut static_init_idx = 0u32;
        for member in &class.body {
            match member {
                swc_ast::ClassMember::Method(method) => {
                    block = self.lower_class_method_member(
                        block,
                        method,
                        class_name,
                        ctor_function_id,
                        ctor_dest,
                        proto_dest,
                    )?;
                }
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    block = self.lower_class_static_block(
                        block,
                        static_block,
                        class_name,
                        ctor_function_id,
                        ctor_dest,
                        static_init_idx,
                    )?;
                    static_init_idx += 1;
                }
                swc_ast::ClassMember::PrivateProp(prop) if prop.is_static => {
                    let field_name =
                        self.resolve_private_storage_name(prop.key.name.as_ref(), prop.key.span)?;
                    self.emit_static_field_init(
                        block,
                        ctor_dest,
                        &field_name,
                        prop.value.as_deref(),
                        true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    self.emit_static_field_init_with_key(
                        block,
                        ctor_dest,
                        &prop.key,
                        prop.value.as_deref(),
                    )?;
                }
                swc_ast::ClassMember::Constructor(_) | swc_ast::ClassMember::PrivateMethod(_) => {}
                swc_ast::ClassMember::PrivateProp(p) if !p.is_static => {}
                swc_ast::ClassMember::ClassProp(p) if !p.is_static => {}
                other => {
                    return Err(self.error(
                        class_member_span(other),
                        format!(
                            "unsupported class member `{}` during class lowering",
                            class_member_kind(other),
                        ),
                    ));
                }
            }
        }

        // ── 后处理 ──
        self.emit_static_private_member_binds(block, ctor_dest, &private_members);

        let proto_key_dest = self.emit_string_const(block, "prototype");
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: ctor_dest,
                key: proto_key_dest,
                value: proto_dest,
            },
        );

        let (block, ctor_dest) =
            self.emit_apply_class_decorators(block, ctor_dest, &class.decorators, decorator_name)?;

        self.pop_class_private_name_scope();
        Ok((block, ctor_dest))
    }

    /// 处理单个类方法成员（Method / Getter / Setter）。
    fn lower_class_method_member(
        &mut self,
        mut block: BasicBlockId,
        method: &swc_ast::ClassMethod,
        class_name: &str,
        ctor_function_id: FunctionId,
        ctor_dest: ValueId,
        proto_dest: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let is_static = method.is_static;
        let target = if is_static { ctor_dest } else { proto_dest };
        let (method_name, m_key_dest) = self.lower_class_member_key(&method.key, block)?;

        match method.kind {
            swc_ast::MethodKind::Method => {
                if method.function.is_generator {
                    let mut method_value = self.lower_method_prop_to_fn(
                        &method.key,
                        &method.function,
                        Some(target),
                        block,
                    )?;
                    if !method.function.decorators.is_empty() {
                        (block, method_value) = self.emit_apply_value_decorators(
                            block,
                            method_value,
                            &ValueDecoratorContext {
                                decorators: &method.function.decorators,
                                kind: "method",
                                name: &method_name,
                                is_static,
                                is_private: false,
                            },
                        )?;
                    }
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: target,
                            key: m_key_dest,
                            value: method_value,
                        },
                    );
                    return Ok(block);
                }

                let fn_name = format!("{}.{}", class_name, method_name);
                let m_function_id = self.lower_class_method_fn(
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;

                let mut m_dest = self.emit_function_ref(block, m_function_id);
                if !method.function.decorators.is_empty() {
                    (block, m_dest) = self.emit_apply_value_decorators(
                        block,
                        m_dest,
                        &ValueDecoratorContext {
                            decorators: &method.function.decorators,
                            kind: "method",
                            name: &method_name,
                            is_static,
                            is_private: false,
                        },
                    )?;
                }
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: target,
                        key: m_key_dest,
                        value: m_dest,
                    },
                );
                Ok(block)
            }
            swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                    "get"
                } else {
                    "set"
                };
                let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                let m_function_id = self.lower_class_method_fn(
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;

                let mut fn_dest = self.emit_function_ref(block, m_function_id);
                if !method.function.decorators.is_empty() {
                    let kind = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                        "getter"
                    } else {
                        "setter"
                    };
                    (block, fn_dest) = self.emit_apply_value_decorators(
                        block,
                        fn_dest,
                        &ValueDecoratorContext {
                            decorators: &method.function.decorators,
                            kind,
                            name: &method_name,
                            is_static,
                            is_private: false,
                        },
                    )?;
                }
                let desc = self.build_descriptor(accessor, fn_dest, false, true, block)?;
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::DefineProperty,
                        args: vec![target, m_key_dest, desc],
                    },
                );
                Ok(block)
            }
        }
    }

    /// 处理类静态块成员：创建 IR 函数并在当前 block 发起调用。
    fn lower_class_static_block(
        &mut self,
        block: BasicBlockId,
        static_block: &swc_ast::StaticBlock,
        class_name: &str,
        ctor_function_id: FunctionId,
        ctor_dest: ValueId,
        idx: u32,
    ) -> Result<BasicBlockId, LoweringError> {
        let fn_name = format!("{}.static_init_{}", class_name, idx);
        let m_function_id = self.lower_class_static_block_fn(
            &fn_name,
            &static_block.body,
            static_block.span,
            ctor_function_id,
        )?;

        let fn_dest = self.emit_function_ref(block, m_function_id);
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: None,
                callee: fn_dest,
                this_val: ctor_dest,
                args: vec![],
            },
        );
        Ok(block)
    }

    /// 为类方法/访问器创建 IR 函数并注册到模块。返回 FunctionId。
    fn lower_class_method_fn(
        &mut self,
        fn_name: &str,
        function: &swc_ast::Function,
        method_span: Span,
        ctor_function_id: FunctionId,
        is_static: bool,
    ) -> Result<FunctionId, LoweringError> {
        self.push_function_context(fn_name, BasicBlockId(0));
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(ctor_function_id, is_static);

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(method_span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(method_span, msg))?;

        let mut param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        for param in &function.params {
            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                let name = binding_ident.id.sym.to_string();
                let scope_id = self
                    .scopes
                    .declare(&name, VarKind::Let, true)
                    .map_err(|msg| self.error(method_span, msg))?;
                param_ir_names.push(format!("${scope_id}.{name}"));
            }
        }

        if let Some(body) = &function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);
        self.arguments_param_count = Self::count_regular_params(&function.params);
        let m_entry =
            self.emit_arguments_init(m_entry, Self::function_needs_arguments_object(function))?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&function.params)
            || self.scopes.lookup("arguments").is_ok();

        let mut m_flow = StmtFlow::Open(m_entry);
        if let Some(body) = &function.body {
            for stmt in &body.stmts {
                if matches!(m_flow, StmtFlow::Terminated) {
                    continue;
                }
                m_flow = self.lower_stmt(stmt, m_flow)?;
            }
        }

        if let StmtFlow::Open(b) = m_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let home_object = if is_static {
            HomeObject::Constructor(ctor_function_id)
        } else {
            HomeObject::Prototype(ctor_function_id)
        };
        let m_function_id =
            self.finalize_class_method_function(fn_name, method_span, param_ir_names, home_object);
        self.pop_function_context();
        Ok(m_function_id)
    }

    /// 为类静态块创建 IR 函数并注册到模块。返回 FunctionId。
    fn lower_class_static_block_fn(
        &mut self,
        fn_name: &str,
        body: &swc_ast::BlockStmt,
        span: Span,
        ctor_function_id: FunctionId,
    ) -> Result<FunctionId, LoweringError> {
        self.push_function_context(fn_name, BasicBlockId(0));
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(ctor_function_id, true);

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);
        self.arguments_param_count = 0;
        let m_entry = self.emit_arguments_init(m_entry, Self::body_references_arguments(body))?;
        self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

        let mut m_flow = StmtFlow::Open(m_entry);
        for stmt in &body.stmts {
            if matches!(m_flow, StmtFlow::Terminated) {
                continue;
            }
            m_flow = self.lower_stmt(stmt, m_flow)?;
        }

        if let StmtFlow::Open(b) = m_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let m_function_id = self.finalize_class_method_function(
            fn_name,
            span,
            param_ir_names,
            HomeObject::Constructor(ctor_function_id),
        );
        self.pop_function_context();
        Ok(m_function_id)
    }

    /// 收尾方法 IR 函数：提取 blocks、设置元数据、注册到模块。返回 FunctionId。
    fn finalize_class_method_function(
        &mut self,
        fn_name: &str,
        span: Span,
        param_ir_names: Vec<String>,
        home_object: HomeObject,
    ) -> FunctionId {
        let m_old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let m_has_eval = m_old_fn.has_eval();
        let m_blocks = m_old_fn.into_blocks();
        let mut m_ir_function = Function::new(fn_name, BasicBlockId(0));
        m_ir_function.set_has_eval(m_has_eval);
        if let Some(src) = self.span_to_source_span(span) {
            m_ir_function.set_source_span(src);
        }
        m_ir_function.set_params(param_ir_names);
        let m_captured = self.captured_names_stack.last().unwrap().clone();
        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
        m_ir_function.home_object = Some(home_object);
        for b in m_blocks {
            m_ir_function.push_block(b);
        }
        self.module.push_function(m_ir_function)
    }

    /// 物化 FunctionRef 常量为运行时值。
    fn emit_function_ref(&mut self, block: BasicBlockId, function_id: FunctionId) -> ValueId {
        let ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: ref_const,
            },
        );
        dest
    }

    /// 提取类成员键（方法/访问器）：返回 (名称字符串, 运行时 key value)。
    /// 支持 Ident / Str / Computed 三种键类型。
    fn lower_class_member_key(
        &mut self,
        key: &swc_ast::PropName,
        block: BasicBlockId,
    ) -> Result<(String, ValueId), LoweringError> {
        match key {
            swc_ast::PropName::Ident(ident) => {
                let name = ident.sym.to_string();
                Ok((name, self.emit_string_const(block, ident.sym.as_ref())))
            }
            swc_ast::PropName::Str(s) => {
                let name = s.value.to_string_lossy().into_owned();
                let key_dest = self.emit_string_const(block, &name);
                Ok((name, key_dest))
            }
            swc_ast::PropName::Computed(_) => {
                let key_dest = self.lower_prop_name(key, block)?;
                Ok(("<computed>".to_string(), key_dest))
            }
            other => Err(self.error(other.span(), "unsupported property key kind")),
        }
    }
}
