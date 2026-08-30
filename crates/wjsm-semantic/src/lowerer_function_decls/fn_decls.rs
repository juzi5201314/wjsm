use super::*;

impl Lowerer {
    pub(crate) fn lower_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if fn_decl.function.is_async && fn_decl.function.is_generator {
            return self.lower_async_gen_fn_decl(fn_decl, flow);
        }
        if fn_decl.function.is_async {
            return self.lower_async_fn_decl(fn_decl, flow);
        }
        if fn_decl.function.is_generator {
            return self.lower_gen_fn_decl(fn_decl, flow);
        }
        let name = fn_decl.ident.sym.to_string();
        self.push_function_context(&name, BasicBlockId(0));
        self.register_fn_decl_self_binding(&name, fn_decl.span())?;
        self.apply_function_strictness(fn_decl.function.body.as_ref());

        // 声明 $env（闭包环境对象），非闭包时传入 undefined
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        // Register $this so that this-keyword expressions resolve.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let param_ir_names = self.build_plain_function_param_ir_names(
            &fn_decl.function,
            env_scope_id,
            this_scope_id,
        )?;

        // Predeclare hoisted vars in the function body.
        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        // Emit parameter initialization (default values + destructuring)
        let body_entry = self.emit_param_inits(&fn_decl.function.params, &param_ir_names, entry)?;

        self.arguments_param_count = Self::count_regular_params(&fn_decl.function.params);
        self.stage_arguments_alias_meta(&fn_decl.function, &param_ir_names);
        let body_entry = self.emit_arguments_init(
            body_entry,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();
        // Lower the function body.
        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        // Add implicit return if the body is still open.
        if let StmtFlow::Open(block) = inner_flow {
            self.current_function
                .set_terminator(block, Terminator::Return { value: None });
        }

        // Finalize the function IR and push it to the module.
        let mut old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let known_callees = old_fn.take_known_callee_vars();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        // SetFunctionLength：按 ExpectedArgumentCount，默认值/rest 不计入。
        ir_function.set_js_length(Self::expected_param_count(&fn_decl.function.params));
        if let Some(span) = self.span_to_source_span(fn_decl.span()) {
            ir_function.set_source_span(span);
        }
        if let Some(text) = self.span_source_text(fn_decl.span()) {
            ir_function.set_source_text(text);
        }
        ir_function.set_params(param_ir_names);
        // 设置捕获变量列表（逃逸分析结果）；剔除函数声明自引用。
        let self_binding = self
            .fn_decl_self_binding_stack
            .last()
            .and_then(|slot| slot.clone());
        let captured = Self::filter_fn_decl_self_captures(
            self.captured_names_stack.last().unwrap(),
            self_binding.as_ref(),
        );
        self.finalize_function_captures(&mut ir_function, &captured);
        for (ir_name, fn_id) in known_callees {
            ir_function.record_known_callee(ir_name, fn_id);
        }
        for block in blocks {
            ir_function.push_block(block);
        }
        ir_function.set_needs_prototype(true);
        let function_id = self.module.push_function(ir_function);

        // Restore the outer function context.
        self.pop_function_context();

        // 在外层函数中 emit 函数引用（闭包或直接 FunctionRef）
        let outer_block = self.ensure_open(flow)?;
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // 如果有捕获变量，使用共享 env 对象 + CreateClosure
        // 为捕获闭包准备 store_block：当 ensure_shared_env 因已有 env 而返回不同 continuation block 时，
        // CreateClosure 的 dest 产生于 closure_block，必须在此 block 上 StoreVar 才能保证 def dominates use，否则 store 读到未初始化值 → 闭包变量为 undefined（shared_mutable / 工厂返回方法等场景）。
        let mut store_block = outer_block;
        let callee_val = if captured.is_empty() {
            // eval 桥下无捕获声明也要携带词法环境（调用方 ScopeRecord / 外层
            // $env 链），否则嵌套函数读调用方绑定时链根缺失。
            if let Some((closure_val, closure_block)) =
                self.materialize_eval_bridge_closure(outer_block, func_ref_val)
            {
                store_block = closure_block;
                closure_val
            } else {
                // 非闭包函数：直接使用 FunctionRef
                func_ref_val
            }
        } else {
            let env_val = self.ensure_shared_env(outer_block, &captured, fn_decl.span())?;
            let closure_block = self.resolve_store_block(outer_block);
            store_block = closure_block; // 关键：必须用 resolve 后的 block 存 StoreVar，保证 CreateClosure 的 dest dominate 后续对 callee_val 的使用
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

        let (scope_id, _) = self
            .scopes
            .lookup(&name)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let store_block =
            self.store_function_decl_callee(store_block, &name, scope_id, callee_val, function_id)?;

        Ok(StmtFlow::Open(store_block))
    }

    pub(super) fn store_function_decl_callee(
        &mut self,
        block: BasicBlockId,
        name: &str,
        scope_id: usize,
        callee_val: ValueId,
        callee_fn_id: wjsm_ir::FunctionId,
    ) -> Result<BasicBlockId, LoweringError> {
        // 脚本全局顶层函数声明：CreateGlobalFunctionBinding 定义全局对象属性，
        // 不再落 `$0.*` 槽（读取统一经 GlobalEnvGet，宿主记录是唯一权威）。
        if scope_id == 0
            && matches!(
                self.script_global_names.get(name),
                Some(ScriptGlobalKind::Func)
            )
        {
            return self.emit_script_global_declare_func(block, name, callee_val);
        }
        // 与形参同名的函数声明（§10.2.11 步骤 36 SetMutableBinding）：绑定
        // 已别名到 arguments 索引属性时改写对象；该绑定可经 arguments[i]
        // 重赋，不得登记 known-callee。
        let decl_binding = CapturedBinding::new(name, scope_id);
        if let Some(alias) = self.mapped_arg_alias(&decl_binding) {
            return self.emit_mapped_arg_write(block, &alias, callee_val);
        }
        let ir_name = format!("${scope_id}.{name}");
        // 记录 callee 变量→FunctionId 映射（Layer 3 callee no-GC 分析）。
        // 仅对函数声明（hoisted，语义不可重赋）安全；async / async-generator 的包装函数
        // 也是通过此路径记录的（它们仍是函数声明，不是 let/const 闭包），
        // 后端分析对该 callee 保守视其函数体 may-GC（闭包 env 可能分配）。
        // eval 桥下声明一律物化为携带记录环境的闭包，direct_call 读取折叠的
        // 契约「绑定值 ≡ FunctionRef 常量」失效（折叠会分裂函数身份），不登记。
        if !self.eval_scope_bridge_active() {
            self.current_function
                .record_known_callee(ir_name.clone(), callee_fn_id);
        }
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: ir_name,
                value: callee_val,
            },
        );

        let binding = CapturedBinding::new(name, scope_id);
        if self.is_shared_binding(&binding) {
            // 函数声明可被自身 direct eval 捕获；共享 env 创建时先快照旧值，声明完成后必须同步新函数值。
            let env_val = self
                .shared_env_value()
                .expect("shared binding must have materialized env");
            let key_val = self.append_env_key_const(block, &binding);
            self.emit_set_prop(block, env_val, key_val, callee_val);
        }

        self.append_eval_var_leak_if_needed(name, VarKind::Var, callee_val, block)
    }
}
