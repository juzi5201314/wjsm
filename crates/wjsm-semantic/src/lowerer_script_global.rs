//! 脚本模式主程序的全局环境建模（ES §9.1.1.4 / §16.1.7）。
//!
//! 顶层 `var`/函数声明进入全局对象记录（globalThis 属性），顶层
//! `let`/`const`/`class` 进入全局声明式记录（宿主 `GlobalEnvRecord`）。
//! 命中脚本全局绑定的标识符读/写/typeof/delete 全部路由到 `GlobalEnv*`
//! builtin，使间接 eval / `new Function` / vm 与主脚本共享同一真值。

use super::*;

impl Lowerer {
    /// 全局对象上恒为非可配置数据属性的受限名（HasRestrictedGlobalProperty）。
    /// 这些名字在 lowering 期以 `$0.*` 常量槽建模，不进入脚本全局路由。
    pub(crate) fn is_restricted_global_name(name: &str) -> bool {
        matches!(name, "undefined" | "NaN" | "Infinity")
    }

    /// predeclare 阶段登记脚本全局声明名。仅脚本模式且声明落在顶层作用域
    /// （scope 0）时生效；函数声明名可覆盖先行 var 登记（GDI 按函数初始化），
    /// 反向（var 重复声明函数名）不降级类别。
    pub(crate) fn record_script_global(&mut self, scope_id: usize, name: &str, kind: VarKind) {
        self.record_script_global_impl(scope_id, name, kind, false);
    }

    /// 登记直接 eval 字面量静态提升出的 var 名：按对象记录 var 类别路由读写，
    /// GDI 建属性时用 configurable=true（§19.2.1.3 CreateGlobalVarBinding(N, true)）。
    /// 显式 var/函数声明命中同名时保持非可配置（不降级为可删除属性）。
    pub(crate) fn record_script_global_eval_var(&mut self, scope_id: usize, name: &str) {
        self.record_script_global_impl(scope_id, name, VarKind::Var, true);
    }

    fn record_script_global_impl(
        &mut self,
        scope_id: usize,
        name: &str,
        kind: VarKind,
        from_eval: bool,
    ) {
        if !self.script_mode || scope_id != 0 || Self::is_restricted_global_name(name) {
            return;
        }
        let entry = match kind {
            VarKind::Var => ScriptGlobalKind::Var,
            VarKind::Let => ScriptGlobalKind::Lexical { is_const: false },
            VarKind::Const => ScriptGlobalKind::Lexical { is_const: true },
        };
        match entry {
            ScriptGlobalKind::Var => {
                if from_eval {
                    // 仅当此名尚无显式登记时才是「eval 专属」的可删除属性。
                    if !self.script_global_names.contains_key(name) {
                        self.script_global_eval_vars.insert(name.to_string());
                    }
                } else {
                    // 显式 var 声明：非可配置属性优先于 eval 静态提升。
                    self.script_global_eval_vars.remove(name);
                }
                if !self.script_global_names.contains_key(name) {
                    self.script_global_names
                        .insert(name.to_string(), ScriptGlobalKind::Var);
                }
                if !self.script_global_vars.iter().any(|n| n == name) {
                    self.script_global_vars.push(name.to_string());
                }
            }
            lexical => {
                self.script_global_names.insert(name.to_string(), lexical);
                self.script_global_lexicals.push((
                    name.to_string(),
                    matches!(lexical, ScriptGlobalKind::Lexical { is_const: true }),
                ));
            }
        }
    }

    /// predeclare 阶段登记顶层函数声明名（CreateGlobalFunctionBinding 类别）。
    pub(crate) fn record_script_global_func(&mut self, scope_id: usize, name: &str) {
        if !self.script_mode || scope_id != 0 || Self::is_restricted_global_name(name) {
            return;
        }
        self.script_global_eval_vars.remove(name);
        self.script_global_names
            .insert(name.to_string(), ScriptGlobalKind::Func);
        if !self.script_global_vars.iter().any(|n| n == name) {
            self.script_global_vars.push(name.to_string());
        }
    }

    /// 名字在当前解析点是否解析到脚本全局绑定（未被内层绑定遮蔽）。
    pub(crate) fn script_global_kind_for(&self, name: &str) -> Option<ScriptGlobalKind> {
        if !self.script_mode {
            return None;
        }
        match self.scopes.resolve_scope_id(name) {
            Ok(0) => self.script_global_names.get(name).copied(),
            _ => None,
        }
    }

    /// 脚本模式下未声明标识符是否路由到动态全局解析（GlobalEnvGet/Set）。
    /// builtin 全局保留既有 `$0.$global` 属性快路径（惰性内建物化）。
    pub(crate) fn script_global_dynamic_free_name(&self, name: &str) -> bool {
        self.script_mode
            && !self.eval_mode
            && self.scopes.resolve_scope_id(name).is_err()
            && !is_builtin_global(name)
            && name != "eval"
    }

