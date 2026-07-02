use super::*;

impl Lowerer {
    pub(crate) fn lower_object_expr(
        &mut self,
        obj_expr: &swc_ast::ObjectLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
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
                        let val_dest = self.lower_expr_then_continue(&kv.value, &mut block)?;
                        self.lower_object_prop(obj_dest, &kv.key, val_dest, &mut block)?;
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
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val_dest,
                            },
                        );
                    }
                    swc_ast::Prop::Getter(getter) => {
                        let key_dest = self.lower_prop_name(&getter.key, block)?;
                        block = self.resolve_store_block(block);
                        let body = getter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(getter.span, "getter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let fn_value =
                            self.lower_method_to_fn(&getter.key, body, None, home_object, block)?;
                        block = self.resolve_store_block(block);
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
                        let key_dest = self.lower_prop_name(&setter.key, block)?;
                        block = self.resolve_store_block(block);
                        let body = setter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(setter.span, "setter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let fn_value = self.lower_method_to_fn(
                            &setter.key,
                            body,
                            Some(std::slice::from_ref(&*setter.param)),
                            home_object,
                            block,
                        )?;
                        block = self.resolve_store_block(block);
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
                        let key_dest = self.lower_prop_name(&method.key, block)?;
                        block = self.resolve_store_block(block);
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
                        let fn_value = self.lower_method_prop_to_fn(
                            &method.key,
                            &method.function,
                            home_object,
                            block,
                        )?;
                        block = self.resolve_store_block(block);
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: fn_value,
                            },
                        );
                    }
                    _ => {
                        return Err(
                            self.error(prop.span(), "unsupported property kind in object literal")
                        );
                    }
                },
                swc_ast::PropOrSpread::Spread(spread) => {
                    let source = self.lower_expr_then_continue(&spread.expr, &mut block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
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
            _ => Err(self.error(key.span(), "unsupported property key kind")),
        }
    }

    /// 对对象字面量中的 KeyValue prop 设置属性，支持计算属性名
    pub(crate) fn lower_object_prop(
        &mut self,
        obj_dest: ValueId,
        key: &swc_ast::PropName,
        val_dest: ValueId,
        block: &mut BasicBlockId,
    ) -> Result<(), LoweringError> {
        let is_proto_key = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.as_ref() == "__proto__",
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().as_ref() == "__proto__",
            _ => false,
        };
        if is_proto_key {
            self.current_function.append_instruction(
                *block,
                Instruction::SetProto {
                    object: obj_dest,
                    value: val_dest,
                },
            );
        } else {
            let key_dest = self.lower_prop_name(key, *block)?;
            *block = self.resolve_store_block(*block);
            self.current_function.append_instruction(
                *block,
                Instruction::SetProp {
                    object: obj_dest,
                    key: key_dest,
                    value: val_dest,
                },
            );
        }
        Ok(())
    }

    fn create_method_env_with_home(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        home_object: ValueId,
    ) -> ValueId {
        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env_val,
                capacity: captured.len() as u32 + 1,
            },
        );

        for binding in captured {
            let current_val = if self.binding_belongs_to_current_function(binding) {
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
            };

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

        let home_key = self
            .module
            .add_constant(Constant::String("home".to_string()));
        let home_key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: home_key_val,
                constant: home_key,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: env_val,
                key: home_key_val,
                value: home_object,
            },
        );

        env_val
    }

    /// 将 getter/setter 方法体编译为内联函数，返回 FunctionRef 的 ValueId
    pub(crate) fn lower_method_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        body: &swc_ast::BlockStmt,
        accessor_params: Option<&[swc_ast::Pat]>,
        home_object: Option<ValueId>,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        // 推入新的函数上下文（使用 push_function_context 管理作用域栈）
        self.push_function_context(&fn_name, BasicBlockId(0));
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
        m_ir_function.set_params(method_param_ir_names);
        let m_captured = self.captured_names_stack.last().unwrap().clone();
        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
        for b in m_blocks {
            m_ir_function.push_block(b);
        }
        let m_function_id = self.module.push_function(m_ir_function);

        self.pop_function_context();

        let m_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(m_function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: m_ref_const,
            },
        );

        if let Some(home_object) = home_object {
            let env_val = self.create_method_env_with_home(block, &m_captured, home_object);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            Ok(closure_val)
        } else if m_captured.is_empty() {
            Ok(func_ref_val)
        } else {
            let mut closure_block = block;
            let env_val = self.ensure_shared_env(closure_block, &m_captured, key.span())?;
            closure_block = self.resolve_store_block(closure_block);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                closure_block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            Ok(closure_val)
        }
    }

    // TODO: lower_method_prop_to_fn 与 lower_fn_expr 的逻辑高度相似
    // （创建函数上下文、声明 $env/$this、构建参数、降低函数体、创建闭包等），
    // 未来应提取共享逻辑以减少代码重复。
    pub(crate) fn lower_method_prop_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        function: &swc_ast::Function,
        home_object: Option<ValueId>,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if function.is_generator {
            let method_name = match key {
                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                _ => "anonymous".to_string(),
            };
            let fn_expr = swc_ast::FnExpr {
                ident: Some(swc_ast::Ident::new(
                    method_name.into(),
                    key.span(),
                    swc_core::common::SyntaxContext::empty(),
                )),
                function: Box::new(function.clone()),
            };
            return self.lower_fn_expr(&fn_expr, block);
        }
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        self.push_function_context(&fn_name, BasicBlockId(0));
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
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        self.pop_function_context();

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let callee_val = if let Some(home_object) = home_object {
            let env_val = self.create_method_env_with_home(block, &captured, home_object);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        } else if captured.is_empty() {
            func_ref_val
        } else {
            let mut closure_block = block;
            let env_val = self.ensure_shared_env(closure_block, &captured, key.span())?;
            closure_block = self.resolve_store_block(closure_block);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                closure_block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
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
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: key_dest,
                value: fn_value,
            },
        );

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
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: enum_key_dest,
                value: enum_val_dest,
            },
        );

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
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: conf_key_dest,
                value: conf_val_dest,
            },
        );

        Ok(desc_dest)
    }
}
