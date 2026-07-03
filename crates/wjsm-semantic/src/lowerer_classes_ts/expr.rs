use super::*;

impl Lowerer {
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

        // 命名类表达式：仅在类体内绑定名称（块作用域）。
        let class_body_name_scope =
            if let Some(ref name) = class_expr.ident.as_ref().map(|id| id.sym.to_string()) {
                self.scopes.push_scope(ScopeKind::Block);
                let scope_id = self
                    .scopes
                    .declare(name, VarKind::Const, false)
                    .map_err(|msg| self.error(class_expr.span(), msg))?;
                Some((name.clone(), scope_id))
            } else {
                None
            };

        let decorator_name = class_expr.ident.as_ref().map(|id| id.sym.as_ref());

        let (block, ctor_dest) = self.lower_class_body(
            &class_name,
            &class_expr.class,
            class_expr.span(),
            decorator_name,
            block,
        )?;

        // 命名类表达式：初始化类体绑定并弹出作用域。
        if let Some((ref name, scope_id)) = class_body_name_scope {
            self.scopes
                .mark_initialised(name)
                .map_err(|msg| self.error(class_expr.span(), msg))?;
            let ir_name = format!("${scope_id}.{name}");
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: ir_name,
                    value: ctor_dest,
                },
            );
            self.scopes.pop_scope();
        }

        Ok(ctor_dest)
    }
}
