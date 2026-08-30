//! 类自身名字绑定（§15.7.14 ClassDefinitionEvaluation）：
//!
//! 规范在外层词法环境与类体之间插入 classEnv，
//! `CreateImmutableBinding(classBinding, true)` 后在静态元素求值前
//! `InitializeBinding(classBinding, F)`（步骤 29）。classEnv 按【每次求值】
//! 新建——同一 `class C {}` 在循环中多次求值产生互不相干的绑定实例。本实现
//! 复用按迭代绑定的 env 帧机制：类求值点新建原型链接到外层词法环境的 env
//! 对象并写入 TDZ 哨兵；类对象创建、方法安装完成后写为其自有属性，方法体 /
//! 静态元素内的自引用沿闭包 env 原型链读到本次求值的类对象；随后弹出帧与
//! 作用域使名字对外不可见。
//!
//! 写入语义按 CreateImmutableBinding 的 S=true（类体恒为严格代码）：
//! TDZ 内（extends / 计算键期间）写抛 ReferenceError，初始化后写在写点抛
//! 运行时 TypeError，而非 const 的编译期拒绝。

use super::*;

/// 类自身名字作用域的降级句柄：`scope_id` 为名字绑定所在作用域，
/// `has_frame` 表示是否建立了按每次求值的 classEnv 帧（类体静态不可能
/// 引用自身名字时省略帧，避免无谓触发函数级共享 env 的建立）。
pub(crate) struct ClassSelfNameScope {
    has_frame: bool,
}

/// 保守静态扫描：类体（含 heritage / 计算键 / 装饰器 / TS 注解）内是否
/// 可能引用类自身名字。任何同名标识符或 `eval`（direct eval 可动态解析
/// 词法名）命中即为真；过近似只多建一个 classEnv 帧，不影响语义。
fn class_may_reference_self_name(class: &swc_ast::Class, name: &str) -> bool {
    struct Scan<'a> {
        name: &'a str,
        found: bool,
    }
    impl Visit for Scan<'_> {
        fn visit_ident(&mut self, ident: &swc_ast::Ident) {
            if !self.found {
                let sym = ident.sym.as_ref();
                if sym == self.name || sym == "eval" {
                    self.found = true;
                }
            }
        }
    }
    let mut scan = Scan { name, found: false };
    class.visit_with(&mut scan);
    scan.found
}

impl Lowerer {
    /// 解析 `name` 的最近可见绑定；若其为类自身名字绑定则返回。
    /// 同名内层声明（var / let / 形参）遮蔽时最近绑定不带标记，返回 None。
    pub(crate) fn class_self_name_binding(&self, name: &str) -> Option<CapturedBinding> {
        let scope_id = self.scopes.resolve_scope_id(name).ok()?;
        self.scopes
            .is_class_self_name(scope_id, name)
            .then(|| CapturedBinding::new(name, scope_id))
    }

    /// 进入类自身名字作用域：压入块作用域、声明 TDZ 中的不可变绑定；类体
    /// 可能引用自身名字时，再在求值点新建仅含该绑定的 classEnv 帧（按每次
    /// 求值新建，循环内每轮得到独立绑定实例），帧内绑定写入 TDZ 哨兵，类
    /// 求值期（extends / 计算键）内嵌闭包的受检读取据此抛 ReferenceError。
    /// `block` 就地推进到 env 帧建立后的延续块。
    pub(crate) fn begin_class_self_name_scope(
        &mut self,
        name: &str,
        class: &swc_ast::Class,
        block: &mut BasicBlockId,
        span: Span,
    ) -> Result<ClassSelfNameScope, LoweringError> {
        self.scopes.push_scope(ScopeKind::Block);
        let scope_id = self
            .scopes
            .declare(name, VarKind::Const, false)
            .map_err(|msg| self.error(span, msg))?;
        self.scopes
            .set_class_self_name(scope_id, name)
            .map_err(|msg| self.error(span, msg))?;
        let has_frame = class_may_reference_self_name(class, name);
        if has_frame {
            let binding = CapturedBinding::new(name, scope_id);
            let (continuation, frame) = self.prepare_iteration_env(*block, vec![binding])?;
            self.initialize_iteration_env(continuation, &frame, false);
            self.iteration_env_stack.push(frame);
            *block = continuation;
        }
        Ok(ClassSelfNameScope { has_frame })
    }

    /// 类求值完成：弹出 classEnv 帧（若建立过）与名字作用域，名字对外不可见。
    pub(crate) fn finish_class_self_name_scope(&mut self, scope: &ClassSelfNameScope) {
        if scope.has_frame {
            self.iteration_env_stack
                .pop()
                .expect("class self name env frame must be on stack");
        }
        self.scopes.pop_scope();
    }

    /// InitializeBinding(classBinding, F)（步骤 29）：类对象写入本地槽并
    /// 解除 TDZ；classEnv 帧存在时同步写为帧的自有属性（覆盖 TDZ 哨兵），
    /// 方法体 / 静态元素沿闭包 env 原型链读到本次求值的类对象。须在静态
    /// 元素求值前调用。返回写入完成后的延续块。
    pub(crate) fn initialize_class_self_name_binding(
        &mut self,
        block: BasicBlockId,
        class_name: &str,
        scope_id: usize,
        class_val: ValueId,
        span: Span,
    ) -> Result<BasicBlockId, LoweringError> {
        self.scopes
            .set_initialised(scope_id, class_name, true)
            .map_err(|msg| self.error(span, msg))?;
        let binding = CapturedBinding::new(class_name, scope_id);
        let store_block = self.resolve_store_block(block);
        self.current_function.append_instruction(
            store_block,
            Instruction::StoreVar {
                name: binding.var_ir_name(),
                value: class_val,
            },
        );
        if self.iteration_env_for_binding(&binding).is_some() {
            let env = self.load_iteration_env_for_binding(store_block, &binding);
            let key = self.append_env_key_const(store_block, &binding);
            self.emit_set_prop(store_block, env, key, class_val);
        }
        Ok(store_block)
    }

