use super::*;

impl Lowerer {
    /// 按求值顺序降低 `new` 参数，并推进到可能的异常继续块。
    /// 实参抛出时必须在 [[Construct]] 前中止并传播，不得作为实参值流入构造器。
    pub(crate) fn lower_construct_args(
        &mut self,
        args: Option<&[swc_ast::ExprOrSpread]>,
        block: &mut BasicBlockId,
    ) -> Result<Vec<ValueId>, LoweringError> {
        let Some(args) = args else {
            return Ok(Vec::new());
        };
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.lower_call_operand_then_continue(&arg.expr, block)?);
        }
        Ok(values)
    }

    /// ECMAScript [[Construct]] step 12：构造器返回值若为 Object 则作为 `new` 的结果，否则用 `this`。
    pub(crate) fn select_construct_result(
        &mut self,
        block: BasicBlockId,
        ctor_result: ValueId,
        this_val: ValueId,
    ) -> (ValueId, BasicBlockId) {
        let is_exception = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::IsException {
                dest: is_exception,
                value: ctor_result,
            },
        );

        let use_exception_block = self.current_function.new_block();
        let check_object_block = self.current_function.new_block();
        let use_ctor_block = self.current_function.new_block();
        let use_this_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_exception,
                true_block: use_exception_block,
                false_block: check_object_block,
            },
        );

        let is_obj = self.alloc_value();
        self.current_function.append_instruction(
            check_object_block,
            Instruction::CallBuiltin {
                dest: Some(is_obj),
                builtin: Builtin::IsJsObject,
                args: vec![ctor_result],
            },
        );

        self.current_function.set_terminator(
            check_object_block,
            Terminator::Branch {
                condition: is_obj,
                true_block: use_ctor_block,
                false_block: use_this_block,
            },
        );

        self.current_function
            .set_terminator(use_exception_block, Terminator::Jump { target: merge });
        self.current_function
            .set_terminator(use_ctor_block, Terminator::Jump { target: merge });
        self.current_function
            .set_terminator(use_this_block, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: use_exception_block,
                        value: ctor_result,
                    },
                    PhiSource {
                        predecessor: use_ctor_block,
                        value: ctor_result,
                    },
                    PhiSource {
                        predecessor: use_this_block,
                        value: this_val,
                    },
                ],
            },
        );

        (result, merge)
    }

    /// SuperCall（ES §13.3.7.1）步骤 6–8 的结果选择与 BindThisValue：
    /// 父构造器返回对象时该对象即 `? Construct(...)` 的结果，须重绑当前
    /// this 绑定；返回非对象时结果为传入的 this（与当前绑定相同，重绑为
    /// 幂等写，省略）；异常原样进入合流，由调用方分叉传播。
    pub(crate) fn select_super_call_result(
        &mut self,
        block: BasicBlockId,
        ctor_result: ValueId,
        this_val: ValueId,
    ) -> (ValueId, BasicBlockId) {
        let is_exception = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::IsException {
                dest: is_exception,
                value: ctor_result,
            },
        );

        let use_exception_block = self.current_function.new_block();
        let check_object_block = self.current_function.new_block();
        let bind_block = self.current_function.new_block();
        let use_this_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_exception,
                true_block: use_exception_block,
                false_block: check_object_block,
            },
        );

        let is_obj = self.alloc_value();
        self.current_function.append_instruction(
            check_object_block,
            Instruction::CallBuiltin {
                dest: Some(is_obj),
                builtin: Builtin::IsJsObject,
                args: vec![ctor_result],
            },
        );
        self.current_function.set_terminator(
            check_object_block,
            Terminator::Branch {
                condition: is_obj,
                true_block: bind_block,
                false_block: use_this_block,
            },
        );

        let bind_end = self.emit_bind_this_value(bind_block, ctor_result);

        self.current_function
            .set_terminator(use_exception_block, Terminator::Jump { target: merge });
        self.current_function
            .set_terminator(bind_end, Terminator::Jump { target: merge });
        self.current_function
            .set_terminator(use_this_block, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: use_exception_block,
                        value: ctor_result,
                    },
                    PhiSource {
                        predecessor: bind_end,
                        value: ctor_result,
                    },
                    PhiSource {
                        predecessor: use_this_block,
                        value: this_val,
                    },
                ],
            },
        );

        (result, merge)
    }

    /// 派生构造器的返回裁决（[[Construct]] 步骤 13，ES §10.2.2）：
    /// - 返回对象 → 该对象即构造结果；
    /// - 返回 undefined（含无值 `return;` 与体正常完结）→ 返回当前 this 绑定；
    /// - 返回其它原语 → TypeError。该异常属于 [[Construct]]，在函数体完结之后
    ///   抛出，不可被构造器体内的 try/catch 捕获，故用 Throw 终结子直接向
    ///   调用方传播，不走函数内 handler 路由。
    pub(crate) fn emit_derived_ctor_return(&mut self, block: BasicBlockId, value: Option<ValueId>) {
        let Some(value) = value else {
            let this_val = self.emit_read_ctor_this(block);
            self.current_function.set_terminator(
                block,
                Terminator::Return {
                    value: Some(this_val),
                },
            );
            return;
        };

        let is_obj = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(is_obj),
                builtin: Builtin::IsJsObject,
                args: vec![value],
            },
        );
        let return_value_block = self.current_function.new_block();
        let check_undefined_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_obj,
                true_block: return_value_block,
                false_block: check_undefined_block,
            },
        );
        self.current_function.set_terminator(
            return_value_block,
            Terminator::Return { value: Some(value) },
        );

        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            check_undefined_block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let is_undefined = self.alloc_value();
        self.current_function.append_instruction(
            check_undefined_block,
            Instruction::Compare {
                dest: is_undefined,
                op: CompareOp::StrictEq,
                lhs: value,
                rhs: undef_val,
            },
        );
        let return_this_block = self.current_function.new_block();
        let type_error_block = self.current_function.new_block();
        self.current_function.set_terminator(
            check_undefined_block,
            Terminator::Branch {
                condition: is_undefined,
                true_block: return_this_block,
                false_block: type_error_block,
            },
        );

        let this_val = self.emit_read_ctor_this(return_this_block);
        self.current_function.set_terminator(
            return_this_block,
            Terminator::Return {
                value: Some(this_val),
            },
        );

        let message = self.emit_string_const(
            type_error_block,
            "Derived constructors may only return object or undefined",
        );
        let error_val = self.alloc_value();
        self.current_function.append_instruction(
            type_error_block,
            Instruction::CallBuiltin {
                dest: Some(error_val),
                builtin: Builtin::TypeErrorConstructor,
                args: vec![message],
            },
        );
        self.current_function
            .set_terminator(type_error_block, Terminator::Throw { value: error_val });
    }

    /// BindThisValue：把当前 this 绑定重绑为 `value`。
    ///
    /// - 箭头帧：this 属于最近的非箭头函数，沿 env 原型链定位 owner env 后
    ///   写入（GetThisEnvironment 的环境遍历）。
    /// - 构造器帧（存在箭头 super() 时）：this 的规范存储是共享 env，写 env
    ///   自有 `$this`，同时同步本地槽（后续本帧直线读取与闭包快照一致）。
    /// - 构造器帧（无箭头 super()）：写本地 scoped `$this` 槽即可——env 副本
    ///   只会在此后创建的闭包快照时从本地槽复制。
    fn emit_bind_this_value(&mut self, block: BasicBlockId, value: ValueId) -> BasicBlockId {
        if self.is_arrow {
            let binding = CapturedBinding::lexical_this();
            self.record_capture(binding.clone());
            let start_env = self.load_env_object(block);
            let (owner_block, owner_env) =
                self.resolve_env_binding_owner(block, start_env, &binding);
            let key_val = self.append_env_key_const(owner_block, &binding);
            self.emit_set_prop(owner_block, owner_env, key_val, value);
            return owner_block;
        }
        if self.ctor_this_via_env {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest: env_val,
                    name: self.shared_env_ir_name(),
                },
            );
            let key_val = self.append_env_key_const(block, &CapturedBinding::lexical_this());
            self.emit_set_prop(block, env_val, key_val, value);
        }
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: self.this_var_ir_name(),
                value,
            },
        );
        block
    }
}
