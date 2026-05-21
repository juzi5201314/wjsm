use super::*;

impl Lowerer {
    pub(crate) fn new() -> Self {
        let mut scopes = ScopeTree::new();
        // 预注册 ECMAScript 全局内置标识符
        let _ = scopes.declare("undefined", VarKind::Var, true);
        let _ = scopes.declare("NaN", VarKind::Var, true);
        let _ = scopes.declare("Infinity", VarKind::Var, true);
        let _ = scopes.declare("Symbol", VarKind::Var, true);

        Self {
            module: Module::new(),
            next_value: 0,
            scopes,
            hoisted_vars: Vec::new(),
            hoisted_vars_set: std::collections::HashSet::new(),
            current_function: FunctionBuilder::new("main", BasicBlockId(0)),
            label_stack: Vec::new(),
            finally_stack: Vec::new(),
            try_contexts: Vec::new(),
            next_temp: 0,
            pending_loop_label: None,
            active_finalizers: Vec::new(),
            anon_counter: 0,
            function_stack: Vec::new(),
            function_hoisted_stack: Vec::new(),
            function_next_value_stack: Vec::new(),
            function_next_temp_stack: Vec::new(),
            async_context_stack: Vec::new(),
            function_try_contexts_stack: Vec::new(),
            function_finally_stack_stack: Vec::new(),
            function_label_stack_stack: Vec::new(),
            function_active_finalizers_stack: Vec::new(),
            function_pending_loop_label_stack: Vec::new(),
            captured_names_stack: Vec::new(),
            function_scope_id_stack: Vec::new(),
            is_arrow_fn_stack: Vec::new(),
            shared_env_stack: Vec::new(),
            current_module_id: None,
            import_bindings: std::collections::HashMap::new(),
            export_map: std::collections::HashMap::new(),
            import_aliases: std::collections::HashMap::new(),
            dynamic_import_targets: std::collections::HashMap::new(),
            dynamic_import_namespace_modules: std::collections::HashSet::new(),
            dynamic_import_namespace_objects: std::collections::HashMap::new(),
            dynamic_import_specifier_map: std::collections::HashMap::new(),
            module_export_names: std::collections::HashMap::new(),
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_main_body_entry: None,
            async_main_param_ir_names: Vec::new(),
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
            strict_mode: false,
            script_mode: false,
            eval_mode: false,
            eval_has_scope_bridge: false,
            eval_var_writes_to_scope: false,
            eval_scope_record: false,
            eval_caller_has_arguments: false,
            active_using_vars: Vec::new(),
            typedarray_bindings: std::collections::HashSet::new(),
            eval_continue_block: None,
            eval_completion: None,
        }
    }

    pub(crate) fn capture_async_context(&self) -> AsyncContextState {
        AsyncContextState {
            is_async_fn: self.is_async_fn,
            is_async_generator_fn: self.is_async_generator_fn,
            async_state_counter: self.async_state_counter,
            captured_var_slots: self.captured_var_slots.clone(),
            async_next_continuation_slot: self.async_next_continuation_slot,
            async_resume_blocks: self.async_resume_blocks.clone(),
            async_promise_scope_id: self.async_promise_scope_id,
            async_dispatch_block: self.async_dispatch_block,
            async_env_scope_id: self.async_env_scope_id,
            async_state_scope_id: self.async_state_scope_id,
            async_resume_val_scope_id: self.async_resume_val_scope_id,
            async_is_rejected_scope_id: self.async_is_rejected_scope_id,
            async_generator_scope_id: self.async_generator_scope_id,
            async_closure_env_ir_name: self.async_closure_env_ir_name.clone(),
        }
    }

    pub(crate) fn restore_async_context(&mut self, context: AsyncContextState) {
        self.is_async_fn = context.is_async_fn;
        self.is_async_generator_fn = context.is_async_generator_fn;
        self.async_state_counter = context.async_state_counter;
        self.captured_var_slots = context.captured_var_slots;
        self.async_next_continuation_slot = context.async_next_continuation_slot;
        self.async_resume_blocks = context.async_resume_blocks;
        self.async_promise_scope_id = context.async_promise_scope_id;
        self.async_dispatch_block = context.async_dispatch_block;
        self.async_env_scope_id = context.async_env_scope_id;
        self.async_state_scope_id = context.async_state_scope_id;
        self.async_resume_val_scope_id = context.async_resume_val_scope_id;
        self.async_is_rejected_scope_id = context.async_is_rejected_scope_id;
        self.async_generator_scope_id = context.async_generator_scope_id;
        self.async_closure_env_ir_name = context.async_closure_env_ir_name;
    }

