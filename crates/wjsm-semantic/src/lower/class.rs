use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp,
    ValueId,
};
use crate::scope_tree::{ScopeKind, VarKind, LexicalMode, ScopeTree};
use crate::cfg_builder::{FunctionBuilder, LabelContext, LabelKind, FinallyContext, TryContext, StmtFlow};
use crate::builtins::*;
use crate::eval_helpers::*;
use crate::kind_strings::*;
use crate::{LoweringError, Diagnostic};
use super::lowerer::{Lowerer, ActiveUsingVar, AsyncContextState, HoistedVar, CapturedBinding, EVAL_SCOPE_ENV_PARAM, WK_SYMBOL_ITERATOR, WK_SYMBOL_SPECIES, WK_SYMBOL_TO_STRING_TAG, WK_SYMBOL_ASYNC_ITERATOR, WK_SYMBOL_HAS_INSTANCE, WK_SYMBOL_TO_PRIMITIVE, WK_SYMBOL_DISPOSE, WK_SYMBOL_MATCH, WK_SYMBOL_ASYNC_DISPOSE};

impl Lowerer {
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
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_decl.span(), msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
            }

            // Predeclare hoisted vars in constructor body.
            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        for member in &class_decl.class.body {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    field_block = self.emit_field_init(
                        field_block, this_scope_id, &field_name, prop.value.as_deref(), true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    field_block = self.emit_field_init(
                        field_block, this_scope_id, &prop_name, prop.value.as_deref(), false,
                    )?;
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if !is_static {
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                );
                self.emit_private_method_bind(field_block, this_val, field_name, *func_id);
                field_block = self.resolve_store_block(field_block);
            }
        }

        // Lower constructor body.
        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if let Some(_ctor) = constructor {
        }
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
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
        // 计算非构造函数方法数量，作为原型对象的容量
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

        // For each member, process according to its kind.
        let mut static_init_idx = 0u32;
        for member in &class_decl.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => {
                    match method.kind {
                        swc_ast::MethodKind::Method => {
                            let is_static = method.is_static;
                            let target = if is_static { ctor_dest } else { proto_dest };

                            let method_name = match &method.key {
                                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}", class_name, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));

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
                            // 设置 home_object（实例方法才有 super 访问）
                            if !is_static {
                                m_ir_function.home_object = Some(ctor_function_id);
                            }
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

                            let m_key_const =
                                self.module.add_constant(Constant::String(method_name));
                            let m_key_dest = self.alloc_value();
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: m_key_dest,
                                    constant: m_key_const,
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

                            let method_name = match &method.key {
                                swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                                swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                                _ => continue,
                            };

                            let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                            self.push_function_context(&fn_name, BasicBlockId(0));

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
                            if !is_static {
                                m_ir_function.home_object = Some(ctor_function_id);
                            }
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
                            let m_key_const =
                                self.module.add_constant(Constant::String(method_name));
                            let m_key_dest = self.alloc_value();
                            self.current_function.append_instruction(
                                outer_block,
                                Instruction::Const {
                                    dest: m_key_dest,
                                    constant: m_key_const,
                                },
                            );
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
                        outer_block, ctor_dest, &field_name, prop.value.as_deref(), true,
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
                        outer_block, ctor_dest, &prop_name, prop.value.as_deref(), false,
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
                if let swc_ast::ParamOrTsParamProp::Param(p) = param {
                    if let swc_ast::Pat::Ident(binding_ident) = &p.pat {
                        let name = binding_ident.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(class_expr.span(), msg))?;
                        param_ir_names.push(format!("${scope_id}.{name}"));
                    }
                }
            }

            if let Some(body) = &ctor.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let mut field_block = entry;
        for member in &class_expr.class.body {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::PrivateSet,
                            args: vec![this_val, key_dest, init_val],
                        },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let key_const = self.module.add_constant(Constant::String(prop_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, field_block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            field_block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        field_block,
                        Instruction::SetProp { object: this_val, key: key_dest, value: init_val },
                    );
                    field_block = self.resolve_store_block(field_block);
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if !is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    field_block,
                    Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    field_block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
                );
                self.current_function.append_instruction(
                    field_block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::PrivateSet,
                        args: vec![this_val, key_dest, fn_dest],
                    },
                );
                field_block = self.resolve_store_block(field_block);
            }
        }

        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if let Some(_ctor) = constructor {
        }
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.body {
                for stmt in &body.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
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

        self.pop_function_context();

        // 在当前 block 中创建构造函数 FunctionRef 常量
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

        // Methods (full support for all method kinds, static, and static blocks)
        let mut static_init_idx = 0u32;
        for member in &class_expr.class.body {
            match member {
                swc_ast::ClassMember::Method(method) => match method.kind {
                    swc_ast::MethodKind::Method => {
                        let is_static = method.is_static;
                        let target = if is_static { ctor_dest } else { proto_dest };

                        let method_name = match &method.key {
                            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}", class_name, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));

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
                        if !is_static {
                            m_ir_function.home_object = Some(ctor_function_id);
                        }
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

                        let m_key_const = self.module.add_constant(Constant::String(method_name));
                        let m_key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_key_dest,
                                constant: m_key_const,
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

                        let method_name = match &method.key {
                            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                            _ => continue,
                        };

                        let fn_name = format!("{}.{}_{}", class_name, accessor, method_name);
                        self.push_function_context(&fn_name, BasicBlockId(0));

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
                        if !is_static {
                            m_ir_function.home_object = Some(ctor_function_id);
                        }
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
                        let m_key_const = self.module.add_constant(Constant::String(method_name));
                        let m_key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: m_key_dest,
                                constant: m_key_const,
                            },
                        );
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
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
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
                        Instruction::Const { dest: key_dest, constant: key_const },
                    );
                    let init_val = if let Some(value) = &prop.value {
                        self.lower_expr(value, block)?
                    } else {
                        let ud_const = self.module.add_constant(Constant::Undefined);
                        let ud_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest: ud_dest, constant: ud_const },
                        );
                        ud_dest
                    };
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp { object: ctor_dest, key: key_dest, value: init_val },
                    );
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in &private_method_ids {
            if *is_static {
                let key_const = self.module.add_constant(Constant::String(field_name.clone()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const { dest: key_dest, constant: key_const },
                );
                let fn_dest = self.alloc_value();
                let fn_ref_const = self.module.add_constant(Constant::FunctionRef(*func_id));
                self.current_function.append_instruction(
                    block,
                    Instruction::Const { dest: fn_dest, constant: fn_ref_const },
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

}
