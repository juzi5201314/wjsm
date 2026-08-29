//! `delete` 操作符降级（ES §13.5.1）。
//!
//! 目标按 §13.5.1.2 Evaluation 分类：属性引用（Member / 可选链 / super）、
//! 标识符引用（含 with 对象环境记录、脚本全局、eval 桥），以及非 Reference
//! 操作数（求值弃值后恒返回 true）。严格模式 delete 标识符（§13.5.1.1）已在
//! 降级前由 strict_check 拒绝为 early error；私有成员 delete 在此按 early
//! error 拒绝（§13.5.1.1，所有模式统一成立）。

use super::*;

/// delete 操作数剥掉括号与 TS 类型包装后的引用目标：括号求值透传
/// Reference（§13.2.6.5，早错误的括号规则亦递归适用），TS 的
/// as/!/satisfies 等仅类型层包装，擦除后与括号内表达式同为一个引用。
pub(crate) fn delete_reference_target(expr: &swc_ast::Expr) -> &swc_ast::Expr {
    let mut target = expr;
    loop {
        target = match target {
            swc_ast::Expr::Paren(e) => e.expr.as_ref(),
            swc_ast::Expr::TsAs(e) => e.expr.as_ref(),
            swc_ast::Expr::TsNonNull(e) => e.expr.as_ref(),
            swc_ast::Expr::TsConstAssertion(e) => e.expr.as_ref(),
            swc_ast::Expr::TsTypeAssertion(e) => e.expr.as_ref(),
            swc_ast::Expr::TsSatisfies(e) => e.expr.as_ref(),
            swc_ast::Expr::TsInstantiation(e) => e.expr.as_ref(),
            _ => return target,
        };
    }
}

