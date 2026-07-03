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

        let (outer_block, ctor_dest) = self.lower_class_body(
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
        let body_ir_name = format!("${class_body_name_scope_id}.{class_name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: body_ir_name,
                value: ctor_dest,
            },
        );
        self.scopes.pop_scope();

        // 初始化外围作用域绑定（来自 predeclare）。
        self.scopes
            .mark_initialised(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let outer_scope_id = self
            .scopes
            .resolve_scope_id(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let ir_name = format!("${outer_scope_id}.{class_name}");
        self.current_function.append_instruction(
            outer_block,
            Instruction::StoreVar {
                name: ir_name,
                value: ctor_dest,
            },
        );

        Ok(StmtFlow::Open(outer_block))
    }
}
