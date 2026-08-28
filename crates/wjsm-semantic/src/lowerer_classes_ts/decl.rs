use super::*;

impl Lowerer {
    pub(crate) fn lower_class_decl(
        &mut self,
        class_decl: &swc_ast::ClassDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let class_name = class_decl.ident.sym.to_string();

        // 类声明名在类体内经 classEnv 绑定可见（TDZ 直到静态元素求值前的
        // InitializeBinding）；classEnv 帧按每次求值新建，循环内每轮得到独立
        // 绑定实例（§15.7.14 步骤 3 / 29）。
        let mut outer_block = self.ensure_open(flow)?;
        let self_name_scope = self.begin_class_self_name_scope(
            &class_name,
            &class_decl.class,
            &mut outer_block,
            class_decl.span(),
        )?;

        let (outer_block, ctor_dest, _ctor_function_id) = self.lower_class_body(
            &class_name,
            &class_decl.class,
            class_decl.span(),
            Some(&class_name),
            &class_name,
            outer_block,
        )?;

        // classEnv 绑定已由 lower_class_body 初始化：弹出 classEnv 帧与名字
        // 作用域。类绑定（内层与外围）不进 known_callee_vars：direct_call 的
        // 读取折叠契约是「绑定值 ≡ FunctionRef 常量」，而类对象按每次求值
        // CreateClosure 物化，折叠会使读取绕过本次求值的类对象。
        self.finish_class_self_name_scope(&self_name_scope);

        // 初始化外围作用域绑定（来自 predeclare）。
        self.scopes
            .mark_initialised(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        let outer_scope_id = self
            .scopes
            .resolve_scope_id(&class_name)
            .map_err(|msg| self.error(class_decl.span(), msg))?;
        // 脚本全局类声明：InitializeBinding 写全局声明式记录（解除 TDZ）。
        if outer_scope_id == 0
            && matches!(
                self.script_global_names.get(&class_name),
                Some(ScriptGlobalKind::Lexical { .. })
            )
        {
            let outer_block = self.emit_script_global_init_lex(outer_block, &class_name, ctor_dest);
            return Ok(StmtFlow::Open(outer_block));
        }

        let outer_binding = CapturedBinding::new(&class_name, outer_scope_id);
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
