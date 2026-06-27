use super::*;

impl Lowerer {
    pub(crate) fn eval_scope_bridge_active(&self) -> bool {
        self.eval_mode && self.eval_has_scope_bridge
    }

    pub(crate) fn load_eval_scope_env(&mut self, block: BasicBlockId) -> ValueId {
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env,
                name: EVAL_SCOPE_ENV_PARAM.to_string(),
            },
        );
        env
    }

    pub(crate) fn append_eval_env_key_const(&mut self, block: BasicBlockId, name: &str) -> ValueId {
        let key_const = self.module.add_constant(Constant::String(name.to_string()));
        let key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key,
                constant: key_const,
            },
        );
        key
    }

    pub(crate) fn lower_eval_env_read(
        &mut self,
        name: &str,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let env = self.load_eval_scope_env(block);
        if self.eval_scope_record {
            let name_const = self.module.add_constant(Constant::String(name.to_string()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::EvalGetBinding,
                    args: vec![env, name_val],
                },
            );
            // EvalGetBinding 可能返回 TAG_EXCEPTION（TDZ 等），需检查并传播。
            let cont = self.lower_value_exception_branch(block, dest)?;
            self.eval_continue_block = Some(cont);
            Ok(dest)
        } else {
            let key = self.append_eval_env_key_const(block, name);
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest,
                    object: env,
                    key,
                },
            );
            Ok(dest)
        }
    }

    pub(crate) fn append_eval_env_write(
        &mut self,
        name: &str,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        if !self.eval_scope_bridge_active() {
            return Ok(block);
        }
        let env = self.load_eval_scope_env(block);
        if self.eval_scope_record {
            let name_const = self.module.add_constant(Constant::String(name.to_string()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );
            let set_result = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(set_result),
                    builtin: Builtin::EvalSetBinding,
                    args: vec![env, name_val, value],
                },
            );
            let is_exc = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::IsException {
                    dest: is_exc,
                    value: set_result,
                },
            );
            let ok_block = self.current_function.new_block();
            let exc_block = self.current_function.new_block();
            self.current_function.set_terminator(
                block,
                Terminator::Branch {
                    condition: is_exc,
                    true_block: exc_block,
                    false_block: ok_block,
                },
            );
            let thrown_val = self.alloc_value();
            self.current_function.append_instruction(
                exc_block,
                Instruction::CallBuiltin {
                    dest: Some(thrown_val),
                    builtin: Builtin::ExceptionValue,
                    args: vec![set_result],
                },
            );
            self.emit_throw_value(exc_block, thrown_val)?;
            return Ok(ok_block);
        }
        let key = self.append_eval_env_key_const(block, name);
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: env,
                key,
                value,
            },
        );
        Ok(block)
    }
    pub(super) fn const_val_i64(&mut self, block: BasicBlockId, value: i64) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: self.module.add_constant(Constant::Number(value as f64)),
            },
        );
        dest
    }

    fn expr_is_static_number_receiver(&self, expr: &swc_ast::Expr) -> bool {
        let mut cur = expr;
        loop {
            match cur {
                swc_ast::Expr::Paren(paren) => cur = paren.expr.as_ref(),
                swc_ast::Expr::Lit(swc_ast::Lit::Num(_)) => return true,
                swc_ast::Expr::Unary(unary) => {
                    return matches!(unary.op, swc_ast::UnaryOp::Minus | swc_ast::UnaryOp::Plus)
                        && matches!(unary.arg.as_ref(), swc_ast::Expr::Lit(swc_ast::Lit::Num(_)));
                }
                _ => return false,
            }
        }
    }

    /// `toString` / `valueOf` 对任意 `*.toString()` 误匹配会抢走 Error 等对象；仅数值字面量走快路径。
    /// `toFixed` 等格式方法仍对数值字面量 `(42).toFixed(2)` 保持快路径。
    pub(super) fn should_use_number_proto_call_fast_path(
        &self,
        method: &str,
        receiver: &swc_ast::Expr,
    ) -> bool {
        match method {
            "toString" | "valueOf" => self.expr_is_static_number_receiver(receiver),
            "toFixed" | "toExponential" | "toPrecision" => {
                self.expr_is_static_number_receiver(receiver)
            }
            _ => false,
        }
    }

    /// 检测表达式是否为 Object.prototype.toString 或 Object.prototype.valueOf
    /// 用于优化 Function.prototype.call 调用模式
    pub(super) fn is_object_proto_method_access(&self, expr: &swc_ast::Expr) -> Option<Builtin> {
        // 检测模式: Object.prototype.toString 或 Object.prototype.valueOf
        if let swc_ast::Expr::Member(outer_member) = expr
            && let swc_ast::Expr::Member(inner_member) = outer_member.obj.as_ref()
            && let swc_ast::Expr::Ident(obj_ident) = inner_member.obj.as_ref()
            && obj_ident.sym.as_ref() == "Object"
            && let swc_ast::MemberProp::Ident(proto_prop) = &inner_member.prop
            && proto_prop.sym.as_ref() == "prototype"
            && let swc_ast::MemberProp::Ident(method_prop) = &outer_member.prop
        {
            return match method_prop.sym.as_str() {
                "toString" => Some(Builtin::ObjectProtoToString),
                "valueOf" => Some(Builtin::ObjectProtoValueOf),
                _ => None,
            };
        }
        None
    }
}
