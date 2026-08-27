use super::*;

impl Lowerer {
    pub(crate) fn lower_class_decl(
        &mut self,
        class_decl: &swc_ast::ClassDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let class_name = class_decl.ident.sym.to_string();

        // 类声明名在类体内可见但处于 TDZ 直到求值完成。
        self.scopes.push_scope(ScopeKind::Block);
        let class_body_name_scope_id = self
            .scopes
            .declare(&class_name, VarKind::Const, false)
            .map_err(|msg| self.error(class_decl.span(), msg))?;

        let outer_block = self.ensure_open(flow)?;

        let (outer_block, ctor_dest, ctor_function_id) = self.lower_class_body(
            &class_name,
            &class_decl.class,
            class_decl.span(),
            Some(&class_name),
            outer_block,
        )?;

        // 初始化类体绑定（退出 TDZ，供类体内引用）。
        self.scopes
            .mark_initialised(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        // 收尾存储须同步进共享 env：方法闭包在类求值期间 materialize（此时类名尚未存储），
        // 运行时方法体读到的是 env 中的类名值。
        let class_binding = CapturedBinding::new(&class_name, class_body_name_scope_id);
        if let Some(function_id) = ctor_function_id {
            self.current_function
                .record_known_callee(class_binding.var_ir_name(), function_id);
        }
        let outer_block = self.store_binding_value(
            outer_block,
            &class_binding,
            ctor_dest,
            class_decl.span(),
            true,
        )?;
        self.scopes.pop_scope();

        // 初始化外围作用域绑定（来自 predeclare）。
        self.scopes
            .mark_initialised(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let outer_scope_id = self
            .scopes
            .resolve_scope_id(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let outer_binding = CapturedBinding::new(&class_name, outer_scope_id);
        if let Some(function_id) = ctor_function_id {
            self.current_function
                .record_known_callee(outer_binding.var_ir_name(), function_id);
        }
        // 经 store_binding_value 写入：被闭包捕获（前向引用）时同步共享 env，
        // 覆盖 env 快照中的 TDZ 未初始化哨兵。
        let outer_block = self.store_binding_value(
            outer_block,
            &outer_binding,
            ctor_dest,
            class_decl.span(),
            true,
        )?;

        Ok(StmtFlow::Open(outer_block))
    }
}
