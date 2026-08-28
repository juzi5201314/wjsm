use super::*;

impl Lowerer {
    pub(crate) fn lower_class_expr(
        &mut self,
        class_expr: &swc_ast::ClassExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // NamedEvaluation（ES §8.4.5）：匿名类表达式的构造器 `name` 取绑定名
        // 提示（变量声明/赋值/属性定义等），无提示为空串；命名类表达式取 ident。
        let named_eval = self.named_eval_hint.take();
        // 类表达式可选名称（匿名类表达式无名称）
        let class_name = class_expr
            .ident
            .as_ref()
            .map(|id| id.sym.to_string())
            .unwrap_or_else(|| format!("anon_class_{}", self.anon_counter));
        if class_expr.ident.is_none() {
            self.anon_counter += 1;
        }
        let js_ctor_name = class_expr
            .ident
            .as_ref()
            .map(|id| id.sym.to_string())
            .or(named_eval)
            .unwrap_or_default();

        // 命名类表达式：仅在类体内绑定名称（classEnv 帧按每次求值新建，
        // 循环内每轮得到独立绑定实例）；InitializeBinding 由 lower_class_body
        // 在静态元素求值前完成（§15.7.14 步骤 29）。
        let entry_block = block;
        let mut block = block;
        let class_body_name_scope =
            if let Some(name) = class_expr.ident.as_ref().map(|id| id.sym.to_string()) {
                Some(self.begin_class_self_name_scope(
                    &name,
                    &class_expr.class,
                    &mut block,
                    class_expr.span(),
                )?)
            } else {
                None
            };

        let decorator_name = class_expr.ident.as_ref().map(|id| id.sym.as_ref());

        let (block, ctor_dest, _ctor_function_id) = self.lower_class_body(
            &class_name,
            &class_expr.class,
            class_expr.span(),
            decorator_name,
            &js_ctor_name,
            block,
        )?;

        // 命名类表达式：弹出 classEnv 帧与名字作用域（绑定已由 lower_class_body
        // 初始化）。类绑定不进 known_callee_vars：direct_call 的读取折叠契约是
        // 「绑定值 ≡ FunctionRef 常量」，而类对象按每次求值 CreateClosure 物化。
        if let Some(scope) = &class_body_name_scope {
            self.finish_class_self_name_scope(scope);
        }

        // 类求值（计算键异常分叉、装饰器等）可能推进 block：把延续块发布给
        // 表达式调用方，否则后续指令会落回已终止的入口块。
        if block != entry_block {
            self.expr_merge_block = Some(block);
        }

        Ok(ctor_dest)
    }
}
