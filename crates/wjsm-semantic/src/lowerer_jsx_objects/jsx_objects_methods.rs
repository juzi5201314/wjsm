use super::*;

impl Lowerer {
    pub(crate) fn lower_object_expr(
        &mut self,
        obj_expr: &swc_ast::ObjectLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if let Some(keys) = collect_static_object_literal_keys(obj_expr, &mut self.module) {
            return self.lower_sso_object_literal(obj_expr, block, keys);
        }

        let obj_dest = self.alloc_value();
        // 容量取 4 和属性数量的较大值，确保对象字面量有足够的槽位
        let capacity = std::cmp::max(4, obj_expr.props.len() as u32);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        // 属性值（方法闭包共享环境 phi、计算键的三元/new 异常分叉等）可能引入控制流，
        // 推进 block。每个依赖该值的 SetProp/DefineProperty 必须发射在推进后的块上，
        // 否则会先于值定义执行（如方法属性的闭包尚未创建就 SetProp）。
        let original_block = block;
        let mut block = block;

        for prop in &obj_expr.props {
            match prop {
                swc_ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                    swc_ast::Prop::KeyValue(kv) => {
                        if is_proto_object_literal_key(&kv.key) {
                            // `__proto__: value` 走 SetProto；静态键无副作用，
                            // 仅需按规范传播属性值求值抛出的异常。
                            let val_dest = self.lower_expr_then_continue(&kv.value, &mut block)?;
                            if self.expr_can_throw(&kv.value) {
                                block = self.lower_value_exception_branch(block, val_dest)?;
                            }
                            self.current_function.append_instruction(
                                block,
                                Instruction::SetProto {
                                    object: obj_dest,
                                    value: val_dest,
                                },
                            );
                            continue;
                        }
                        // PropertyDefinitionEvaluation：先求属性键再求属性值；
                        // 计算键抛异常必须传播且不得继续求属性值。
                        let key_dest = self.lower_prop_name_checked(&kv.key, &mut block)?;
                        // NamedEvaluation：匿名函数定义按属性键命名——静态键
                        // 走降级期提示，计算键在值求值后运行时设置。
                        let named_by_key = Self::is_anonymous_fn_definition(&kv.value);
                        let static_key_name = Self::static_prop_name_text(&kv.key);
                        if named_by_key && let Some(name) = &static_key_name {
                            self.named_eval_hint = Some(name.clone());
                        }
                        let val_dest = self.lower_expr_then_continue(&kv.value, &mut block)?;
                        // 属性值求值抛异常必须传播，
                        // 不得把 TAG_EXCEPTION 存为属性值后继续求值后续属性。
                        if self.expr_can_throw(&kv.value) {
                            block = self.lower_value_exception_branch(block, val_dest)?;
                        }
                        if named_by_key && static_key_name.is_none() {
                            self.emit_runtime_set_function_name(
                                block,
                                val_dest,
                                key_dest,
                                AccessorPrefix::None,
                            );
                        }
                        self.emit_set_prop(block, obj_dest, key_dest, val_dest);
                    }
                    swc_ast::Prop::Shorthand(ident) => {
                        let val_dest = self.lower_ident(ident, block)?;
                        block = self.resolve_store_block(block);
                        let key_str = ident.sym.to_string();
                        let key_const = self.module.add_constant(Constant::String(key_str));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        self.emit_set_prop(block, obj_dest, key_dest, val_dest);
                    }
                    swc_ast::Prop::Getter(getter) => {
                        let key_dest = self.lower_prop_name_checked(&getter.key, &mut block)?;
                        let body = getter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(getter.span, "getter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let function =
                            self.lower_method_to_fn(&getter.key, body, None, home_object)?;
                        let (continuation, fn_value) = self.materialize_method_function_value(
                            block,
                            &function,
                            home_object,
                            getter.span,
                        )?;
                        block = continuation;
                        // getter 的 length 恒为 0（无形参）；name 为 `get x`。
                        self.set_function_js_metadata(function.function_id, None, 0);
                        let source_text = self.span_source_text(getter.span).map(str::to_owned);
                        self.set_function_source_text(function.function_id, source_text);
                        self.apply_method_js_name(
                            block,
                            function.function_id,
                            fn_value,
                            &getter.key,
                            key_dest,
                            AccessorPrefix::Get,
                        );
                        let desc = self.build_descriptor("get", fn_value, true, true, block)?;
                        block = self.resolve_store_block(block);
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Setter(setter) => {
                        let key_dest = self.lower_prop_name_checked(&setter.key, &mut block)?;
                        let body = setter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(setter.span, "setter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let function = self.lower_method_to_fn(
                            &setter.key,
                            body,
                            Some(std::slice::from_ref(&*setter.param)),
                            home_object,
                        )?;
                        let (continuation, fn_value) = self.materialize_method_function_value(
                            block,
                            &function,
                            home_object,
                            setter.span,
                        )?;
                        block = continuation;
                        // setter 的 length 按 ExpectedArgumentCount（默认值形参为 0）。
                        self.set_function_js_metadata(
                            function.function_id,
                            None,
                            Self::expected_argument_count(std::slice::from_ref(&*setter.param)),
                        );
                        let source_text = self.span_source_text(setter.span).map(str::to_owned);
                        self.set_function_source_text(function.function_id, source_text);
                        self.apply_method_js_name(
                            block,
                            function.function_id,
                            fn_value,
                            &setter.key,
                            key_dest,
                            AccessorPrefix::Set,
                        );
                        let desc = self.build_descriptor("set", fn_value, true, true, block)?;
                        block = self.resolve_store_block(block);
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Method(method) => {
                        let key_dest = self.lower_prop_name_checked(&method.key, &mut block)?;
                        let home_object = if method
                            .function
                            .body
                            .as_ref()
                            .is_some_and(block_needs_home_object)
                        {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let function = self.lower_method_prop_to_fn(
                            &method.key,
                            &method.function,
                            home_object,
                            None,
                        )?;
                        let (continuation, fn_value) = self.materialize_method_function_value(
                            block,
                            &function,
                            home_object,
                            method.function.span,
                        )?;
                        block = continuation;
                        self.set_function_js_metadata(
                            function.function_id,
                            None,
                            Self::expected_param_count(&method.function.params),
                        );
                        let source_text = self.span_source_text(method.span()).map(str::to_owned);
                        self.set_function_source_text(function.function_id, source_text);
                        self.apply_method_js_name(
                            block,
                            function.function_id,
                            fn_value,
                            &method.key,
                            key_dest,
                            AccessorPrefix::None,
                        );
                        self.emit_set_prop(block, obj_dest, key_dest, fn_value);
                    }
                    _ => {
                        return Err(
                            self.error(prop.span(), "unsupported property kind in object literal")
                        );
                    }
                },
                swc_ast::PropOrSpread::Spread(spread) => {
                    let source = self.lower_expr_then_continue(&spread.expr, &mut block)?;
                    // CopyDataProperties：spread 源求值抛异常必须传播，
                    // 不得让 TAG_EXCEPTION 流入 ObjectSpread 被静默吞掉。
                    if self.expr_can_throw(&spread.expr) {
                        block = self.lower_value_exception_branch(block, source)?;
                    }
                    block = self.emit_object_spread_checked(block, obj_dest, source)?;
                }
            }
        }

        if block != original_block {
            self.expr_merge_block = Some(block);
        }
        Ok(obj_dest)
    }

    /// 将 PropName 转换为运行时的 key value：静态名生成 String 常量，Computed 则 lower 表达式
    pub(crate) fn lower_prop_name(
        &mut self,
        key: &swc_ast::PropName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match key {
            swc_ast::PropName::Ident(ident) => {
                let key_str = ident.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Str(s) => {
                let key_str = s.value.to_string_lossy().into_owned();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Computed(computed) => self.lower_expr(&computed.expr, block),
            swc_ast::PropName::Num(num) => {
                let key_str = num
                    .raw
                    .as_ref()
                    .map(|raw| raw.to_string())
                    .unwrap_or_else(|| js_number_property_key(num.value));
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::BigInt(bigint) => {
                let key_str = bigint.value.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
        }
    }

    /// 求值属性名并推进 block。计算键遵循 PropertyDefinitionEvaluation：
    /// 键表达式抛出的异常必须在求属性值 / 构建方法闭包之前传播；随后按
    /// ComputedPropertyName 语义在求属性值之前完成 ToPropertyKey（对象键
    /// 再入用户 `toString` / `valueOf` / `Symbol.toPrimitive`，异常同样传播）。
    /// 类成员键（方法 / 字段）与对象字面量键共用本入口。
    pub(crate) fn lower_prop_name_checked(
        &mut self,
        key: &swc_ast::PropName,
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let swc_ast::PropName::Computed(computed) = key else {
            // 静态键只发射 Const，不产生控制流与异常。
            return self.lower_prop_name(key, *block);
        };
        let key_dest = self.lower_expr_then_continue(&computed.expr, block)?;
        if self.expr_can_throw(&computed.expr) {
            *block = self.lower_value_exception_branch(*block, key_dest)?;
        }
        let converted = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::CallBuiltin {
                dest: Some(converted),
                builtin: Builtin::ToPropertyKey,
                args: vec![key_dest],
            },
        );
        // 转换本身可抛（用户转换函数 throw / 无法转为 primitive 的 TypeError），
        // 与键表达式是否可抛无关，必须无条件分叉传播。
        *block = self.lower_value_exception_branch(*block, converted)?;
        Ok(converted)
    }

    fn create_method_env_with_home(
        &mut self,
        block: BasicBlockId,
        parent_env: ValueId,
        home_object: ValueId,
    ) -> ValueId {
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env,
                capacity: 1,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: env,
                value: parent_env,
            },
        );

        let home_key = self
            .module
            .add_constant(Constant::String("home".to_string()));
        let home_key_value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: home_key_value,
                constant: home_key,
            },
        );
        self.emit_set_prop(block, env, home_key_value, home_object);
        env
    }

    /// 将 getter/setter 方法体编译为独立 IR 函数，物化由 caller 在外层 continuation 完成。
    pub(crate) fn lower_method_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        body: &swc_ast::BlockStmt,
        accessor_params: Option<&[swc_ast::Pat]>,
        home_object: Option<ValueId>,
    ) -> Result<LoweredMethodFunction, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        // 推入新的函数上下文（使用 push_function_context 管理作用域栈）
        self.push_function_context(&fn_name, BasicBlockId(0));
        self.apply_function_strictness(Some(body));
        self.super_allowed = home_object.is_some();

        // 声明 $env 和 $this
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let method_param_ir_names = if let Some(pats) = accessor_params {
            self.build_arrow_param_ir_names(pats, env_scope_id, this_scope_id)?
        } else {
            vec![
                format!("${env_scope_id}.$env"),
                format!("${this_scope_id}.$this"),
            ]
        };

        // 预声明提升变量
        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);