    /// 类自身名字绑定的受检读取：同函数（类求值期，帧可见）经帧 env 读，
    /// 嵌套函数（方法 / 静态元素）沿闭包 env 原型链读；TdzCheck 在值仍为
    /// 未初始化哨兵时抛 ReferenceError。返回（受检值, 续接块）。
    fn load_class_self_name_checked(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let value = if self.iteration_env_for_binding(binding).is_some() {
            self.load_iteration_binding(block, binding)
        } else {
            self.load_captured_binding(block, binding)?
        };
        let current_block = self.resolve_store_block(block);
        self.emit_tdz_check(current_block, value, &binding.name)
    }

    /// 发射不可变绑定写入违例（PutValue → SetMutableBinding，§9.1.1.1.5）：
    /// 绑定未初始化（类求值期）抛 ReferenceError，已初始化抛 TypeError
    /// （S=true，与外围代码严格性无关）。异常经分叉传播（try/catch 可捕获），
    /// 返回静态可达（动态必抛）的正常延续块。
    pub(crate) fn emit_class_self_name_write(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<BasicBlockId, LoweringError> {
        let (_, block) = self.load_class_self_name_checked(block, binding)?;
        let msg_val = self.append_string_const(block, "Assignment to constant variable.");
        let error_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(error_val),
                builtin: Builtin::TypeErrorConstructor,
                args: vec![msg_val],
            },
        );
        let exception_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(exception_val),
                builtin: Builtin::Throw,
                args: vec![error_val],
            },
        );
        self.lower_value_exception_branch(block, exception_val)
    }

    /// 对类自身名字绑定的赋值表达式（§13.15.2 → PutValue 于不可变绑定）：
    /// 简单赋值先求 RHS（副作用保留）再写点分流；复合赋值按 GetValue 先于
    /// RHS 的规范顺序受检读旧值。
    pub(crate) fn lower_assign_class_self_name(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        match assign.op {
            swc_ast::AssignOp::Assign => {
                let mut current_block = block;
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let after = self.emit_class_self_name_write(current_block, binding)?;
                self.expr_merge_block = Some(after);
                Ok(rhs)
            }
            swc_ast::AssignOp::AndAssign
            | swc_ast::AssignOp::OrAssign
            | swc_ast::AssignOp::NullishAssign => {
                self.lower_logical_assign_class_self_name(assign, block, binding)
            }
            op => {
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                let (old_val, mut current_block) =
                    self.load_class_self_name_checked(block, binding)?;
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: old_val,
                        rhs,
                    },
                );
                let after = self.emit_class_self_name_write(current_block, binding)?;
                self.expr_merge_block = Some(after);
                Ok(dest)
            }
        }
    }

    /// 类自身名字绑定的逻辑复合赋值（&&= / ||= / ??=）：受检读旧值短路，
    /// 赋值分支求 RHS 后写点分流，Phi 合并表达式值。
    fn lower_logical_assign_class_self_name(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let (loaded, branch_block) = self.load_class_self_name_checked(block, binding)?;

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                branch_block,
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
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        let mut assign_end = assign_block;
        let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut assign_end)?;
        let assign_end = self.emit_class_self_name_write(assign_end, binding)?;
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: branch_block,
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

    /// 类自身名字绑定的 update 表达式（§13.4 → PutValue 于不可变绑定）：
    /// 受检读旧值（TDZ 内抛 ReferenceError）、ToNumeric（副作用与异常保留），
    /// 写点分流后返回规范结果值。
    pub(crate) fn lower_update_class_self_name(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let (old_val, current_block) = self.load_class_self_name_checked(block, binding)?;
        let (num_val, new_val, math_block) =
            self.append_update_math(current_block, old_val, update.op)?;
        let after = self.emit_class_self_name_write(math_block, binding)?;
        self.expr_merge_block = Some(after);
        Ok(if update.prefix { new_val } else { num_val })
    }

    /// 顶层 `$module_main` 且类只求值一次：不在任何循环内，除本类 classEnv
    /// 外没有其它按迭代 env。循环内每次求值新建构造器，不能折成共享 FunctionRef。
    pub(crate) fn class_eval_is_once(&self, class_self: Option<&CapturedBinding>) -> bool {
        if self.current_function.name() != MODULE_ENTRY_IR_NAME {
            return false;
        }
        // `iteration_env_stack` 在循环头绑定未被捕获时为空；while 条件在
        // `label_stack` 压 Loop 之前求值。loop_depth 覆盖整条循环语句。
        if self.loop_depth != 0
            || self
                .label_stack
                .iter()
                .any(|ctx| ctx.kind == LabelKind::Loop)
        {
            return false;
        }
        self.iteration_env_stack
            .iter()
            .all(|frame| class_self.is_some_and(|binding| frame.bindings.contains(binding)))
    }

    pub(crate) fn push_once_eval_class_ctor(&mut self, binding: CapturedBinding, ctor: FunctionId) {
        self.once_eval_class_ctors.push((binding, ctor));
    }

    pub(crate) fn pop_once_eval_class_ctor(&mut self) {
        self.once_eval_class_ctors.pop();
    }

    pub(crate) fn once_eval_class_ctor_for(&self, binding: &CapturedBinding) -> Option<FunctionId> {
        // 仅方法/构造器/静态块体：计算键箭头在类求值期调用，必须保留 TDZ。
        if !self.is_method {
            return None;
        }
        self.once_eval_class_ctors
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == binding)
            .map(|(_, function_id)| *function_id)
    }
}
