use super::*;

#[derive(Clone)]
pub(super) struct LoweredClassFunction {
    pub(super) function_id: FunctionId,
    pub(super) captured: Vec<CapturedBinding>,
}

impl Lowerer {
    pub(super) fn materialize_class_function_value(
        &mut self,
        block: BasicBlockId,
        function: &LoweredClassFunction,
        span: Span,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let function_ref = self
            .module
            .add_constant(Constant::FunctionRef(function.function_id));
        let function_value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: function_value,
                constant: function_ref,
            },
        );
        if function.captured.is_empty() {
            // eval 桥下类方法/访问器等函数值同样携带词法环境（链根接调用方记录）。
            if let Some((closure_val, closure_block)) =
                self.materialize_eval_bridge_closure(block, function_value)
            {
                return Ok((closure_block, closure_val));
            }
            return Ok((block, function_value));
        }

        let env = self.ensure_shared_env(block, &function.captured, span)?;
        let continuation = self.resolve_store_block(block);
        let closure = self.alloc_value();
        self.current_function.append_instruction(
            continuation,
            Instruction::CallBuiltin {
                dest: Some(closure),
                builtin: Builtin::CreateClosure,
                args: vec![function_value, env],
            },
        );
        Ok((continuation, closure))
    }
}