    pub(crate) fn reset_async_context(&mut self) {
        self.restore_async_context(AsyncContextState {
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
        });
    }

    pub(crate) fn push_function_context(&mut self, name: impl Into<String>, entry: BasicBlockId) {
        self.async_context_stack.push(self.capture_async_context());
        self.function_stack.push(std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new(name, entry),
        ));
        // 压入函数作用域到现有作用域树，而非创建新树
        self.scopes.push_scope(ScopeKind::Function);
        // 记录当前函数的 scope id（用于逃逸分析）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false); // 默认非箭头函数，箭头函数会单独设置
        self.shared_env_stack.push(None); // 新函数上下文，尚无共享 env 对象
        self.function_hoisted_stack.push((
            std::mem::take(&mut self.hoisted_vars),
            std::mem::take(&mut self.hoisted_vars_set),
        ));
        self.function_next_value_stack.push(self.next_value);
        self.function_next_temp_stack.push(self.next_temp);
        self.next_value = 0;
        self.next_temp = 0;
        self.function_try_contexts_stack
            .push(std::mem::take(&mut self.try_contexts));
        self.function_finally_stack_stack
            .push(std::mem::take(&mut self.finally_stack));
        self.function_label_stack_stack
            .push(std::mem::take(&mut self.label_stack));
        self.function_active_finalizers_stack
            .push(std::mem::take(&mut self.active_finalizers));
        self.function_pending_loop_label_stack
            .push(self.pending_loop_label.take());
        self.reset_async_context();
    }

    pub(crate) fn pop_function_context(&mut self) {
        self.current_function = self.function_stack.pop().expect("function stack underflow");
        // 弹出函数作用域，回到外层作用域
        self.scopes.pop_scope();
        self.function_scope_id_stack.pop();
        self.captured_names_stack.pop();
        self.is_arrow_fn_stack.pop();
        self.shared_env_stack.pop();
        let (vars, set) = self
            .function_hoisted_stack
            .pop()
            .expect("hoisted stack underflow");
        self.hoisted_vars = vars;
        self.hoisted_vars_set = set;
        self.next_value = self
            .function_next_value_stack
            .pop()
            .expect("next value stack underflow");
        self.next_temp = self
            .function_next_temp_stack
            .pop()
            .expect("next temp stack underflow");
        self.try_contexts = self
            .function_try_contexts_stack
            .pop()
            .expect("try contexts stack underflow");
        self.finally_stack = self
            .function_finally_stack_stack
            .pop()
            .expect("finally stack stack underflow");
        self.label_stack = self
            .function_label_stack_stack
            .pop()
            .expect("label stack stack underflow");
        self.active_finalizers = self
            .function_active_finalizers_stack
            .pop()
            .expect("active finalizers stack underflow");
        self.pending_loop_label = self
            .function_pending_loop_label_stack
            .pop()
            .expect("pending loop label stack underflow");
        let async_context = self
            .async_context_stack
            .pop()
            .expect("async context stack underflow");
        self.restore_async_context(async_context);
    }

    pub(crate) fn current_function_scope_id(&self) -> usize {
        self.function_scope_id_stack.last().copied().unwrap_or(0)
    }

    pub(crate) fn binding_owner_function_scope(&self, binding: &CapturedBinding) -> usize {
        binding
            .scope_id
            .map(|scope_id| self.scopes.function_scope_for_scope(scope_id))
            .unwrap_or_else(|| self.current_function_scope_id())
    }

    pub(crate) fn binding_belongs_to_current_function(&self, binding: &CapturedBinding) -> bool {
        self.binding_owner_function_scope(binding) == self.current_function_scope_id()
    }

    pub(crate) fn record_capture(&mut self, binding: CapturedBinding) {
        if let Some(captured) = self.captured_names_stack.last_mut()
            && !captured.contains(&binding)
        {
            captured.push(binding);
        }
    }

    pub(crate) fn captured_display_names(captured: &[CapturedBinding]) -> Vec<String> {
        captured.iter().map(CapturedBinding::display_name).collect()
    }

    pub(crate) fn is_shared_binding(&self, binding: &CapturedBinding) -> bool {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref())
            .is_some_and(|(_, names)| names.contains(binding))
    }

    pub(crate) fn shared_env_value(&self) -> Option<ValueId> {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref().map(|(value, _)| *value))
    }

    pub(crate) fn append_env_key_const(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> ValueId {
        let key_const = self
            .module
            .add_constant(Constant::String(binding.env_key()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        key_val
    }

    pub(crate) fn load_captured_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let env_val = if self.binding_belongs_to_current_function(binding) {
            self.shared_env_value()
                .expect("shared binding must have a materialized env")
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(block)
        };
        let key_val = self.append_env_key_const(block, binding);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object: env_val,
                key: key_val,
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_module(
        mut self,
        module: &swc_ast::Module,
    ) -> Result<Program, LoweringError> {
        // main 函数也需要 shared_env_stack 条目（顶层闭包需要在 main 中创建 env 对象）
        self.shared_env_stack.push(None);
        self.strict_mode = module_has_use_strict_directive(module);
        // Pre-scan: hoist variable declarations so let/const are in TDZ.
        self.predeclare_stmts(&module.body)?;

        let has_tla = has_top_level_await(module);
        let entry = if has_tla {
            self.init_async_main_context(module.span)?
        } else {
            BasicBlockId(0)
        };
        self.emit_hoisted_var_initializers(entry);

        // 初始化全局内置变量：undefined, NaN, Infinity
        // undefined
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.undefined".to_string(),
                value: undef_val,
            },
        );
        // NaN
        let nan_const = self.module.add_constant(Constant::Number(f64::NAN));
        let nan_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: nan_val,
                constant: nan_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.NaN".to_string(),
                value: nan_val,
            },
        );
        // Infinity
        let inf_const = self.module.add_constant(Constant::Number(f64::INFINITY));
        let inf_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: inf_val,
                constant: inf_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.Infinity".to_string(),
                value: inf_val,
            },
        );

        // Math constants
        let math_constants: [(&str, f64); 8] = [
            ("$0.Math.E", std::f64::consts::E),
            ("$0.Math.LN10", std::f64::consts::LN_10),
            ("$0.Math.LN2", std::f64::consts::LN_2),
            ("$0.Math.LOG10E", std::f64::consts::LOG10_E),
            ("$0.Math.LOG2E", std::f64::consts::LOG2_E),
            ("$0.Math.PI", std::f64::consts::PI),
            ("$0.Math.SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("$0.Math.SQRT2", std::f64::consts::SQRT_2),
        ];
        for (name, val) in math_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

        // Number constants
        let number_constants: [(&str, f64); 8] = [
            ("$0.Number.EPSILON", f64::EPSILON),
            ("$0.Number.MAX_VALUE", f64::MAX),
            ("$0.Number.MIN_VALUE", f64::MIN_POSITIVE),
            ("$0.Number.MAX_SAFE_INTEGER", (1i64 << 53) as f64 - 1.0),
            ("$0.Number.MIN_SAFE_INTEGER", -((1i64 << 53) as f64 - 1.0)),
            ("$0.Number.NaN", f64::NAN),
            ("$0.Number.NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("$0.Number.POSITIVE_INFINITY", f64::INFINITY),
        ];
        for (name, val) in number_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

        // 创建全局对象，用于两种模式下的 builtin 解析和 globalThis
        let global_obj = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(global_obj),
                builtin: Builtin::CreateGlobalObject,
                args: vec![],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.$global".to_string(),
                value: global_obj,
            },
        );

        // 设置 $this：script 模式 = 全局对象，module 模式 = undefined
        let this_val = if self.script_mode {
            global_obj
        } else {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: undef_const,
                },
            );
            v
        };
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$this".to_string(),
                value: this_val,
            },
        );

        let mut flow = StmtFlow::Open(entry);

        for item in &module.body {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    flow = self.lower_stmt(stmt, flow)?;
                }
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    match decl {
                        // export const/let/var/function/class → 将内层声明作为普通语句处理
                        swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                            flow = self
                                .lower_stmt(&swc_ast::Stmt::Decl(export_decl.decl.clone()), flow)?;
                        }
                        // export default expr → 将表达式作为普通语句处理
                        swc_ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                            let expr_stmt = swc_ast::ExprStmt {
                                span: default_expr.span,
                                expr: default_expr.expr.clone(),
                            };
                            flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                        }
                        // export default function/class → 作为声明处理
                        swc_ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                            match &default_decl.decl {
                                swc_ast::DefaultDecl::Fn(fn_expr) => {
                                    if let Some(ident) = &fn_expr.ident {
                                        // export default function foo() {} → 作为命名函数声明处理
                                        let decl = swc_ast::Decl::Fn(swc_ast::FnDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            function: fn_expr.function.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    } else {
                                        // 匿名默认导出函数 — 作为表达式语句求值
                                        let expr_stmt = swc_ast::ExprStmt {
                                            span: default_decl.span,
                                            expr: Box::new(swc_ast::Expr::Fn(fn_expr.clone())),
                                        };
                                        flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                                    }
                                }
                                swc_ast::DefaultDecl::Class(class_expr) => {
                                    if let Some(ident) = &class_expr.ident {
                                        // export default class Foo {} → 作为命名类声明处理
                                        let decl = swc_ast::Decl::Class(swc_ast::ClassDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            class: class_expr.class.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    }
                                    // 匿名默认导出类 — 跳过（无法作为表达式求值）
                                }
                                swc_ast::DefaultDecl::TsInterfaceDecl(_) => {
                                    // TypeScript 接口声明，跳过
                                }
                            }
                        }
                        // import 声明 → 单模块模式下跳过
                        swc_ast::ModuleDecl::Import(_) => {
                            // 单模块模式，跳过 import
                        }
                        // export * from / export { ... } → 暂时跳过
                        _ => {
                            // 暂不处理 re-exports
                        }
                    }
                }
            }
        }

        // If the last block is still open and hasn't been terminated, finalize it.
        match flow {
            StmtFlow::Open(block) => {
                if has_tla {
                    // TLA：resolve promise 然后 return
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let undef_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: undef_val,
                            constant: undef_const,
                        },
                    );
                    let promise_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: promise_val,
                            name: format!("${}.$promise", self.async_promise_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::PromiseResolve {
                            promise: promise_val,
                            value: undef_val,
                        },
                    );
                    self.current_function
                        .set_terminator(block, Terminator::Return { value: None });
                } else {
                    // 非 TLA：检查 unreachable 并设置 Return
                    let is_unreachable = self
                        .current_function
                        .block(block)
                        .is_some_and(|b| matches!(b.terminator(), Terminator::Unreachable));
                    if self.eval_mode {
                        let return_value = if let Some(value) = self.eval_completion {
                            value
                        } else {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let undef_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: undef_val,
                                    constant: undef_const,
                                },
                            );
                            undef_val
                        };
                        self.current_function.set_terminator(
                            block,
                            Terminator::Return {
                                value: Some(return_value),
                            },
                        );
                    } else if is_unreachable {
                        self.current_function
                            .set_terminator(block, Terminator::Return { value: None });
                    }
                }
            }
            StmtFlow::Terminated => {}
        }

        if has_tla {
            self.finalize_async_main()?;
        } else {
            let has_eval = self.current_function.has_eval();
            let blocks = self.current_function.into_blocks();
            let mut function = Function::new("main", BasicBlockId(0));
            function.set_has_eval(has_eval);
            if self.eval_mode {
                function.set_params(vec![EVAL_SCOPE_ENV_PARAM.to_string()]);
            }
            for block in blocks {
                function.push_block(block);
            }
            self.module.push_function(function);
        }
        Ok(self.module)
    }
}

impl Lowerer {
    /// 检查指定 Ident 是否为已知的 TypedArray 绑定。
    pub(crate) fn is_typedarray_binding(&self, ident: &swc_ast::Ident) -> bool {
        let name = ident.sym.to_string();
        if let Ok((scope_id, _)) = self.scopes.lookup(&name) {
            self.typedarray_bindings.contains(&(scope_id, name))
        } else {
            false
        }
    }
}
