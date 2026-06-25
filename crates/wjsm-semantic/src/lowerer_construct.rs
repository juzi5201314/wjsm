use super::*;

impl Lowerer {
    /// ECMAScript [[Construct]] step 12：构造器返回值若为 Object 则作为 `new` 的结果，否则用 `this`。
    pub(crate) fn select_construct_result(
        &mut self,
        block: BasicBlockId,
        ctor_result: ValueId,
        this_val: ValueId,
    ) -> (ValueId, BasicBlockId) {
        let is_obj = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(is_obj),
                builtin: Builtin::IsJsObject,
                args: vec![ctor_result],
            },
        );

        let use_ctor_block = self.current_function.new_block();
        let use_this_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_obj,
                true_block: use_ctor_block,
                false_block: use_this_block,
            },
        );

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
}