        // 对象字面量方法/访问器始终物化 arguments（不参与惰性消除）。
        // 原因：此路径（lower_method_to_fn / lower_method_prop_to_fn）在降级方法体时会
        // 再入式地为内部嵌套方法/捕获闭包建 $shared_env，依赖 entry block 的指令布局；
        // 减少 entry block 指令数（消除 arguments-init）会触发该路径既有的 block-resolution
        // 缺陷，使方法体被截断为 unreachable（见 for-await + [Symbol.iterator] 复现）。
        // 且对象方法必然分配（闭包/对象），恒为 may-GC，消除 arguments 对 Layer 3 零收益。
        // 故安全且无损地保持基线行为。
        let m_entry = self.emit_arguments_init(m_entry, true)?;
        self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

        // 降低方法体
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

        // Finalize method function
        let m_old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let m_has_eval = m_old_fn.has_eval();
        let m_blocks = m_old_fn.into_blocks();
        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
        m_ir_function.set_has_eval(m_has_eval);
        if let Some(span) = self.span_to_source_span(body.span()) {
            m_ir_function.set_source_span(span);
        }
        m_ir_function.set_params(method_param_ir_names);
        let m_captured = self.captured_names_stack.last().unwrap().clone();
        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
        for b in m_blocks {
            m_ir_function.push_block(b);
        }
        let m_function_id = self.module.push_function(m_ir_function);

