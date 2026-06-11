use super::*;
use swc_core::common::Span;
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Default)]
struct DerivedCtorPreSuperUse {
    seen_super_call: bool,
    invalid_span: Option<Span>,
}

impl Visit for DerivedCtorPreSuperUse {
    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        if self.invalid_span.is_some() {
            return;
        }
        if matches!(call.callee, swc_ast::Callee::Super(_)) {
            for arg in &call.args {
                arg.visit_with(self);
            }
            self.seen_super_call = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_this_expr(&mut self, this_expr: &swc_ast::ThisExpr) {
        if !self.seen_super_call && self.invalid_span.is_none() {
            self.invalid_span = Some(this_expr.span);
        }
    }

    fn visit_super_prop_expr(&mut self, super_prop: &swc_ast::SuperPropExpr) {
        if !self.seen_super_call && self.invalid_span.is_none() {
            self.invalid_span = Some(super_prop.span);
        }
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_arrow_expr(&mut self, _: &swc_ast::ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_ast::Class) {}
}

fn first_pre_super_this_or_super_span(body: &swc_ast::BlockStmt) -> Option<Span> {
    let mut visitor = DerivedCtorPreSuperUse::default();
    body.visit_with(&mut visitor);
    visitor.invalid_span
}
fn stmt_is_direct_super_call(stmt: &swc_ast::Stmt) -> bool {
    matches!(
        stmt,
        swc_ast::Stmt::Expr(expr_stmt)
            if matches!(
                expr_stmt.expr.as_ref(),
                swc_ast::Expr::Call(call)
                    if matches!(call.callee, swc_ast::Callee::Super(_))
            )
    )
}

/// 私有名静态校验（早错误）：
/// 1. AllPrivateIdentifiersValid（ES §13.3.1.1）：任何 `obj.#x` / `#x in obj` 引用都必须
///    出现在声明 `#x` 的某个词法封闭类内，否则为 SyntaxError。
/// 2. ClassBody 私有名重复：同一类体内私有名不得重复声明（同名 getter+setter 各一次的
///    配对除外），否则为 SyntaxError。
///
/// 作为降级前的一次性 AST 遍历执行（模式同 `DerivedCtorPreSuperUse`）。
struct PrivateNameValidator {
    /// 词法作用域栈：进入每个类体时压入其声明的全部私有名集合；引用有效当且仅当其名
    /// 存在于栈中任一层（最近的或更外层的封闭类）。
    scopes: Vec<std::collections::HashSet<String>>,
    error: Option<(Span, String)>,
}

impl PrivateNameValidator {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            error: None,
        }
    }

    /// 收集类体声明的全部私有名，并检测重复声明。返回该类的私有名集合。
    fn collect_class_private_names(
        &mut self,
        class: &swc_ast::Class,
    ) -> std::collections::HashSet<String> {
        use std::collections::HashMap;
        // 每个私有名累计 (值/普通方法 计数, getter 计数, setter 计数)。
        let mut tally: HashMap<String, (u32, u32, u32)> = HashMap::new();
        let mut order: Vec<(String, Span)> = Vec::new();
        for member in &class.body {
            let (name, span, slot) = match member {
                swc_ast::ClassMember::PrivateMethod(m) => {
                    let slot = match m.kind {
                        swc_ast::MethodKind::Getter => 1usize,
                        swc_ast::MethodKind::Setter => 2usize,
                        swc_ast::MethodKind::Method => 0usize,
                    };
                    (m.key.name.to_string(), m.key.span, slot)
                }
                swc_ast::ClassMember::PrivateProp(p) => (p.key.name.to_string(), p.key.span, 0usize),
                _ => continue,
            };
            let entry = tally.entry(name.clone()).or_insert((0, 0, 0));
            match slot {
                0 => entry.0 += 1,
                1 => entry.1 += 1,
                _ => entry.2 += 1,
            }
            order.push((name, span));
        }
        // 重复规则：非访问器名只能出现一次且不可与访问器同名；getter / setter 各至多一次。
        if self.error.is_none() {
            for (name, span) in &order {
                let (values, getters, setters) = tally[name];
                let duplicate = values > 1
                    || (values >= 1 && getters + setters > 0)
                    || getters > 1
                    || setters > 1;
                if duplicate {
                    self.error = Some((
                        *span,
                        format!("Identifier '#{name}' has already been declared"),
                    ));
                    break;
                }
            }
        }
        tally.into_keys().collect()
    }
}

