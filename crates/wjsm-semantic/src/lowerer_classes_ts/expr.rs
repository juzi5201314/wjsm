use super::*;

impl Lowerer {
    pub(crate) fn lower_class_expr(
        &mut self,
        class_expr: &swc_ast::ClassExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 类表达式可选名称（匿名类表达式无名称）
        let class_name = class_expr
            .ident
            .as_ref()
            .map(|id| id.sym.to_string())
            .unwrap_or_else(|| format!("anon_class_{}", self.anon_counter));
        if class_expr.ident.is_none() {
            self.anon_counter += 1;
        }

        // Named class expression: bind the name only inside the class body (block scope).
        let class_body_name = class_expr.ident.as_ref().map(|id| id.sym.to_string());
        let class_body_name_scope = if let Some(ref name) = class_body_name {
            self.scopes.push_scope(ScopeKind::Block);
            let scope_id = self
                .scopes
                .declare(name, VarKind::Const, false)
                .map_err(|msg| self.error(class_expr.span(), msg))?;
            Some((name.clone(), scope_id))
        } else {
            None
        };

        // 查找构造函数
        let constructor = class_expr
            .class
            .body
            .iter()
            .find_map(|member| match member {
                swc_ast::ClassMember::Constructor(c) => Some(c),
                _ => None,
            });

        let mut private_method_ids: Vec<(String, bool, FunctionId)> = Vec::new();
        for member in &class_expr.class.body {
            if let swc_ast::ClassMember::PrivateMethod(pm) = member {
                let field_name = format!("#{}", pm.key.name);
                let is_static = pm.is_static;
                let fn_name = if is_static {
                    format!("{}.static_{}", class_name, pm.key.name)
                } else {
                    format!("{}.{}", class_name, pm.key.name)
                };

                self.push_function_context(&fn_name, BasicBlockId(0));
                self.is_method = true;
                self.super_allowed = true;
                self.set_lexical_home_object_for_enclosing_method(
                    Self::PENDING_CTOR_FUNCTION_ID,
                    is_static,
                );
                let env_scope_id = self
                    .scopes
                    .declare("$env", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let this_scope_id = self
                    .scopes
                    .declare("$this", VarKind::Let, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
                let mut param_ir_names = vec![
                    format!("${env_scope_id}.$env"),
                    format!("${this_scope_id}.$this"),
                ];
                for param in &pm.function.params {
                    if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(pm.span, msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
                if let Some(body) = &pm.function.body {
                    self.predeclare_block_stmts(&body.stmts)?;
                }
                let m_entry = BasicBlockId(0);
                self.emit_hoisted_var_initializers(m_entry);
                self.arguments_param_count = Self::count_regular_params(&pm.function.params);
                let m_entry = self.emit_arguments_init(
                    m_entry,
                    Self::function_needs_arguments_object(&pm.function),
                )?;
                self.eval_caller_has_arguments = Self::detect_param_arguments(&pm.function.params)
                    || self.scopes.lookup("arguments").is_ok();
                let mut m_flow = StmtFlow::Open(m_entry);
                if let Some(body) = &pm.function.body {
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
                let m_old_fn = std::mem::replace(
                    &mut self.current_function,
                    FunctionBuilder::new("", BasicBlockId(0)),
                );
                let m_has_eval = m_old_fn.has_eval();
                let m_blocks = m_old_fn.into_blocks();
                let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                m_ir_function.set_has_eval(m_has_eval);
                m_ir_function.set_params(param_ir_names);
                let m_captured = self.captured_names_stack.last().unwrap().clone();
                m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                for b in m_blocks {
                    m_ir_function.push_block(b);
                }
                let m_function_id = self.module.push_function(m_ir_function);
                self.pop_function_context();
                private_method_ids.push((field_name, is_static, m_function_id));
            }
        }

        let ctor_name = format!("{}.constructor", class_name);
        self.push_function_context(&ctor_name, BasicBlockId(0));
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(Self::PENDING_CTOR_FUNCTION_ID, false);
        self.super_call_allowed = class_expr.class.super_class.is_some();

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_expr.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_expr.span(), msg))?;

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
                        .map_err(|msg| self.error(class_expr.span(), msg))?;
                    param_ir_names.push(format!("${scope_id}.{name}"));
                }
            }

            if class_expr.class.super_class.is_some()
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
        if constructor.is_none() && class_expr.class.super_class.is_some() {
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
        let defer_instance_initializers =
            constructor.is_some() && class_expr.class.super_class.is_some();
        if !defer_instance_initializers {
            field_block = self.emit_instance_initializers(
                field_block,
                this_scope_id,
                &class_expr.class.body,
                &private_method_ids,
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
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
                if defer_instance_initializers
                    && !deferred_instance_initializers_emitted
                    && stmt_is_direct_super_call(stmt)
                    && let StmtFlow::Open(block) = inner_flow
                {
                    inner_flow = StmtFlow::Open(self.emit_instance_initializers(
                        block,
                        this_scope_id,
                        &class_expr.class.body,
                        &private_method_ids,
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
        for (_, is_static, func_id) in &private_method_ids {
            if let Some(function) = self.module.function_mut(*func_id) {
                function.home_object = Some(if *is_static {
                    HomeObject::Constructor(ctor_function_id)
                } else {
                    HomeObject::Prototype(ctor_function_id)
                });
            }
        }
        self.pop_function_context();

        let ctor_dest = self.alloc_value();
        let ctor_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(ctor_function_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: ctor_dest,
                constant: ctor_ref_const,
            },
        );


        // 创建 prototype 对象
        let proto_dest = self.alloc_value();
        // 计算非构造函数方法数量，作为原型对象的容量
        let method_count = class_expr.class.body.iter().filter(|m| {
            matches!(m, swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method))
        }).count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        if let Some(super_class) = &class_expr.class.super_class {
            let super_ctor = self.lower_expr(super_class, block)?;
            let proto_key_const = self
                .module
                .add_constant(Constant::String("prototype".to_string()));
            let proto_key_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: proto_key_dest,
                    constant: proto_key_const,
                },
            );
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
        }

        // Methods (full support for all method kinds, static, and static blocks)
        let mut static_init_idx = 0u32;
        for member in &class_expr.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => match method.kind {
                    swc_ast::MethodKind::Method => {
                        let is_static = method.is_static;
                        let target = if is_static { ctor_dest } else { proto_dest };

                        let (method_name, m_key_dest) = match &method.key {
                            swc_ast::PropName::Ident(ident) => {
                                let name = ident.sym.to_string();
                                let key_const =
                                    self.module.add_constant(Constant::String(name.clone()));
                                let key_dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: key_dest,
                                        constant: key_const,
                                    },
                                );
                                (name, key_dest)
                            }
                            swc_ast::PropName::Str(s) => {
                                let name = s.value.to_string_lossy().into_owned();
                                let key_const =
                                    self.module.add_constant(Constant::String(name.clone()));
                                let key_dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: key_dest,
                                        constant: key_const,
                                    },
                                );
                                (name, key_dest)
                            }
                            swc_ast::PropName::Computed(_) => {
                                let key_dest = self.lower_prop_name(&method.key, block)?;
                                ("<computed>".to_string(), key_dest)
                            }
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}", class_name, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));
                        self.is_method = true;
                        self.super_allowed = true;
                        self.set_lexical_home_object_for_enclosing_method(
                            ctor_function_id,
                            is_static,
                        );

                        let env_scope_id = self
                            .scopes
                            .declare("$env", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        let this_scope_id = self
                            .scopes
                            .declare("$this", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;

                        let mut method_param_ir_names = vec![
                            format!("${env_scope_id}.$env"),
                            format!("${this_scope_id}.$this"),
                        ];
                        for param in &method.function.params {
                            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                let name = binding_ident.id.sym.to_string();
                                let scope_id = self
                                    .scopes
                                    .declare(&name, VarKind::Let, true)
                                    .map_err(|msg| self.error(method.span, msg))?;
                                method_param_ir_names.push(format!("${scope_id}.{name}"));
                            }
                        }

                        if let Some(body) = &method.function.body {
                            self.predeclare_block_stmts(&body.stmts)?;
                        }

                        let m_entry = BasicBlockId(0);
                        self.emit_hoisted_var_initializers(m_entry);
                        self.arguments_param_count =
                            Self::count_regular_params(&method.function.params);
                        let m_entry = self.emit_arguments_init(
                            m_entry,
                            Self::function_needs_arguments_object(&method.function),
                        )?;
                        self.eval_caller_has_arguments =
                            Self::detect_param_arguments(&method.function.params)
                                || self.scopes.lookup("arguments").is_ok();

                        let mut m_flow = StmtFlow::Open(m_entry);
                        if let Some(body) = &method.function.body {
                            for stmt in &body.stmts {
                                m_flow = self.lower_stmt(stmt, m_flow)?;
                            }
                        }

                        if let StmtFlow::Open(b) = m_flow {
                            self.current_function
                                .set_terminator(b, Terminator::Return { value: None });
                        }

                        let m_old_fn = std::mem::replace(
                            &mut self.current_function,
                            FunctionBuilder::new("", BasicBlockId(0)),
                        );
                        let m_has_eval = m_old_fn.has_eval();
                        let m_blocks = m_old_fn.into_blocks();
                        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                        m_ir_function.set_has_eval(m_has_eval);
                        m_ir_function.set_params(method_param_ir_names);
                        let m_captured = self.captured_names_stack.last().unwrap().clone();
                        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                        m_ir_function.home_object = Some(if is_static {
                            HomeObject::Constructor(ctor_function_id)
                        } else {
                            HomeObject::Prototype(ctor_function_id)
                        });
                        for b in m_blocks {
                            m_ir_function.push_block(b);
                        }
                        let m_function_id = self.module.push_function(m_ir_function);

                        self.pop_function_context();

                        let m_dest = self.alloc_value();
                        let m_ref_const = self
                            .module
                            .add_constant(Constant::FunctionRef(m_function_id));
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_dest,
                                constant: m_ref_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: target,
                                key: m_key_dest,
                                value: m_dest,
                            },
                        );
                    }
                    swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                        let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                            "get"
                        } else {
                            "set"
                        };
                        let is_static = method.is_static;
                        let target = if is_static { ctor_dest } else { proto_dest };

                        let (method_name, m_key_dest) = match &method.key {
                            swc_ast::PropName::Ident(ident) => {
                                let name = ident.sym.to_string();
                                let key_const =
                                    self.module.add_constant(Constant::String(name.clone()));
                                let key_dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: key_dest,
                                        constant: key_const,
                                    },
                                );
                                (name, key_dest)
                            }
                            swc_ast::PropName::Str(s) => {
                                let name = s.value.to_string_lossy().into_owned();
                                let key_const =
                                    self.module.add_constant(Constant::String(name.clone()));
                                let key_dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: key_dest,
                                        constant: key_const,
                                    },
                                );
                                (name, key_dest)
                            }
                            swc_ast::PropName::Computed(_) => {
                                let key_dest = self.lower_prop_name(&method.key, block)?;
                                ("<computed>".to_string(), key_dest)
                            }
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));
                        self.is_method = true;
                        self.super_allowed = true;
                        self.set_lexical_home_object_for_enclosing_method(
                            ctor_function_id,
                            is_static,
                        );

                        let env_scope_id = self
                            .scopes
                            .declare("$env", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                        let this_scope_id = self
                            .scopes
                            .declare("$this", VarKind::Let, true)
                            .map_err(|msg| self.error(method.span, msg))?;

                        let mut param_ir_names = vec![
                            format!("${env_scope_id}.$env"),
                            format!("${this_scope_id}.$this"),
                        ];
                        for param in &method.function.params {
                            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                                let name = binding_ident.id.sym.to_string();
                                let scope_id = self
                                    .scopes
                                    .declare(&name, VarKind::Let, true)
                                    .map_err(|msg| self.error(method.span, msg))?;
                                param_ir_names.push(format!("${scope_id}.{name}"));
                            }
                        }

                        if let Some(body) = &method.function.body {
                            self.predeclare_block_stmts(&body.stmts)?;
                        }

                        let m_entry = BasicBlockId(0);
                        self.emit_hoisted_var_initializers(m_entry);
                        self.arguments_param_count =
                            Self::count_regular_params(&method.function.params);
                        let m_entry = self.emit_arguments_init(
                            m_entry,
                            Self::function_needs_arguments_object(&method.function),
                        )?;
                        self.eval_caller_has_arguments =
                            Self::detect_param_arguments(&method.function.params)
                                || self.scopes.lookup("arguments").is_ok();

                        let mut m_flow = StmtFlow::Open(m_entry);
                        if let Some(body) = &method.function.body {
                            for stmt in &body.stmts {
                                m_flow = self.lower_stmt(stmt, m_flow)?;
                            }
                        }

                        if let StmtFlow::Open(b) = m_flow {
                            self.current_function
                                .set_terminator(b, Terminator::Return { value: None });
                        }

                        let m_old_fn = std::mem::replace(
                            &mut self.current_function,
                            FunctionBuilder::new("", BasicBlockId(0)),
                        );
                        let m_has_eval = m_old_fn.has_eval();
                        let m_blocks = m_old_fn.into_blocks();
                        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                        m_ir_function.set_has_eval(m_has_eval);
                        m_ir_function.set_params(param_ir_names);
                        let m_captured = self.captured_names_stack.last().unwrap().clone();
                        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                        m_ir_function.home_object = Some(if is_static {
                            HomeObject::Constructor(ctor_function_id)
                        } else {
                            HomeObject::Prototype(ctor_function_id)
                        });
                        for b in m_blocks {
                            m_ir_function.push_block(b);
                        }
                        let m_function_id = self.module.push_function(m_ir_function);
                        self.pop_function_context();

                        let fn_dest = self.alloc_value();
                        let fn_ref_const = self
                            .module
                            .add_constant(Constant::FunctionRef(m_function_id));
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: fn_dest,
                                constant: fn_ref_const,
                            },
                        );

                        let desc = self.build_descriptor(accessor, fn_dest, false, true, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![target, m_key_dest, desc],
                            },
                        );
                    }
                },
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    let fn_name = format!("{}.static_init_{}", class_name, static_init_idx);
                    static_init_idx += 1;

                    self.push_function_context(&fn_name, BasicBlockId(0));
                    self.is_method = true;
                    self.super_allowed = true;
                    self.set_lexical_home_object_for_enclosing_method(ctor_function_id, true);

                    let env_scope_id = self
                        .scopes
                        .declare("$env", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;
                    let this_scope_id = self
                        .scopes
                        .declare("$this", VarKind::Let, true)
                        .map_err(|msg| self.error(static_block.span, msg))?;

                    let param_ir_names = vec![
                        format!("${env_scope_id}.$env"),
                        format!("${this_scope_id}.$this"),
                    ];

                    self.predeclare_block_stmts(&static_block.body.stmts)?;

                    let m_entry = BasicBlockId(0);
                    self.emit_hoisted_var_initializers(m_entry);
                    self.arguments_param_count = 0;
                    let m_entry = self.emit_arguments_init(
                        m_entry,
                        Self::body_references_arguments(&static_block.body),
                    )?;
                    self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

                    let mut m_flow = StmtFlow::Open(m_entry);
                    for stmt in &static_block.body.stmts {
                        m_flow = self.lower_stmt(stmt, m_flow)?;
                    }

                    if let StmtFlow::Open(b) = m_flow {
                        self.current_function
                            .set_terminator(b, Terminator::Return { value: None });
                    }

                    let m_old_fn = std::mem::replace(
                        &mut self.current_function,
                        FunctionBuilder::new("", BasicBlockId(0)),
                    );
                    let m_has_eval = m_old_fn.has_eval();
                    let m_blocks = m_old_fn.into_blocks();
                    let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
                    m_ir_function.set_has_eval(m_has_eval);
                    m_ir_function.set_params(param_ir_names);
                    let m_captured = self.captured_names_stack.last().unwrap().clone();
                    m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
                    m_ir_function.home_object = Some(HomeObject::Constructor(ctor_function_id));
                    for b in m_blocks {
                        m_ir_function.push_block(b);
                    }
                    let m_function_id = self.module.push_function(m_ir_function);

                    self.pop_function_context();

                    let fn_dest = self.alloc_value();
                    let fn_ref_const = self
                        .module
                        .add_constant(Constant::FunctionRef(m_function_id));
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: fn_dest,
                            constant: fn_ref_const,
                        },
                    );

                    self.current_function.append_instruction(
                        block,
                        Instruction::Call {
                            dest: None,
                            callee: fn_dest,
                            this_val: ctor_dest,
                            args: vec![],
                        },
                    );
                }
                swc_ast::ClassMember::PrivateProp(prop) if prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    let init_val = if let Some(value) = &prop.value {
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
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![ctor_dest, key_dest, init_val],
                        },
                    );
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    let init_val = if let Some(value) = &prop.value {
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
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: ctor_dest,
                            key: key_dest,
                            value: init_val,
                        },
                    );
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if *is_static {
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
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
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
                        args: vec![ctor_dest, key_dest, fn_dest],
                    },
                );
            }
        }

        // Set constructor.prototype = proto_obj
        let proto_key_const = self
            .module
            .add_constant(Constant::String("prototype".to_string()));
        let proto_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: proto_key_dest,
                constant: proto_key_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: ctor_dest,
                key: proto_key_dest,
                value: proto_dest,
            },
        );

        if let Some((ref name, scope_id)) = class_body_name_scope {
            self.scopes
                .mark_initialised(name)
                .map_err(|msg| self.error(class_expr.span(), msg))?;
            let ir_name = format!("${scope_id}.{name}");
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: ir_name,
                    value: ctor_dest,
                },
            );
            self.scopes.pop_scope();
        }



        Ok(ctor_dest)
    }
}