    fn load_script_global_object(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: "$0.$global".to_string(),
            },
        );
        dest
    }

    fn append_name_const(&mut self, block: BasicBlockId, name: &str) -> ValueId {
        let constant = self.module.add_constant(Constant::String(name.to_string()));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn append_number_const(&mut self, block: BasicBlockId, value: f64) -> ValueId {
        let constant = self.module.add_constant(Constant::Number(value));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    pub(crate) fn append_bool_const(&mut self, block: BasicBlockId, value: bool) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(value));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// ResolveBinding + GetValue（TDZ / "x is not defined" 由宿主抛出）。
    /// `typeof_tolerant` 时未解析名返回 undefined 而非抛错。
    /// 发射后经异常分叉推进插入点，续接块经 `expr_merge_block` 上报。
    pub(crate) fn lower_script_global_read(
        &mut self,
        block: BasicBlockId,
        name: &str,
        typeof_tolerant: bool,
    ) -> Result<ValueId, LoweringError> {
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        let flags = self.append_number_const(block, if typeof_tolerant { 1.0 } else { 0.0 });
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvGet,
                args: vec![global, name_val, flags],
            },
        );
        let cont = self.lower_value_exception_branch(block, dest)?;
        self.expr_merge_block = Some(cont);
        Ok(dest)
    }

    /// SetMutableBinding / PutValue（TDZ、const TypeError、strict 未解析名
    /// ReferenceError 由宿主抛出）。返回写入完成后的续接块。
    pub(crate) fn emit_script_global_set(
        &mut self,
        block: BasicBlockId,
        name: &str,
        value: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let block = self.resolve_store_block(block);
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        let strict = self.append_bool_const(block, self.strict_mode);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvSet,
                args: vec![global, name_val, value, strict],
            },
        );
        self.lower_value_exception_branch(block, dest)
    }

    /// InitializeBinding：声明语句处写入词法绑定初值并解除 TDZ。
    /// GDI 序幕保证绑定已 DeclareLex，宿主侧不会失败，无需异常分叉。
    pub(crate) fn emit_script_global_init_lex(
        &mut self,
        block: BasicBlockId,
        name: &str,
        value: ValueId,
    ) -> BasicBlockId {
        let block = self.resolve_store_block(block);
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::GlobalEnvInitLex,
                args: vec![global, name_val, value],
            },
        );
        block
    }

    /// CreateGlobalFunctionBinding（§9.1.1.4.18）：顶层函数声明在全局对象上
    /// 定义 {value, writable, enumerable, configurable=false} 并计入 [[VarNames]]。
    /// 全局对象不可扩展且属性缺失时宿主抛 TypeError，需异常分叉。
    pub(crate) fn emit_script_global_declare_func(
        &mut self,
        block: BasicBlockId,
        name: &str,
        value: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let block = self.resolve_store_block(block);
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvDeclareFunc,
                args: vec![global, name_val, value],
            },
        );
        self.lower_value_exception_branch(block, dest)
    }

    /// `delete <ident>`（脚本全局或未声明名）：全局环境 DeleteBinding。
    pub(crate) fn lower_script_global_delete(
        &mut self,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvDelete,
                args: vec![global, name_val],
            },
        );
        Ok(dest)
    }

    /// 脚本全局标识符赋值：简单 / 算术复合 / 逻辑复合。所有读改写均经
    /// GlobalEnvGet/GlobalEnvSet，宿主是绑定语义的唯一权威。
    pub(crate) fn lower_assign_script_global(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        match assign.op {
            swc_ast::AssignOp::Assign => {
                let mut current_block = block;
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                if self.expr_can_throw(assign.right.as_ref()) {
                    current_block = self.lower_value_exception_branch(current_block, rhs)?;
                }
                let after = self.emit_script_global_set(current_block, name, rhs)?;
                self.expr_merge_block = Some(after);
                Ok(rhs)
            }
            swc_ast::AssignOp::AndAssign
            | swc_ast::AssignOp::OrAssign
            | swc_ast::AssignOp::NullishAssign => {
                self.lower_logical_assign_script_global(assign, block, name)
            }
            op => {
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                let loaded = self.lower_script_global_read(block, name, false)?;
                let mut current_block = self.resolve_store_block(block);
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );
                let after = self.emit_script_global_set(current_block, name, dest)?;
                self.expr_merge_block = Some(after);
                Ok(dest)
            }
        }
    }

    /// 脚本全局逻辑复合赋值（&&= / ||= / ??=）：短路时不写回。
    fn lower_logical_assign_script_global(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let loaded = self.lower_script_global_read(block, name, false)?;
        let read_end = self.resolve_store_block(block);

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                read_end,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };
        let (true_target, false_target) = match assign.op {
            swc_ast::AssignOp::AndAssign | swc_ast::AssignOp::NullishAssign => {
                (assign_block, merge)
            }
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            _ => unreachable!("调用方仅在逻辑复合赋值时进入"),
        };
        self.current_function.set_terminator(
            read_end,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        let mut assign_end = assign_block;
        let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut assign_end)?;
        if self.expr_can_throw(assign.right.as_ref()) {
            assign_end = self.lower_value_exception_branch(assign_end, rhs)?;
        }
        let assign_end = self.emit_script_global_set(assign_end, name, rhs)?;
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: read_end,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(result)
    }

    /// 脚本全局 update 表达式（++x / x++ / --x / x--）。
    pub(crate) fn lower_script_global_update(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let old_val = self.lower_script_global_read(block, name, false)?;
        let read_end = self.resolve_store_block(block);
        let (num_val, new_val, math_block) =
            self.append_update_math(read_end, old_val, update.op)?;
        let after = self.emit_script_global_set(math_block, name, new_val)?;
        self.expr_merge_block = Some(after);
        Ok(if update.prefix { new_val } else { num_val })
    }

    /// GlobalDeclarationInstantiation（§16.1.7）序幕：声明冲突预检 →
    /// 词法绑定创建（TDZ）→ 顶层函数声明实例化 → var 属性创建。
    /// 返回序幕结束后的续接块；随后的语句循环需跳过已在此降级的函数声明。
    pub(crate) fn emit_script_gdi_prologue(
        &mut self,
        module_body: &[swc_ast::ModuleItem],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        if !self.script_mode
            || (self.script_global_names.is_empty() && self.script_global_vars.is_empty())
        {
            return Ok(block);
        }
        let mut block = block;

        // 步骤 1–6：声明冲突预检（kind 0 = 词法名，kind 1 = var/函数名）。
        // 同脚本内冲突已在 predeclare 期编译报错；此处防御的是同 realm 先行
        // eval/vm 脚本遗留的全局绑定。
        let lexicals = self.script_global_lexicals.clone();
        let vars = self.script_global_vars.clone();
        for (name, _) in &lexicals {
            block = self.emit_script_global_check(block, name, 0.0)?;
        }
        for name in &vars {
            block = self.emit_script_global_check(block, name, 1.0)?;
        }

        // 步骤 15：词法声明 CreateMutableBinding / CreateImmutableBinding。
        for (name, is_const) in &lexicals {
            let global = self.load_script_global_object(block);
            let name_val = self.append_name_const(block, name);
            let const_val = self.append_bool_const(block, *is_const);
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::GlobalEnvDeclareLex,
                    args: vec![global, name_val, const_val],
                },
            );
        }

        // 步骤 16：顶层函数声明按源码顺序实例化（重名时后者覆盖前者，
        // 与 functionsToInitialize 取最后一个声明一致）。
        let mut flow = StmtFlow::Open(block);
        for item in module_body {
            if let swc_ast::ModuleItem::Stmt(stmt) = item
                && Self::script_top_level_fn_stmt(stmt)
            {
                flow = self.lower_stmt(stmt, flow)?;
            }
        }
        block = self.ensure_open(flow)?;

        // 步骤 18：var 名创建全局属性（函数名已由 CreateGlobalFunctionBinding
        // 建立并计入 [[VarNames]]，跳过）。脚本级 var 恒 configurable=false；
        // 仅由直接 eval 字面量静态提升引入的 var 按 EvalDeclarationInstantiation
        // 的 CreateGlobalVarBinding(N, true) 建可删除属性。
        for name in &vars {
            if matches!(
                self.script_global_names.get(name),
                Some(ScriptGlobalKind::Func)
            ) {
                continue;
            }
            let from_eval_only = self.script_global_eval_vars.contains(name);
            let check_block = self.resolve_store_block(block);
            let global = self.load_script_global_object(check_block);
            let name_val = self.append_name_const(check_block, name);
            let configurable = self.append_bool_const(check_block, from_eval_only);
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                check_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::GlobalEnvDeclareVar,
                    args: vec![global, name_val, configurable],
                },
            );
            block = self.lower_value_exception_branch(check_block, dest)?;
        }
        Ok(block)
    }

    fn emit_script_global_check(
        &mut self,
        block: BasicBlockId,
        name: &str,
        kind: f64,
    ) -> Result<BasicBlockId, LoweringError> {
        let global = self.load_script_global_object(block);
        let name_val = self.append_name_const(block, name);
        let kind_val = self.append_number_const(block, kind);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvCheck,
                args: vec![global, name_val, kind_val],
            },
        );
        self.lower_value_exception_branch(block, dest)
    }

    /// 顶层语句是否为（可带标签的）函数声明——GDI 序幕已将其实例化，
    /// 主语句循环须跳过。标签对函数声明无可观察效果（完成值为 empty）。
    pub(crate) fn script_top_level_fn_stmt(stmt: &swc_ast::Stmt) -> bool {
        match stmt {
            swc_ast::Stmt::Decl(swc_ast::Decl::Fn(_)) => true,
            swc_ast::Stmt::Labeled(labeled) => Self::script_top_level_fn_stmt(&labeled.body),
            _ => false,
        }
    }
}