impl Visit for PrivateNameValidator {
    fn visit_class(&mut self, class: &swc_ast::Class) {
        let names = self.collect_class_private_names(class);
        self.scopes.push(names);
        class.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_private_name(&mut self, name: &swc_ast::PrivateName) {
        // 引用（含类体内的声明键）：声明键此时已在作用域内，故仅词法外的引用会报错。
        if self.error.is_none()
            && !self
                .scopes
                .iter()
                .any(|scope| scope.contains(name.name.as_ref()))
        {
            self.error = Some((
                name.span,
                format!(
                    "Private field '#{}' must be declared in an enclosing class",
                    name.name
                ),
            ));
        }
    }
}

/// 对整棵模块 AST 运行私有名静态校验，返回首个早错误（若有）。
pub(crate) fn validate_private_names(module: &swc_ast::Module) -> Result<(), LoweringError> {
    let mut validator = PrivateNameValidator::new();
    module.visit_with(&mut validator);
    if let Some((span, message)) = validator.error {
        return Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0, span.hi.0, message,
        )));
    }
    Ok(())
}

impl Lowerer {
    fn emit_instance_initializers(
        &mut self,
        mut block: BasicBlockId,
        this_scope_id: usize,
        members: &[swc_ast::ClassMember],
        private_method_ids: &[(String, bool, FunctionId)],
    ) -> Result<BasicBlockId, LoweringError> {
        for member in members {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    block = self.emit_field_init(
                        block,
                        this_scope_id,
                        &field_name,
                        prop.value.as_deref(),
                        true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    block = self.emit_field_init(
                        block,
                        this_scope_id,
                        &prop_name,
                        prop.value.as_deref(),
                        false,
                    )?;
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in private_method_ids {
            if !is_static {
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: this_val,
                        name: format!("${this_scope_id}.$this"),
                    },
                );
                self.emit_private_method_bind(block, this_val, field_name, *func_id);
                block = self.resolve_store_block(block);
            }
        }

        Ok(block)
    }

    pub(crate) fn lower_class_decl(
        &mut self,
        class_decl: &swc_ast::ClassDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let class_name = class_decl.ident.sym.to_string();

        let constructor = class_decl
            .class
            .body
            .iter()
            .find_map(|member| match member {
                swc_ast::ClassMember::Constructor(c) => Some(c),
                _ => None,
            });

        let mut private_method_ids: Vec<(String, bool, FunctionId)> = Vec::new();
        for member in &class_decl.class.body {
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
                let m_entry = self.emit_arguments_init(m_entry)?;
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
        self.super_call_allowed = class_decl.class.super_class.is_some();

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        // Register $this as the first param.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_decl.span(), msg))?;

        // Register explicit constructor params.
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
                        .map_err(|msg| self.error(class_decl.span(), msg))?;
                    param_ir_names.push(format!("${scope_id}.{name}"));
                }
            }
            // 派生构造器在 super() 前不能读取 this / super 属性。
            if class_decl.class.super_class.is_some()
                && let Some(body) = &ctor.body
                && let Some(span) = first_pre_super_this_or_super_span(body)
            {
                return Err(self.error(
                    span,
                    "derived constructor cannot access this or super before super()",
                ));
            }

