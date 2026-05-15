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
    pub(crate) fn lower_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match expr {
            swc_ast::Expr::Bin(bin) => self.lower_binary(bin, block),
            swc_ast::Expr::Lit(lit) => self.lower_literal(lit, block),
            swc_ast::Expr::Ident(ident) => self.lower_ident(ident, block),
            swc_ast::Expr::Assign(assign) => self.lower_assign(assign, block),
            swc_ast::Expr::Unary(unary) => self.lower_unary(unary, block),
            swc_ast::Expr::Cond(cond) => self.lower_cond(cond, block),
            swc_ast::Expr::Seq(seq) => self.lower_seq(seq, block),
            swc_ast::Expr::Paren(paren) => self.lower_expr(&paren.expr, block),
            swc_ast::Expr::Call(call) => self.lower_call_expr(call, block),
            swc_ast::Expr::Fn(fn_expr) => self.lower_fn_expr(fn_expr, block),
            swc_ast::Expr::Arrow(arrow) => self.lower_arrow_expr(arrow, block),
            swc_ast::Expr::Object(obj_expr) => self.lower_object_expr(obj_expr, block),
            swc_ast::Expr::Array(arr) => self.lower_array_expr(arr, block),
            swc_ast::Expr::Member(member) => self.lower_member_expr(member, block),
            swc_ast::Expr::This(_) => self.lower_this(block),
            swc_ast::Expr::New(new_expr) => self.lower_new_expr(new_expr, block),
            swc_ast::Expr::Class(class_expr) => self.lower_class_expr(class_expr, block),
            swc_ast::Expr::Update(update) => self.lower_update(update, block),
            swc_ast::Expr::Tpl(tpl) => self.lower_tpl(tpl, block),
            swc_ast::Expr::TaggedTpl(tagged_tpl) => self.lower_tagged_tpl(tagged_tpl, block),
            swc_ast::Expr::SuperProp(super_prop) => self.lower_super_prop(super_prop, block),
            swc_ast::Expr::Await(await_expr) => {
                if !self.is_async_fn {
                    return Err(self.error(expr.span(), "await is only valid in async functions"));
                }
                self.lower_await_expr(await_expr, block)
            }
            swc_ast::Expr::Yield(yield_expr) => self.lower_yield_expr(yield_expr, block),
            // TS type assertion expressions — 编译时类型信息，透传内层表达式
            swc_ast::Expr::TsTypeAssertion(ts_assert) => {
                self.lower_expr(&ts_assert.expr, block)
            }
            swc_ast::Expr::TsConstAssertion(assert) => {
                self.lower_expr(&assert.expr, block)
            }
            swc_ast::Expr::TsNonNull(ts_non_null) => {
                self.lower_expr(&ts_non_null.expr, block)
            }
            swc_ast::Expr::TsAs(ts_as) => {
                self.lower_expr(&ts_as.expr, block)
            }
            swc_ast::Expr::TsSatisfies(ts_satisfies) => {
                self.lower_expr(&ts_satisfies.expr, block)
            }
            swc_ast::Expr::TsInstantiation(ts_inst) => {
                self.lower_expr(&ts_inst.expr, block)
            }
            // JSX expressions
            swc_ast::Expr::JSXElement(jsx_el) => self.lower_jsx_element(jsx_el, block),
            swc_ast::Expr::JSXFragment(jsx_frag) => self.lower_jsx_fragment(jsx_frag, block),
            swc_ast::Expr::JSXEmpty(_) => {
                let null_const = self.module.add_constant(Constant::Null);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest,
                        constant: null_const,
                    },
                );
                Ok(dest)
            }
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    pub(crate) fn lower_tpl(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // quasis (静态文本段) + exprs (动态表达式) 交错: quasi[0] expr[0] quasi[1] expr[1] ... quasi[n]
        let mut parts = Vec::with_capacity(tpl.quasis.len() + tpl.exprs.len());
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            let cooked_str = quasi.cooked.as_ref().ok_or_else(|| {
                self.error(
                    quasi.span,
                    "template string quasi has no cooked value".to_string(),
                )
            })?;
            let cooked_s = cooked_str.to_atom_lossy().to_string();
            let const_id = self.module.add_constant(Constant::String(cooked_s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            parts.push(val);
            if i < tpl.exprs.len() {
                let expr_val = self.lower_expr(&tpl.exprs[i], block)?;
                parts.push(expr_val);
            }
        }
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::StringConcatVa { dest, parts });
        Ok(dest)
    }

    pub(crate) fn lower_tagged_tpl(
        &mut self,
        tagged_tpl: &swc_ast::TaggedTpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let tpl = &tagged_tpl.tpl;
        // 1. 构建 cooked quasi 数组
        let cooked_arr = self.lower_quasis_to_array(tpl, block, false)?;
        // 2. 构建 raw quasi 数组
        let raw_arr = self.lower_quasis_to_array(tpl, block, true)?;
        // 3. Object.defineProperty(cooked_arr, "raw", { value: raw_arr, ... })
        let define_prop_const = self
            .module
            .add_constant(Constant::String("raw".to_string()));
        let define_prop_key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: define_prop_key,
                constant: define_prop_const,
            },
        );
        // 描述符对象: { value: raw_arr, writable: false, enumerable: false, configurable: false }
        let desc_obj_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_obj_val,
                capacity: 4,
            },
        );
        // value
        let value_key = self
            .module
            .add_constant(Constant::String("value".to_string()));
        let value_key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: value_key_val,
                constant: value_key,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_obj_val,
                key: value_key_val,
                value: raw_arr,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::DefineProperty,
                args: vec![cooked_arr, define_prop_key, desc_obj_val],
            },
        );
        // 4. 解析 callee + this_val（复用 lower_call_expr 的逻辑）
        let (callee_val, this_val) = self.lower_tag_expr(&tagged_tpl.tag, block)?;
        // 5. 收集参数: [cooked_arr, ...exprs]
        let mut args = Vec::with_capacity(1 + tpl.exprs.len());
        args.push(cooked_arr);
        for expr in &tpl.exprs {
            let expr_val = self.lower_expr(expr, block)?;
            args.push(expr_val);
        }
        // 6. 发出 Call 指令
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: Some(dest),
                callee: callee_val,
                this_val,
                args,
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_quasis_to_array(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
        raw: bool,
    ) -> Result<ValueId, LoweringError> {
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: tpl.quasis.len() as u32,
            },
        );
        for quasi in &tpl.quasis {
            let s = if raw {
                quasi.raw.as_str().to_string()
            } else {
                let cooked = quasi.cooked.as_ref().ok_or_else(|| {
                    self.error(
                        quasi.span,
                        "template string quasi has no cooked value".to_string(),
                    )
                })?;
                cooked.to_atom_lossy().to_string()
            };
            let const_id = self.module.add_constant(Constant::String(s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, val],
                },
            );
        }
        Ok(arr)
    }

    /// 解析 tagged template 的 tag 表达式，返回 (callee, this_val)。
    /// 复用 lower_call_expr 的 MemberExpression 解析逻辑。
    pub(crate) fn lower_tag_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<(ValueId, ValueId), LoweringError> {
        match expr {
            swc_ast::Expr::Member(member_expr) => {
                let this_val = self.lower_expr(&member_expr.obj, block)?;
                let callee_val = self.lower_member_expr(member_expr, block)?;
                Ok((callee_val, this_val))
            }
            _ => {
                let undef_const = self.module.add_constant(Constant::Undefined);
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: this_val,
                        constant: undef_const,
                    },
                );
                let callee_val = self.lower_expr(expr, block)?;
                Ok((callee_val, this_val))
            }
        }
    }

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

        for prop in &obj_expr.props {
            match prop {
                swc_ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                    swc_ast::Prop::KeyValue(kv) => {
                        let val_dest = self.lower_expr(&kv.value, block)?;
                        self.lower_object_prop(obj_dest, &kv.key, val_dest, block)?;
                    }
                    swc_ast::Prop::Shorthand(ident) => {
                        let val_dest = self.lower_ident(ident, block)?;
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
                        let body = getter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(getter.span, "getter must have a body"))?;
                        let fn_value = self.lower_method_to_fn(&getter.key, body, None, block)?;
                        let desc = self.build_descriptor("get", fn_value, true, true, block)?;
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
                        let body = setter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(setter.span, "setter must have a body"))?;
                        let fn_value =
                            self.lower_method_to_fn(&setter.key, body, Some(true), block)?;
                        let desc = self.build_descriptor("set", fn_value, true, true, block)?;
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
                        let fn_value =
                            self.lower_method_prop_to_fn(&method.key, &method.function, block)?;
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
                    let source = self.lower_expr(&spread.expr, block)?;
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
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        let key_dest = self.lower_prop_name(key, block)?;
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: obj_dest,
                key: key_dest,
                value: val_dest,
            },
        );
        Ok(())
    }

    /// 将 getter/setter 方法体编译为内联函数，返回 FunctionRef 的 ValueId
    pub(crate) fn lower_method_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        body: &swc_ast::BlockStmt,
        _is_setter: Option<bool>,
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

        // 声明 $env 和 $this
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let method_param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        // 预声明提升变量
        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);

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

        // Create FunctionRef
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

        Ok(m_dest)
    }

    // TODO: lower_method_prop_to_fn 与 lower_fn_expr 的逻辑高度相似
    // （创建函数上下文、声明 $env/$this、构建参数、降低函数体、创建闭包等），
    // 未来应提取共享逻辑以减少代码重复。
    pub(crate) fn lower_method_prop_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        function: &swc_ast::Function,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        self.push_function_context(&fn_name, BasicBlockId(0));

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

        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, key.span())?;
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

    pub(crate) fn lower_array_expr(
        &mut self,
        arr: &swc_ast::ArrayLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let elem_count = arr.elems.len();
        // 根据元素数量分配容量（最少 4 个元素槽位减少扩容）
        let capacity = std::cmp::max(4, elem_count as u32);
        let arr_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr_dest,
                capacity,
            },
        );

        // 遍历元素：对每个元素 push 到数组
        for elem in &arr.elems {
            let val = match elem {
                Some(elem) => self.lower_expr(&elem.expr, block)?,
                None => {
                    // 稀疏数组的空位 → undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let val_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val_dest,
                            constant: undef_const,
                        },
                    );
                    val_dest
                }
            };
            // 使用 CallBuiltin(ArrayPush) 添加元素（同时自动更新 length）
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr_dest, val],
                },
            );
        }

        Ok(arr_dest)
    }

    pub(crate) fn lower_member_expr(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 拦截 Math 常量属性访问（Math.PI, Math.E 等）
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop {
            if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref() {
                if obj_ident.sym.to_string() == "Math" && self.scopes.lookup("Math").is_err() {
                    let prop_name = prop_ident.sym.to_string();
                    let math_value: Option<f64> = match prop_name.as_str() {
                        "E" => Some(std::f64::consts::E),
                        "LN10" => Some(std::f64::consts::LN_10),
                        "LN2" => Some(std::f64::consts::LN_2),
                        "LOG10E" => Some(std::f64::consts::LOG10_E),
                        "LOG2E" => Some(std::f64::consts::LOG2_E),
                        "PI" => Some(std::f64::consts::PI),
                        "SQRT1_2" => Some(std::f64::consts::FRAC_1_SQRT_2),
                        "SQRT2" => Some(std::f64::consts::SQRT_2),
                        _ => None,
                    };
                    if let Some(value) = math_value {
                        let c = self.module.add_constant(Constant::Number(value));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest, constant: c },
                        );
                        return Ok(dest);
                    }
                }

                if obj_ident.sym.to_string() == "Number" && self.scopes.lookup("Number").is_err() {
                    let prop_name = prop_ident.sym.to_string();
                    let number_value: Option<f64> = match prop_name.as_str() {
                        "EPSILON" => Some(f64::EPSILON),
                        "MAX_VALUE" => Some(f64::MAX),
                        "MIN_VALUE" => Some(f64::MIN_POSITIVE),
                        "MAX_SAFE_INTEGER" => Some(9007199254740991.0),
                        "MIN_SAFE_INTEGER" => Some(-9007199254740991.0),
                        "NaN" => Some(f64::NAN),
                        "NEGATIVE_INFINITY" => Some(f64::NEG_INFINITY),
                        "POSITIVE_INFINITY" => Some(f64::INFINITY),
                        _ => None,
                    };
                    if let Some(value) = number_value {
                        let c = self.module.add_constant(Constant::Number(value));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const { dest, constant: c },
                        );
                        return Ok(dest);
                    }
                }
            }
        }

        let obj_val = self.lower_expr(&member.obj, block)?;

        let key = match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let key_const = self
                    .module
                    .add_constant(Constant::String(ident.sym.to_string()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                key_dest
            }
            swc_ast::MemberProp::Computed(computed) => self.lower_expr(&computed.expr, block)?,
            swc_ast::MemberProp::PrivateName(name) => {
                let field_name = format!("#{}", name.name);
                let key_const = self.module.add_constant(Constant::String(field_name));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::PrivateGet,
                        args: vec![obj_val, key_dest],
                    },
                );
                return Ok(dest);
            }
        };

        let dest = self.alloc_value();
        match &member.prop {
            // Ident（命名属性）→ GetProp（走原型链，或读取 length 等内置属性）
            // Ident（命名属性）→ 检查是否为 Symbol 的静态属性（如 Symbol.dispose）
            swc_ast::MemberProp::Ident(ident) => {
                // 检查对象是否为 Symbol（编译时已知的 well-known symbol 访问）
                if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref() {
                    if obj_ident.sym.to_string() == "Symbol" {
                        let prop_name = ident.sym.to_string();
                        // 将 Symbol.dispose 等映射为 well-known symbol
                        let wk_index = match prop_name.as_str() {
                            "iterator" => Some(WK_SYMBOL_ITERATOR),
                            "species" => Some(WK_SYMBOL_SPECIES),
                            "toStringTag" => Some(WK_SYMBOL_TO_STRING_TAG),
                            "asyncIterator" => Some(WK_SYMBOL_ASYNC_ITERATOR),
                            "hasInstance" => Some(WK_SYMBOL_HAS_INSTANCE),
                            "toPrimitive" => Some(WK_SYMBOL_TO_PRIMITIVE),
                            "dispose" => Some(WK_SYMBOL_DISPOSE),
                            "match" => Some(WK_SYMBOL_MATCH),
                            "asyncDispose" => Some(WK_SYMBOL_ASYNC_DISPOSE),
                            _ => None,
                        };
                        if let Some(idx) = wk_index {
                            let idx_const = self.module.add_constant(Constant::Number(idx as f64));
                            let idx_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: idx_val,
                                    constant: idx_const,
                                },
                            );
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: Builtin::SymbolWellKnown,
                                    args: vec![idx_val],
                                },
                            );
                            return Ok(dest);
                        }
                    }
                }
                // 默认走 GetProp 路径
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key,
                    },
                );
            }
            // Computed（计算属性）：数字字面量用 GetElem，其他用 GetProp
            swc_ast::MemberProp::Computed(_) => {
                // 检查 computed key 是否为数字字面量 → GetElem
                let use_get_elem = matches!(
                    member.prop,
                    swc_ast::MemberProp::Computed(swc_ast::ComputedPropName { ref expr, .. })
                        if matches!(expr.as_ref(), swc_ast::Expr::Lit(swc_ast::Lit::Num(_)))
                );
                if use_get_elem {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: obj_val,
                            index: key,
                        },
                    );
                } else {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: obj_val,
                            key,
                        },
                    );
                }
            }
            _ => unreachable!(),
        }
        Ok(dest)
    }

    /// 加载当前函数的闭包环境对象（$env 参数）
    pub(crate) fn load_env_object(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        let name = if let Some(ref env_name) = self.async_closure_env_ir_name {
            env_name.clone()
        } else {
            "$env".to_string()
        };
        self.current_function
            .append_instruction(block, Instruction::LoadVar { dest, name });
        dest
    }

    /// 获取或创建当前外层函数的共享 env 对象，并确保所有捕获变量都已写入。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，保证可变捕获变量的修改对所有闭包可见。
    pub(crate) fn ensure_shared_env(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        _span: Span,
    ) -> Result<ValueId, LoweringError> {
        // 步骤 1：读取当前共享 env 状态（不持有引用的情况下）
        let existing_env_val = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(v, _)| *v);
        let existing_names = self
            .shared_env_stack
            .last()
            .unwrap()
            .as_ref()
            .map(|(_, names)| names.clone())
            .unwrap_or_default();

        let env_val = match existing_env_val {
            Some(val) => val,
            None => {
                if captured
                    .iter()
                    .any(|binding| !self.binding_belongs_to_current_function(binding))
                {
                    // 子闭包继续捕获祖先绑定时，复用父 env，保持同一个绑定槽。
                    self.load_env_object(block)
                } else {
                    // 当前函数首次共享本地绑定时创建 env 对象。
                    let env_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::NewObject {
                            dest: env_val,
                            capacity: captured.len() as u32,
                        },
                    );
                    env_val
                }
            }
        };

        // 步骤 2：写入新变量到 env 对象（仅写入尚未存在的变量）
        for binding in captured {
            if existing_names.contains(binding) {
                continue;
            }

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

        // 步骤 3：更新共享 env 状态
        if existing_env_val.is_none() {
            let mut name_set = std::collections::HashSet::new();
            for binding in captured {
                name_set.insert(binding.clone());
            }
            *self.shared_env_stack.last_mut().unwrap() = Some((env_val, name_set));
        } else {
            // 追加新变量名到已有集合
            let shared = self.shared_env_stack.last_mut().unwrap();
            if let Some((_, names)) = shared {
                for binding in captured {
                    names.insert(binding.clone());
                }
            }
        }

        Ok(env_val)
    }

    pub(crate) fn lower_super_prop(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 1. GetSuperBase: 从 home_object 的 proto 读取基类原型
        let base_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::GetSuperBase { dest: base_val });

        // 2. 根据 prop 类型进行属性访问
        match &super_prop.prop {
            swc_ast::SuperProp::Ident(ident_name) => {
                let key_str = ident_name.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: base_val,
                        key: key_dest,
                    },
                );
                Ok(dest)
            }
            swc_ast::SuperProp::Computed(computed) => {
                let key_val = self.lower_expr(&computed.expr, block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetElem {
                        dest,
                        object: base_val,
                        index: key_val,
                    },
                );
                Ok(dest)
            }
        }
    }

    pub(crate) fn lower_this(&mut self, block: BasicBlockId) -> Result<ValueId, LoweringError> {
        // 箭头函数的 this 是词法捕获的，通过 env 对象读取
        let is_arrow = self.is_arrow_fn_stack.last().copied().unwrap_or(false);
        if is_arrow {
            let binding = CapturedBinding::lexical_this();
            self.record_capture(binding.clone());
            // 通过 env 对象读取 this
            let env_val = self.load_env_object(block);
            let key_val = self.append_env_key_const(block, &binding);
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
        } else {
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest,
                    name: "$this".to_string(),
                },
            );
            Ok(dest)
        }
    }

    pub(crate) fn init_async_continuation_slots(&mut self, param_ir_names: &[String], first_param_slot: u32) {
        self.captured_var_slots.clear();
        for (offset, name) in param_ir_names.iter().skip(2).enumerate() {
            self.captured_var_slots
                .insert(name.clone(), first_param_slot + offset as u32);
        }
        self.async_next_continuation_slot =
            first_param_slot + param_ir_names.len().saturating_sub(2) as u32;
    }
    /// 为包含 top-level await 的模块设置 async main 上下文。
    /// 在 entry block (block 0) 中 emit 从 continuation 加载状态的指令，
    /// 创建 dispatch block 和 body entry block，返回 body_entry。
    /// 调用者应使用返回的 body_entry 作为后续 emit 的起始 block。
    pub(crate) fn init_async_main_context(
        &mut self,
        span: swc_core::common::Span,
    ) -> Result<BasicBlockId, LoweringError> {
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        // 为 main 函数设置函数上下文栈（async_visible_binding_names 依赖此栈）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false);

        let entry = BasicBlockId(0);

        // 声明 async 内部变量
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        // 无用户参数，continuation slots 从 4 开始
        self.init_async_continuation_slots(&[], 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        self.async_main_param_ir_names = param_ir_names;

        // ── entry block: 从 continuation 加载状态 ──

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        // continuation slot 0 → $state
        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        // continuation slot 1 → $is_rejected
        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        // $this → $resume_val
        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        // continuation slot 2 → $promise
        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        // continuation slot 3 → $closure_env
        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        // 创建 dispatch block 和 body entry
        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);
        self.async_main_body_entry = Some(body_entry);

        self.current_function.set_terminator(
            entry,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        Ok(body_entry)
    }

    /// 生成 dispatch block，保存 main$async 函数，创建 wrapper main 函数。
    /// 调用前需要确保：
    /// - 模块体的最后一个 block 已正确终止（open block 需要 emit PromiseResolve + Return）
    /// - async_resume_blocks 已填充
    pub(crate) fn finalize_async_main(&mut self) -> Result<(), LoweringError> {
        let dispatch_block = self
            .async_dispatch_block
            .expect("async_dispatch_block not set");
        let body_entry = self
            .async_main_body_entry
            .expect("async_main_body_entry not set");

        // ── 1. 生成 dispatch block（状态机 switch）──
        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${}.$state", self.async_state_scope_id),
                },
            );

            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases = vec![SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            }];

            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }

            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);

            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        // ── 2. 提取 main$async 函数 ──
        let continuation_slot_count = self.async_next_continuation_slot;
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut async_fn = Function::new("main$async", BasicBlockId(0));
        async_fn.set_has_eval(has_eval);
        async_fn.set_params(self.async_main_param_ir_names.clone());
        for b in blocks {
            async_fn.push_block(b);
        }
        let async_fn_id = self.module.push_function(async_fn);

        // ── 3. 创建 wrapper main 函数 ──
        self.next_value = 0;
        self.next_temp = 0;

        let wrapper_entry = BasicBlockId(0);

        // NewPromise
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(wrapper_entry, Instruction::NewPromise { dest: promise_val });

        // FunctionRef for main$async
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // ContinuationCreate(func_ref, promise, slot_count)
        let count_const = self
            .module
            .add_constant(Constant::Number(continuation_slot_count as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![func_ref_val, promise_val, count_val],
            },
        );

        // ContinuationSaveVar slot 2 = promise
        let save_slot2_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot2_val,
                constant: save_slot2_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot2_val, promise_val],
            },
        );

        // ContinuationSaveVar slot 3 = undefined (no closure env)
        let save_slot3_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot3_val,
                constant: save_slot3_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot3_val, undef_val],
            },
        );

        // AsyncFunctionResume(func_ref, continuation, state=0, resume_val=undefined, is_rejected=false)
        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![func_ref_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function
            .set_terminator(wrapper_entry, Terminator::Return { value: None });

        // 提取 wrapper blocks，推入模块
        let wrapper_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let wrapper_has_eval = wrapper_fn.has_eval();
        let wrapper_blocks = wrapper_fn.into_blocks();
        let mut wrapper_ir = Function::new("main", BasicBlockId(0));
        wrapper_ir.set_has_eval(wrapper_has_eval);
        wrapper_ir.set_params(self.async_main_param_ir_names.clone());
        for b in wrapper_blocks {
            wrapper_ir.push_block(b);
        }
        self.module.push_function(wrapper_ir);

        Ok(())
    }

    pub(crate) fn is_async_internal_binding(name: &str) -> bool {
        matches!(
            name,
            "$env"
                | "$this"
                | "$state"
                | "$resume_val"
                | "$is_rejected"
                | "$promise"
                | "$closure_env"
                | "$generator"
        ) || name.starts_with("$tmp.")
    }

    pub(crate) fn async_visible_binding_names(&self) -> Vec<String> {
        let Some(&function_scope_id) = self.function_scope_id_stack.last() else {
            return Vec::new();
        };

        let mut scope_chain = Vec::new();
        let mut cursor = self.scopes.current_scope_id();
        loop {
            scope_chain.push(cursor);
            if cursor == function_scope_id {
                break;
            }
            let Some(parent) = self.scopes.arenas[cursor].parent else {
                break;
            };
            cursor = parent;
        }
        scope_chain.reverse();

        let mut seen = std::collections::HashSet::new();
        let mut bindings = Vec::new();
        for scope_id in scope_chain {
            let scope = &self.scopes.arenas[scope_id];
            let mut names: Vec<String> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if Self::is_async_internal_binding(&name) {
                    continue;
                }
                let ir_name = format!("${scope_id}.{name}");
                if seen.insert(ir_name.clone()) {
                    bindings.push(ir_name);
                }
            }
        }
        bindings
    }

    pub(crate) fn async_binding_slot(&mut self, ir_name: &str) -> u32 {
        if let Some(slot) = self.captured_var_slots.get(ir_name) {
            return *slot;
        }
        let slot = self.async_next_continuation_slot;
        self.async_next_continuation_slot += 1;
        self.captured_var_slots.insert(ir_name.to_string(), slot);
        slot
    }

    pub(crate) fn emit_save_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let continuation = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: continuation,
                name: format!("${}.$env", self.async_env_scope_id),
            },
        );

        for binding in bindings {
            let slot = self.async_binding_slot(binding);
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: value,
                    name: binding.clone(),
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![continuation, slot_val, value],
                },
            );
        }
    }

    pub(crate) fn emit_restore_async_bindings(&mut self, block: BasicBlockId, bindings: &[String]) {
        if bindings.is_empty() {
            return;
        }

        let continuation = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: continuation,
                name: format!("${}.$env", self.async_env_scope_id),
            },
        );

        for binding in bindings {
            let Some(&slot) = self.captured_var_slots.get(binding) else {
                continue;
            };
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(value),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![continuation, slot_val],
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: binding.clone(),
                    value,
                },
            );
        }
    }

    pub(crate) fn lower_await_expr(
        &mut self,
        await_expr: &swc_ast::AwaitExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr(&await_expr.arg, block)?;

        let promised = self.alloc_value();
        {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::PromiseResolveStatic,
                    args: vec![undef_val, value],
                },
            );
        }

        let next_state = self.async_state_counter;
        self.async_state_counter += 1;

        let resume_block = self.current_function.new_block();
        let reject_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();

        self.async_resume_blocks.push((next_state, resume_block));
        let saved_bindings = self.async_visible_binding_names();
        self.emit_save_async_bindings(block, &saved_bindings);

        self.current_function.append_instruction(
            block,
            Instruction::Suspend {
                promise: promised,
                state: next_state,
            },
        );

        self.current_function.set_terminator(
            block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        self.emit_restore_async_bindings(resume_block, &saved_bindings);

        let resume_val = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: resume_val,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let is_rejected = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: is_rejected,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );

        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_rejected,
                true_block: reject_block,
                false_block: continue_block,
            },
        );

        self.emit_throw_value(reject_block, resume_val)?;
        let result = self.alloc_value();
        self.current_function.append_instruction(
            continue_block,
            Instruction::LoadVar {
                dest: result,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );

        Ok(result)
    }

    pub(crate) fn lower_yield_expr(
        &mut self,
        yield_expr: &swc_ast::YieldExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = if let Some(arg) = &yield_expr.arg {
            self.lower_expr(arg, block)?
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

        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: gen_val,
                name: format!("${}.$generator", self.async_generator_scope_id),
            },
        );

        if self.is_async_fn {
            let next_state = self.async_state_counter;
            self.async_state_counter += 1;

            let resume_block = self.current_function.new_block();
            let reject_block = self.current_function.new_block();
            let continue_block = self.current_function.new_block();

            self.async_resume_blocks.push((next_state, resume_block));
            let saved_bindings = self.async_visible_binding_names();
            self.emit_save_async_bindings(block, &saved_bindings);

            let promised = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::AsyncGeneratorNext,
                    args: vec![gen_val, value],
                },
            );

            self.current_function.append_instruction(
                block,
                Instruction::Suspend {
                    promise: promised,
                    state: next_state,
                },
            );

            self.current_function.set_terminator(
                block,
                Terminator::Jump {
                    target: continue_block,
                },
            );

            self.emit_restore_async_bindings(resume_block, &saved_bindings);
            let resume_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: resume_val,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );
            let is_rejected = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: is_rejected,
                    name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
                },
            );

            self.current_function.set_terminator(
                resume_block,
                Terminator::Branch {
                    condition: is_rejected,
                    true_block: reject_block,
                    false_block: continue_block,
                },
            );

            let gen_for_throw = self.alloc_value();
            self.current_function.append_instruction(
                reject_block,
                Instruction::LoadVar {
                    dest: gen_for_throw,
                    name: format!("${}.$generator", self.async_generator_scope_id),
                },
            );
            self.current_function.append_instruction(
                reject_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::AsyncGeneratorThrow,
                    args: vec![gen_for_throw, resume_val],
                },
            );
            self.current_function
                .set_terminator(reject_block, Terminator::Return { value: None });

            let result = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::LoadVar {
                    dest: result,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );

            Ok(result)
        } else {
            let result = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::AsyncGeneratorNext,
                    args: vec![gen_val, value],
                },
            );
            Ok(result)
        }
    }

    pub(crate) fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref() {
            if ident.sym == "Promise" && self.scopes.lookup(&ident.sym).is_err() {
                return self.lower_new_promise(new_expr, block);
            }
            if ident.sym == "Proxy" && self.scopes.lookup(&ident.sym).is_err() {
                // new Proxy(target, handler) → CallBuiltin(ProxyCreate, [target, handler])
                let mut arg_vals = Vec::new();
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        let arg_val = self.lower_expr(&arg.expr, block)?;
                        arg_vals.push(arg_val);
                    }
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ProxyCreate,
                        args: arg_vals,
                    },
                );
                return Ok(dest);
            }
            // Error constructors: new Error(msg), new TypeError(msg), etc.
            if self.scopes.lookup(&ident.sym).is_err() {
                if let Some(builtin) = builtin_from_global_ident(&ident.sym) {
                    if matches!(
                        builtin,
                        Builtin::ErrorConstructor
                            | Builtin::TypeErrorConstructor
                            | Builtin::RangeErrorConstructor
                            | Builtin::SyntaxErrorConstructor
                            | Builtin::ReferenceErrorConstructor
                            | Builtin::URIErrorConstructor
                            | Builtin::EvalErrorConstructor
                            | Builtin::MapConstructor
                            | Builtin::SetConstructor
                            | Builtin::WeakMapConstructor
                            | Builtin::WeakSetConstructor
                            | Builtin::DateConstructor
                            | Builtin::ArrayBufferConstructor
                            | Builtin::DataViewConstructor
                            | Builtin::Int8ArrayConstructor
                            | Builtin::Uint8ArrayConstructor
                            | Builtin::Uint8ClampedArrayConstructor
                            | Builtin::Int16ArrayConstructor
                            | Builtin::Uint16ArrayConstructor
                            | Builtin::Int32ArrayConstructor
                            | Builtin::Uint32ArrayConstructor
                            | Builtin::Float32ArrayConstructor
                            | Builtin::Float64ArrayConstructor
                    ) {
                        let mut arg_vals = Vec::new();
                        if let Some(args) = &new_expr.args {
                            for arg in args {
                                let arg_val = self.lower_expr(&arg.expr, block)?;
                                arg_vals.push(arg_val);
                            }
                        }
                        if arg_vals.is_empty() {
                            arg_vals.push({
                                let c = self.module.add_constant(Constant::Undefined);
                                let dest = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const { dest, constant: c },
                                );
                                dest
                            });
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin,
                                args: arg_vals,
                            },
                        );
                        return Ok(dest);
                    }
                }
            }
        }

        let callee_val = self.lower_expr(&new_expr.callee, block)?;

        // Create new object.
        let obj_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_val,
                capacity: 4,
            },
        );

        // Get prototype from constructor via GetPrototypeFromConstructor builtin.
        // 语义等价于 ECMAScript GetPrototypeFromConstructor(F)：
        // 1. 读取 ctor.prototype（含原型链遍历）
        // 2. 若非 Object 类型（包含 Array、Function、Closure 等），回退到 Object.prototype
        let proto_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(proto_val),
                builtin: Builtin::GetPrototypeFromConstructor,
                args: vec![callee_val],
            },
        );

        // Set __proto__ on the new object directly via SetProto.
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: obj_val,
                value: proto_val,
            },
        );

        // Lower arguments.
        // 性能优化：预分配容量避免循环中多次 reallocation
        let cap = new_expr.args.as_ref().map_or(0, |a| a.len());
        let mut arg_vals = Vec::with_capacity(cap);
        if let Some(args) = &new_expr.args {
            for arg in args {
                let arg_val = self.lower_expr(&arg.expr, block)?;
                arg_vals.push(arg_val);
            }
        }

        // Call the constructor with the new object as `this`.
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: None,
                callee: callee_val,
                this_val: obj_val,
                args: arg_vals,
            },
        );

        Ok(obj_val)
    }

}