        self.pop_function_context();

        Ok(LoweredMethodFunction {
            function_id: m_function_id,
            captured: m_captured,
        })
    }

    /// `static_home`：类方法传静态 [[HomeObject]]（构造器 id 已知）；对象字面量
    /// 方法传 None，home 经运行时闭包 env 的 `home` 属性解析。
    pub(crate) fn lower_method_prop_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        function: &swc_ast::Function,
        home_object: Option<ValueId>,
        static_home: Option<HomeObject>,
    ) -> Result<LoweredMethodFunction, LoweringError> {
        // async 函数族 body 是独立 IR 函数，super 绑定需要显式接线。
        let method_super = match (static_home, home_object) {
            (Some(home), _) => MethodSuperBinding::Static(home),
            (None, Some(_)) => MethodSuperBinding::ClosureEnv,
            (None, None) => MethodSuperBinding::None,
        };
        if function.is_generator {
            let method_name = match key {
                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                _ => "anonymous".to_string(),
            };
            let declaration = swc_ast::FnDecl {
                ident: swc_ast::Ident::new(
                    method_name.into(),
                    key.span(),
                    swc_core::common::SyntaxContext::empty(),
                ),
                declare: false,
                function: Box::new(function.clone()),
            };
            // async generator 方法与同步 generator 方法各自复用对应的声明路径。
            let (function_id, captured) = if function.is_async {
                self.lower_async_gen_function(&declaration, method_super)?
            } else {
                self.lower_gen_function(&declaration)?
            };
            return Ok(LoweredMethodFunction {
                function_id,
                captured,
            });
        }
        if function.is_async {
            // async 方法复用 async 函数表达式的 body + wrapper 构建路径，
            // 返回 wrapper FunctionId 交由调用方物化为属性值。
            let method_name = match key {
                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                _ => "anonymous".to_string(),
            };
            let fake_expr = swc_ast::FnExpr {
                ident: None,
                function: Box::new(function.clone()),
            };
            let (function_id, captured) =
                self.lower_async_function_parts(&method_name, &fake_expr, method_super)?;
            return Ok(LoweredMethodFunction {
                function_id,
                captured,
            });
        }
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        self.push_function_context(&fn_name, BasicBlockId(0));
        self.apply_function_strictness(function.body.as_ref());
        self.super_allowed = home_object.is_some();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let param_ir_names =
            self.build_param_ir_names(&function.params, env_scope_id, this_scope_id)?;

        if let Some(body) = &function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let body_entry = self.emit_param_inits(&function.params, &param_ir_names, entry)?;

        // 对象字面量方法始终物化 arguments（见上方 lower_method_to_fn 处的说明）。
        let body_entry = self.emit_arguments_init(body_entry, true)?;
        self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
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
        let mut ir_function = Function::new(&fn_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) = self.span_to_source_span(function.span()) {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        self.pop_function_context();

        Ok(LoweredMethodFunction {
            function_id,
            captured,
        })
    }

    pub(crate) fn materialize_method_function_value(
        &mut self,
        block: BasicBlockId,
        function: &LoweredMethodFunction,
        home_object: Option<ValueId>,
        span: Span,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let function_ref = self
            .module
            .add_constant(Constant::FunctionRef(function.function_id));
        let function_value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: function_value,
                constant: function_ref,
            },
        );

        if let Some(home_object) = home_object {
            let (continuation, parent_env) = if function.captured.is_empty() {
                (block, self.load_env_object(block))
            } else {
                let env = self.ensure_shared_env(block, &function.captured, span)?;
                (self.resolve_store_block(block), env)
            };
            let method_env =
                self.create_method_env_with_home(continuation, parent_env, home_object);
            let closure = self.alloc_value();
            self.current_function.append_instruction(
                continuation,
                Instruction::CallBuiltin {
                    dest: Some(closure),
                    builtin: Builtin::CreateClosure,
                    args: vec![function_value, method_env],
                },
            );
            return Ok((continuation, closure));
        }

        if function.captured.is_empty() {
            return Ok((block, function_value));
        }

        let env = self.ensure_shared_env(block, &function.captured, span)?;
        let continuation = self.resolve_store_block(block);
        let closure = self.alloc_value();
        self.current_function.append_instruction(
            continuation,
            Instruction::CallBuiltin {
                dest: Some(closure),
                builtin: Builtin::CreateClosure,
                args: vec![function_value, env],
            },
        );
        Ok((continuation, closure))
    }

    /// 构建 getter/setter descriptor 对象 { get/set: fn, enumerable, configurable }
    pub(crate) fn build_descriptor(
        &mut self,
        accessor_kind: &str,
        fn_value: ValueId,
        enumerable: bool,
        configurable: bool,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let desc_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_dest,
                capacity: 4,
            },
        );

        // descriptor[accessor_kind] = fn
        let key_const = self
            .module
            .add_constant(Constant::String(accessor_kind.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        self.emit_set_prop(block, desc_dest, key_dest, fn_value);

        // descriptor.enumerable
        let enum_key = self
            .module
            .add_constant(Constant::String("enumerable".to_string()));
        let enum_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_key_dest,
                constant: enum_key,
            },
        );
        let enum_val_dest = self.alloc_value();
        let enum_const = self.module.add_constant(Constant::Bool(enumerable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_val_dest,
                constant: enum_const,
            },
        );
        self.emit_set_prop(block, desc_dest, enum_key_dest, enum_val_dest);

        // descriptor.configurable
        let conf_key = self
            .module
            .add_constant(Constant::String("configurable".to_string()));
        let conf_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_key_dest,
                constant: conf_key,
            },
        );
        let conf_val_dest = self.alloc_value();
        let conf_const = self.module.add_constant(Constant::Bool(configurable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_val_dest,
                constant: conf_const,
            },
        );
        self.emit_set_prop(block, desc_dest, conf_key_dest, conf_val_dest);

        Ok(desc_dest)
    }

    /// 静态字符串键对象字面量（ASCII ≤6 码元 inline；其余经 String 常量 NameRef）：
    /// install 期烘焙 shape，运行时走 InitObjectLiteral JIT。
    fn lower_sso_object_literal(
        &mut self,
        obj_expr: &swc_ast::ObjectLit,
        block: BasicBlockId,
        keys: Vec<u64>,
    ) -> Result<ValueId, LoweringError> {
        let template = self.module.add_constant(Constant::ObjectTemplate { keys });
        let original_block = block;
        let mut block = block;
        let mut values = Vec::with_capacity(obj_expr.props.len());
        for prop in &obj_expr.props {
            let swc_ast::PropOrSpread::Prop(prop) = prop else {
                unreachable!("collect_sso_object_literal_keys 已排除 spread");
            };
            let swc_ast::Prop::KeyValue(kv) = prop.as_ref() else {
                unreachable!("collect_sso_object_literal_keys 仅接受 KeyValue");
            };
            // NamedEvaluation：SSO 路径的键恒为静态字符串键，匿名函数定义
            // 按键名命名（与通用路径的静态键分支同语义）。
            if Self::is_anonymous_fn_definition(&kv.value)
                && let Some(name) = Self::static_prop_name_text(&kv.key)
            {
                self.named_eval_hint = Some(name);
            }
            let val_dest = self.lower_expr_then_continue(&kv.value, &mut block)?;
            // 与通用路径一致：属性值求值抛异常必须传播，
            // 不得把 TAG_EXCEPTION 烘焙进 InitObjectLiteral 的值列表。
            if self.expr_can_throw(&kv.value) {
                block = self.lower_value_exception_branch(block, val_dest)?;
            }
            values.push(val_dest);
        }
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::InitObjectLiteral {
                dest: obj_dest,
                template,
                values,
            },
        );
        if block != original_block {
            self.expr_merge_block = Some(block);
        }
        Ok(obj_dest)
    }
}