            // Predeclare hoisted vars in constructor body.
            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        if constructor.is_none() && class_decl.class.super_class.is_some() {
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
            constructor.is_some() && class_decl.class.super_class.is_some();
        if !defer_instance_initializers {
            field_block = self.emit_instance_initializers(
                field_block,
                this_scope_id,
                &class_decl.class.body,
                &private_method_ids,
            )?;
        }

        // Lower constructor body.
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
            let args_block = self.emit_arguments_init(match inner_flow {
                StmtFlow::Open(b) => b,
                _ => entry,
            })?;
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
                        &class_decl.class.body,
                        &private_method_ids,
                    )?);
                    deferred_instance_initializers_emitted = true;
                }
            }
        }

        // Implicit return if the body is still open.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize the constructor function.
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
        for block in blocks {
            ir_function.push_block(block);
        }
        let ctor_function_id = self.module.push_function(ir_function);
        if let Some(function) = self.module.function_mut(ctor_function_id) {
            function.home_object = Some(HomeObject::Prototype(ctor_function_id));
        }
        for (_, is_static, func_id) in &private_method_ids {
            if let Some(function) = self.module.function_mut(*func_id) {
                function.home_object = Some(if *is_static {
                    HomeObject::Constructor(ctor_function_id)
                } else {
                    HomeObject::Prototype(ctor_function_id)
                });
            }
        }

        // Restore the outer function context.
        self.pop_function_context();

        let outer_block = self.ensure_open(flow)?;

        // Create constructor FunctionRef constant.
        let ctor_dest = self.alloc_value();
        let ctor_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(ctor_function_id));
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: ctor_dest,
                constant: ctor_ref_const,
            },
        );

        // Create prototype object.
        let proto_dest = self.alloc_value();

        let method_count = class_decl.class.body.iter().filter(|m| {
            matches!(m, swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method))
        }).count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            outer_block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        if let Some(super_class) = &class_decl.class.super_class {
            let super_ctor = self.lower_expr(super_class, outer_block)?;
            let proto_key_const = self
                .module
                .add_constant(Constant::String("prototype".to_string()));
            let proto_key_dest = self.alloc_value();
            self.current_function.append_instruction(
                outer_block,
                Instruction::Const {
                    dest: proto_key_dest,
                    constant: proto_key_const,
                },
            );
            let super_proto = self.alloc_value();
            self.current_function.append_instruction(
                outer_block,
                Instruction::CallBuiltin {
                    dest: Some(super_proto),
                    builtin: Builtin::ReflectGet,
                    args: vec![super_ctor, proto_key_dest, super_ctor],
                },
            );
            self.current_function.append_instruction(
                outer_block,
                Instruction::SetProto {
                    object: proto_dest,
                    value: super_proto,
                },
            );
            self.current_function.append_instruction(
                outer_block,
                Instruction::SetProto {
                    object: ctor_dest,
                    value: super_ctor,
                },
            );
        }

        // For each member, process according to its kind.
        let mut static_init_idx = 0u32;
        for member in &class_decl.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => {
                    match method.kind {
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
                                        outer_block,
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
                                        outer_block,
                                        Instruction::Const {
                                            dest: key_dest,
                                            constant: key_const,
                                        },
                                    );
                                    (name, key_dest)
                                }
                                swc_ast::PropName::Computed(_) => {
                                    let key_dest =
                                        self.lower_prop_name(&method.key, outer_block)?;
                                    ("<computed>".to_string(), key_dest)
                                }
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}", class_name, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));
                            self.is_method = true;
                            self.super_allowed = true;

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
                            let m_entry = self.emit_arguments_init(m_entry)?;
                            self.eval_caller_has_arguments =
                                Self::detect_param_arguments(&method.function.params)
                                    || self.scopes.lookup("arguments").is_ok();

                            let mut m_flow = StmtFlow::Open(m_entry);
                            if let Some(body) = &method.function.body {
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
                            m_ir_function.set_params(method_param_ir_names);
                            let m_captured = self.captured_names_stack.last().unwrap().clone();
                            m_ir_function
                                .set_captured_names(Self::captured_display_names(&m_captured));
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
                                outer_block,
                                Instruction::Const {
                                    dest: m_dest,
                                    constant: m_ref_const,
                                },
                            );
                            self.current_function.append_instruction(
                                outer_block,
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
                                        outer_block,
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
                                        outer_block,
                                        Instruction::Const {
                                            dest: key_dest,
                                            constant: key_const,
                                        },
                                    );
                                    (name, key_dest)
                                }
                                swc_ast::PropName::Computed(_) => {
                                    let key_dest =
                                        self.lower_prop_name(&method.key, outer_block)?;
                                    ("<computed>".to_string(), key_dest)
                                }
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));
                            self.is_method = true;
                            self.super_allowed = true;

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
                            let m_entry = self.emit_arguments_init(m_entry)?;
                            self.eval_caller_has_arguments =
                                Self::detect_param_arguments(&method.function.params)
                                    || self.scopes.lookup("arguments").is_ok();

                            let mut m_flow = StmtFlow::Open(m_entry);
                            if let Some(body) = &method.function.body {
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
                            m_ir_function
                                .set_captured_names(Self::captured_display_names(&m_captured));
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
                                outer_block,
                                Instruction::Const {
                                    dest: fn_dest,
                                    constant: fn_ref_const,
                                },
                            );

                            // Build descriptor and call DefineProperty
                            let desc =
                                self.build_descriptor(accessor, fn_dest, false, true, outer_block)?;
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::CallBuiltin {
                                    dest: None,
                                    builtin: Builtin::DefineProperty,
                                    args: vec![target, m_key_dest, desc],
                                },
                            );
                        }
                    }
                }
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    let fn_name = format!("{}.static_init_{}", class_name, static_init_idx);
                    static_init_idx += 1;

                    self.push_function_context(&fn_name, BasicBlockId(0));
                    self.is_method = true;
                    self.super_allowed = true;

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
                    let m_entry = self.emit_arguments_init(m_entry)?;
                    self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

                    let mut m_flow = StmtFlow::Open(m_entry);
                    for stmt in &static_block.body.stmts {
                        if matches!(m_flow, StmtFlow::Terminated) {
                            continue;
                        }
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

                    // 创建 FunctionRef 并立即调用 Call(ctor, this=ctor)
                    let fn_dest = self.alloc_value();
                    let fn_ref_const = self
                        .module
                        .add_constant(Constant::FunctionRef(m_function_id));
                    self.current_function.append_instruction(
                        outer_block,
                        Instruction::Const {
                            dest: fn_dest,
                            constant: fn_ref_const,
                        },
                    );

                    // Call(fn, this=ctor, args=[])
                    self.current_function.append_instruction(
                        outer_block,
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
                    self.emit_static_field_init(
                        outer_block,
                        ctor_dest,
                        &field_name,
                        prop.value.as_deref(),
                        true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    self.emit_static_field_init(
                        outer_block,
                        ctor_dest,
                        &prop_name,
                        prop.value.as_deref(),
                        false,
                    )?;
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if *is_static {
                self.emit_private_method_bind(outer_block, ctor_dest, field_name, *func_id);
            }
        }

        // Set Foo.prototype = proto_obj.
        let proto_key_const = self
            .module
            .add_constant(Constant::String("prototype".to_string()));
        let proto_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            outer_block,
            Instruction::Const {
                dest: proto_key_dest,
                constant: proto_key_const,
            },
        );
        self.current_function.append_instruction(
            outer_block,
            Instruction::SetProp {
                object: ctor_dest,
                key: proto_key_dest,
                value: proto_dest,
            },
        );

        // Mark class name as initialised to exit TDZ before lookup.
        self.scopes
            .mark_initialised(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;

        // Register class name in module scope with constructor as value.
        let (scope_id, _) = self
            .scopes
            .lookup(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let ir_name = format!("${}.{}", scope_id, class_name);
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: ctor_dest,
            },
        );

        Ok(StmtFlow::Open(outer_block))
    }
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
                let m_entry = self.emit_arguments_init(m_entry)?;
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
            let args_block = self.emit_arguments_init(match inner_flow {
                StmtFlow::Open(b) => b,
                _ => entry,
            })?;
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
                        let m_entry = self.emit_arguments_init(m_entry)?;
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
                        let m_entry = self.emit_arguments_init(m_entry)?;
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
                    let m_entry = self.emit_arguments_init(m_entry)?;
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

        Ok(ctor_dest)
    }
    // ── TypeScript declarations ──────────────────────────────────────────
}