impl Lowerer {
    pub(crate) fn lower_delete(
        &mut self,
        unary: &swc_ast::UnaryExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match delete_reference_target(unary.arg.as_ref()) {
            // delete obj.prop → DeleteProp 指令。
            swc_ast::Expr::Member(member) => self.lower_delete_member(member, block),
            // delete 可选链（§13.5.1.2）：链短路产出 true，最外环
            // 成员访问发 DeleteProp，调用环求值后恒 true。
            swc_ast::Expr::OptChain(oc) => self.lower_optchain_delete(oc, block),
            // delete super.x：恒抛 ReferenceError（§13.5.1.2 步骤 5.b）。
            swc_ast::Expr::SuperProp(super_prop) => self.lower_delete_super(super_prop, block),
            // delete x：绑定不可删除时返回 false（§9.1.1.1.8）。严格代码中
            // delete 标识符是 early error（§13.5.1.1），已在降级前由
            // strict_check 拒绝，此处只剩 sloppy 路径。
            swc_ast::Expr::Ident(ident) => {
                // 命中 with 对象环境记录时执行 [[Delete]]（§9.1.1.2.7）。
                let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                if !crossed.is_empty() {
                    return self.lower_with_delete(ident, &crossed, block);
                }
                let (value, end_block) =
                    self.lower_delete_ident_fallback(block, ident.sym.as_ref())?;
                self.publish_expr_continuation(block, end_block);
                Ok(value)
            }
            // §13.5.1.2 步骤 1–2：操作数不是 Reference Record（this/字面量/
            // 调用结果/void/逗号表达式等）时求值弃值并返回 true；操作数
            // 求值抛出必须先传播。
            other => {
                let mut current_block = block;
                let _ = self.lower_call_operand_then_continue(other, &mut current_block)?;
                let dest = self.append_bool_const(current_block, true);
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
        }
    }

    fn lower_delete_member(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        // 对象/计算键求值抛出必须在 DeleteProp 前中止并传播。
        let object = self.lower_call_operand_then_continue(&member.obj, &mut current_block)?;
        let key = match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let key_str = ident.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                key_val
            }
            swc_ast::MemberProp::Computed(computed) => {
                self.lower_call_operand_then_continue(&computed.expr, &mut current_block)?
            }
            // §13.5.1.1 早错误：delete 不得作用于私有成员引用（V8 同口径文案）。
            swc_ast::MemberProp::PrivateName(name) => {
                return Err(self.error(name.span, "Private fields can not be deleted"));
            }
        };
        let dest = self.alloc_value();
        // strict 位随降级点静态确定：deleteStatus 为 false 时
        // strict 抛 TypeError（§13.5.5.9 步骤 5.d）。
        self.current_function.append_instruction(
            current_block,
            Instruction::DeleteProp {
                dest,
                object,
                key,
                strict: self.strict_mode,
            },
        );
        self.publish_expr_continuation(block, current_block);
        Ok(dest)
    }

    /// §13.5.1.2 步骤 5.b：delete 的 SuperReference 恒抛 ReferenceError。
    /// SuperProperty 求值（§13.3.7.1）先求计算键并 GetValue，键求值异常先于
    /// 本错误传播（与 V8/Node 同序：键副作用可见之后才抛）。
    fn lower_delete_super(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        if let swc_ast::SuperProp::Computed(computed) = &super_prop.prop {
            let _ = self.lower_call_operand_then_continue(&computed.expr, &mut current_block)?;
        }
        let dummy = self.emit_runtime_error_throw(
            current_block,
            Builtin::ReferenceErrorConstructor,
            "Unsupported reference to 'super'",
        )?;
        self.publish_expr_continuation(block, current_block);
        Ok(dummy)
    }

    /// 裸标识符 delete 的静态回退（无 with 层命中时的 DeleteBinding 裁决）。
    /// 供普通标识符路径与 with 全链未命中的回退共用。返回（结果值, 续接块）。
    pub(crate) fn lower_delete_ident_fallback(
        &mut self,
        block: BasicBlockId,
        name: &str,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        // 脚本全局绑定 / 脚本模式未声明名：全局环境 DeleteBinding
        // （词法与非可配置 var/函数属性返回 false；隐式全局按 [[Delete]]）。
        if self.script_global_kind_for(name).is_some() || self.script_global_dynamic_free_name(name)
        {
            let value = self.lower_script_global_delete(block, name)?;
            return Ok((value, block));
        }
        // eval 桥激活且静态不可解析：移交宿主按调用方作用域记录裁决
        // （声明式绑定 false；全局属性按 [[Delete]]；不可解析名 true）。
        if self.eval_scope_record
            && self.eval_scope_bridge_active()
            && self.scopes.resolve_scope_id(name).is_err()
        {
            return self.lower_eval_delete_binding(name, block);
        }
        let deletable = self.static_ident_deletable(name);
        let value = self.append_bool_const(block, deletable);
        Ok((value, block))
    }

    /// §9.1.1.1.8 DeleteBinding：声明式环境记录的绑定（var/let/const/形参/
    /// 函数名/类名/catch 参/arguments/具名函数表达式名/模块导入）均以
    /// deletable=false 创建，返回 false（TDZ 不影响可删性判定）；唯 eval
    /// 顶层 var 与函数声明按 §19.2.1.3 CreateMutableBinding(N, true) 可删。
    /// 不可解析引用在 sloppy 下返回 true（§13.5.1.2 步骤 3.b）。
    fn static_ident_deletable(&self, name: &str) -> bool {
        // 具名函数表达式自身名字与类自身名字（classEnv）按
        // CreateImmutableBinding 创建，不可删除（§10.2.11 / §15.7.14）。
        if self.fn_expr_name_binding(name).is_some() || self.class_self_name_binding(name).is_some()
        {
            return false;
        }
        // 模块导入别名 / 命名空间局部是不可变间接绑定，不进作用域树。
        if self.current_module_id.is_some_and(|module_id| {
            self.import_aliases
                .contains_key(&(module_id, name.to_string()))
                || self
                    .static_namespace_import_objects
                    .contains_key(&(module_id, name.to_string()))
        }) {
            return false;
        }
        match self.scopes.resolve_binding_any(name) {
            Some((_, kind)) => {
                // 受限全局名（undefined/NaN/Infinity）是预注册绑定，
                // 不属 eval 声明，保持 false。
                self.eval_mode
                    && matches!(kind, VarKind::Var)
                    && !Self::is_restricted_global_name(name)
                    && self.eval_binding_is_top_level(name)
            }
            None => true,
        }
    }
}