/// 收集可 JIT 初始化的静态字符串键；不满足条件时返回 None。
fn collect_static_object_literal_keys(
    obj_expr: &swc_ast::ObjectLit,
    module: &mut wjsm_ir::Module,
) -> Option<Vec<u64>> {
    use wjsm_ir::constants::OBJECT_TEMPLATE_MAX_PROPS;

    if obj_expr.props.len() > OBJECT_TEMPLATE_MAX_PROPS as usize {
        return None;
    }
    let mut keys = Vec::with_capacity(obj_expr.props.len());
    for prop in &obj_expr.props {
        match prop {
            swc_ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                swc_ast::Prop::KeyValue(kv) => {
                    if is_proto_object_literal_key(&kv.key) {
                        return None;
                    }
                    keys.push(static_object_literal_property_key(&kv.key, module)?);
                }
                _ => return None,
            },
            swc_ast::PropOrSpread::Spread(_) => return None,
        }
    }
    Some(keys)
}

fn is_proto_object_literal_key(key: &swc_ast::PropName) -> bool {
    match key {
        swc_ast::PropName::Ident(ident) => ident.sym.as_ref() == "__proto__",
        swc_ast::PropName::Str(s) => s.value.to_string_lossy().as_ref() == "__proto__",
        _ => false,
    }
}

fn static_object_literal_property_key(
    key: &swc_ast::PropName,
    module: &mut wjsm_ir::Module,
) -> Option<u64> {
    use wjsm_ir::{Constant, value};

    let key_str = match key {
        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
        swc_ast::PropName::Num(num) => num
            .raw
            .as_ref()
            .map(|raw| raw.to_string())
            .unwrap_or_else(|| js_number_property_key(num.value)),
        _ => return None,
    };
    if key_str.is_ascii() && key_str.len() <= 6 {
        let encoded = value::encode_inline_ascii(key_str.as_bytes())?;
        value::inline_property_key_raw(encoded)
    } else {
        let constant_idx = module.add_constant(Constant::String(key_str));
        Some(value::template_name_ref_key(constant_idx.0))
    }
}
