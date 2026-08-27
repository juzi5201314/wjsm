use super::*;

impl Lowerer {
    /// 共享类体降级：构造器函数、原型对象、超类链、方法/访问器/静态块/字段、装饰器。
    ///
    /// 调用方负责类名词法作用域的 push/pop 与最终名字绑定。
    /// 返回（最终 block、构造器值、未被 class decorator 替换的构造器 FunctionId）。
    pub(super) fn lower_class_body(
        &mut self,
        class_name: &str,
        class: &swc_ast::Class,
        class_span: Span,
        decorator_name: Option<&str>,
        block: BasicBlockId,
    ) -> Result<(BasicBlockId, ValueId, Option<FunctionId>), LoweringError> {
        let constructor = class.body.iter().find_map(|member| match member {
            swc_ast::ClassMember::Constructor(c) => Some(c),
            _ => None,
        });

        // 本类 lowering 新建函数的起始下标：PENDING ctor 占位回填只作用于该区间，
        // 避免嵌套类误改外层类已入模的 PENDING 引用。
        let class_function_start = self.module.functions().len();

        self.push_class_private_name_scope(class_name, &class.body);
        let mut private_members = self.collect_class_private_members(class_name, &class.body)?;

        // ── 构造器 IR 函数 ──
        // 构造器体延迟到类求值完成后才执行，期间类名已初始化（构造器体/实例字段初始化器可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复（类求值期间仍为 TDZ）。
        let ctor_name = format!("{}.constructor", class_name);
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(class_span, msg))?;
        }
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

        // TypeScript 参数属性 `constructor(private x: number)` 同时是构造器**形参**
        // 与**实例字段声明**。此前整个 TsParamProp 被丢弃，导致形参不存在、字段也
        // 不存在（`this.x` 读到 undefined）。这里把它归一成普通形参 Pat 参与形参
        // 处理，并记录字段名，稍后发射 `this.<name> = <name>`。
        let mut ctor_param_pats_owned: Vec<swc_ast::Pat> = Vec::new();
        // （形参在 param_ir_names 中的下标, 字段名）
        let mut param_prop_slots: Vec<(usize, String)> = Vec::new();
        // param_ir_names 前两项固定为 $env / $this；Rest 形参不占 IR 形参名。
        let mut ir_slot = 2usize;
        for param in constructor.into_iter().flat_map(|ctor| &ctor.params) {
            let pat = match param {
                swc_ast::ParamOrTsParamProp::Param(param) => param.pat.clone(),
                swc_ast::ParamOrTsParamProp::TsParamProp(prop) => {
                    let (pat, binding) = match &prop.param {
                        swc_ast::TsParamPropParam::Ident(binding) => {
                            (swc_ast::Pat::Ident(binding.clone()), binding)
                        }
                        swc_ast::TsParamPropParam::Assign(assign) => match &*assign.left {
                            swc_ast::Pat::Ident(binding) => {
                                (swc_ast::Pat::Assign(assign.clone()), binding)
                            }
                            // TS 早期错误：参数属性的绑定必须是标识符，不允许解构。
                            other => {
                                return Err(self.error(
                                    other.span(),
                                    "a parameter property may not be declared using a binding pattern",
                                ));
                            }
                        },
                    };
                    param_prop_slots.push((ir_slot, binding.id.sym.to_string()));
                    pat
                }
            };
            if !matches!(pat, swc_ast::Pat::Rest(_)) {
                ir_slot += 1;
            }
            ctor_param_pats_owned.push(pat);
        }
        let ctor_param_pats: Vec<&swc_ast::Pat> = ctor_param_pats_owned.iter().collect();
        let param_ir_names =
            self.build_param_ir_names_impl(&ctor_param_pats, env_scope_id, this_scope_id)?;
        // 形参 IR 名此时才确定，回填成 (形参 IR 名, 字段名)。
        let param_prop_fields: Vec<(String, String)> = param_prop_slots
            .into_iter()
            .map(|(slot, field)| (param_ir_names[slot].clone(), field))
            .collect();
        if let Some(ctor) = constructor {
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
        let parameter_block = self.emit_pat_inits_impl(&ctor_param_pats, &param_ir_names, entry)?;

        let mut field_block = parameter_block;
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
            // 参数属性字段先于字段初始化器生效（TS 语义），故在其之前发射。
            field_block =
                self.emit_param_prop_fields(field_block, this_scope_id, &param_prop_fields);
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
        if constructor.is_some() {
            self.arguments_param_count = u32::try_from(
                ctor_param_pats
                    .iter()
                    .take_while(|pat| !matches!(pat, swc_ast::Pat::Rest(_)))
                    .count(),
            )
            .map_err(|_| self.error(class_span, "too many constructor parameters"))?;
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
                    // 派生类：`this` 在 super() 之后才存在，故参数属性字段与
                    // 字段初始化器都必须推迟到此处，且参数属性先行。
                    let after_props =
                        self.emit_param_prop_fields(b, this_scope_id, &param_prop_fields);
                    inner_flow = StmtFlow::Open(self.emit_instance_initializers(
                        after_props,
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
        ir_function.set_needs_prototype(true);
        let ctor_function_id = self.module.push_function(ir_function);
        if let Some(function) = self.module.function_mut(ctor_function_id) {
            function.home_object = Some(HomeObject::Prototype(ctor_function_id));
        }
        self.patch_pending_ctor_home_object_references(ctor_function_id, class_function_start);
        self.patch_private_member_home_objects(ctor_function_id, &private_members);
        self.pop_function_context();
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }

        // ── 物化构造器 + 创建原型 ──
        let block = self.materialize_private_member_values(block, &mut private_members)?;

        let constructor_function = LoweredClassFunction {
            function_id: ctor_function_id,
            captured: ctor_captured,
        };
        let (block, ctor_dest) =
            self.materialize_class_function_value(block, &constructor_function, class_span)?;

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
            self.emit_set_prop(block, ctor_dest, super_key_dest, super_ctor);
        }

        // 设置 prototype.constructor = ctor（ECMAScript: ClassDefinitionEvaluation）
        let ctor_key_dest = self.emit_string_const(block, "constructor");
        self.emit_set_prop(block, proto_dest, ctor_key_dest, ctor_dest);

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
        self.emit_set_prop(block, ctor_dest, proto_key_dest, proto_dest);

        let direct_constructor = class.decorators.is_empty().then_some(ctor_function_id);
        let (block, ctor_dest) =
            self.emit_apply_class_decorators(block, ctor_dest, &class.decorators, decorator_name)?;

        self.pop_class_private_name_scope();
        Ok((block, ctor_dest, direct_constructor))
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
                if method.function.is_generator || method.function.is_async {
                    // generator / async 方法体延迟到类求值完成后才执行，期间类名已初始化；
                    // 二者都需要 body + wrapper 双函数结构，路由在 lower_method_prop_to_fn。
                    let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
                    if let Some(sid) = class_scope_id {
                        self.scopes
                            .set_initialised(sid, class_name, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                    }
                    // 类方法的 [[HomeObject]] 静态可知，async 函数族 body 的 super
                    // 经静态元数据接线，不依赖 env 上的 `home`。generator（含 async
                    // generator）wrapper 因此无需包 home env——包装反而使 body 深度 0
                    // 的捕获写落在包装对象上遮蔽共享 env；async 非 generator 方法
                    // 保留 home wrapper（eval meta 等仍消费 env home）。
                    let method_home = if method.function.is_generator {
                        None
                    } else {
                        Some(target)
                    };
                    let static_home = if is_static {
                        HomeObject::Constructor(ctor_function_id)
                    } else {
                        HomeObject::Prototype(ctor_function_id)
                    };
                    let function = self.lower_method_prop_to_fn(
                        &method.key,
                        &method.function,
                        method_home,
                        Some(static_home),
                    )?;
                    if let Some(sid) = class_scope_id {
                        let _ = self.scopes.set_initialised(sid, class_name, false);
                    }
                    let (continuation, mut method_value) = self.materialize_method_function_value(
                        block,
                        &function,
                        method_home,
                        method.function.span,
                    )?;
                    block = continuation;
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
                    self.emit_set_prop(block, target, m_key_dest, method_value);
                    return Ok(block);
                }

                let fn_name = format!("{class_name}.{method_name}");
                let function = self.lower_class_method_fn(
                    class_name,
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;
                let (continuation, mut m_dest) =
                    self.materialize_class_function_value(block, &function, method.span)?;
                block = continuation;
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
                self.emit_set_prop(block, target, m_key_dest, m_dest);
                Ok(block)
            }
            swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                    "get"
                } else {
                    "set"
                };
                let fn_name = format!("{class_name}.{accessor}_{method_name}");
                let function = self.lower_class_method_fn(
                    class_name,
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;
                let (continuation, mut fn_dest) =
                    self.materialize_class_function_value(block, &function, method.span)?;
                block = continuation;
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
        let function = self.lower_class_static_block_fn(
            &fn_name,
            &static_block.body,
            static_block.span,
            ctor_function_id,
        )?;
        let (continuation, function_value) =
            self.materialize_class_function_value(block, &function, static_block.span)?;
        self.current_function.append_instruction(
            continuation,
            Instruction::Call {
                dest: None,
                callee: function_value,
                this_val: ctor_dest,
                args: vec![],
            },
        );
        Ok(continuation)
    }

    /// 为类方法/访问器创建 IR 函数并返回其函数标识及捕获集合。
    fn lower_class_method_fn(
        &mut self,
        class_name: &str,
        fn_name: &str,
        function: &swc_ast::Function,
        method_span: Span,
        ctor_function_id: FunctionId,
        is_static: bool,
    ) -> Result<LoweredClassFunction, LoweringError> {
        // 方法体延迟到类求值完成后才执行，期间类名已初始化（方法体可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复。
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(method_span, msg))?;
        }
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
        let function =
            self.finalize_class_method_function(fn_name, method_span, param_ir_names, home_object);
        self.pop_function_context();
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }
        Ok(function)
    }

    /// 为类静态块创建 IR 函数并返回其函数标识及捕获集合。
    fn lower_class_static_block_fn(
        &mut self,
        fn_name: &str,
        body: &swc_ast::BlockStmt,
        span: Span,
        ctor_function_id: FunctionId,
    ) -> Result<LoweredClassFunction, LoweringError> {
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

        let function = self.finalize_class_method_function(
            fn_name,
            span,
            param_ir_names,
            HomeObject::Constructor(ctor_function_id),
        );
        self.pop_function_context();
        Ok(function)
    }

    /// 收尾方法 IR 函数：提取 blocks、设置元数据，并返回统一的 class function metadata。
    fn finalize_class_method_function(
        &mut self,
        fn_name: &str,
        span: Span,
        param_ir_names: Vec<String>,
        home_object: HomeObject,
    ) -> LoweredClassFunction {
        let old_function = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_function.has_eval();
        let blocks = old_function.into_blocks();
        let mut ir_function = Function::new(fn_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(source_span) = self.span_to_source_span(span) {
            ir_function.set_source_span(source_span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        ir_function.home_object = Some(home_object);
        for block in blocks {
            ir_function.push_block(block);
        }
        let function_id = self.module.push_function(ir_function);
        LoweredClassFunction {
            function_id,
            captured,
        }
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
            swc_ast::PropName::Computed(_)
            | swc_ast::PropName::Num(_)
            | swc_ast::PropName::BigInt(_) => {
                let key_dest = self.lower_prop_name(key, block)?;
                Ok(("<computed>".to_string(), key_dest))
            }
        }
    }
}
