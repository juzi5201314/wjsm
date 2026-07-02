use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, ConstantId, Function, Instruction,
    MODULE_ENTRY_IR_NAME, Module, PhiSource, Terminator, ValueId,
};

#[test]
fn builtin_display_formats_selected_categories_without_panicking() {
    let cases = [
        (Builtin::ConsoleLog, "console.log"),
        (Builtin::ConsoleError, "console.error"),
        (Builtin::ArrayToSorted, "array.to_sorted"),
        (Builtin::ArrayWith, "array.with"),
        (Builtin::ObjectKeys, "object.keys"),
        (Builtin::ObjectDefineProperties, "object.define_properties"),
        (Builtin::ReadableStreamConstructor, "ReadableStream"),
        (Builtin::TransformStreamConstructor, "TransformStream"),
    ];

    for (builtin, expected) in cases {
        assert_eq!(
            builtin.to_string(),
            expected,
            "unexpected display for {builtin:?}"
        );
    }
}

#[test]
fn verifier_accepts_phi_sources_matching_predecessors() {
    let module = branching_phi_module(|merge| {
        merge.push_instruction(Instruction::Phi {
            dest: ValueId(2),
            sources: vec![
                PhiSource {
                    predecessor: BasicBlockId(0),
                    value: ValueId(0),
                },
                PhiSource {
                    predecessor: BasicBlockId(1),
                    value: ValueId(1),
                },
            ],
        });
        merge.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
    });

    assert_verify_ok(module);
}

#[test]
fn verifier_rejects_phi_source_from_non_predecessor() {
    let module = branching_phi_module(|merge| {
        merge.push_instruction(Instruction::Phi {
            dest: ValueId(2),
            sources: vec![
                PhiSource {
                    predecessor: BasicBlockId(0),
                    value: ValueId(0),
                },
                PhiSource {
                    predecessor: BasicBlockId(99),
                    value: ValueId(1),
                },
            ],
        });
        merge.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
    });

    assert_verify_error_contains(module, &["phi", "predecessor"]);
}

#[test]
fn verifier_rejects_undefined_value_use() {
    let mut module = Module::new();
    let constant = module.add_constant(Constant::Number(1.0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(const_value(0, constant));
    entry.push_instruction(Instruction::Binary {
        dest: ValueId(1),
        op: BinaryOp::Add,
        lhs: ValueId(0),
        rhs: ValueId(99),
    });
    entry.set_terminator(Terminator::Return {
        value: Some(ValueId(1)),
    });

    assert_verify_error_contains(
        module_with_existing_module(module, [entry]),
        &["undefined", "%99"],
    );
}

#[test]
fn verifier_rejects_value_used_before_definition_in_same_block() {
    let mut module = Module::new();
    let constant = module.add_constant(Constant::Number(1.0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(const_value(0, constant));
    entry.push_instruction(Instruction::Binary {
        dest: ValueId(1),
        op: BinaryOp::Add,
        lhs: ValueId(0),
        rhs: ValueId(2),
    });
    entry.push_instruction(const_value(2, constant));
    entry.set_terminator(Terminator::Return {
        value: Some(ValueId(1)),
    });

    assert_verify_error_contains(
        module_with_existing_module(module, [entry]),
        &["before", "%2"],
    );
}

#[test]
fn verifier_rejects_block_with_instructions_but_unreachable_terminator() {
    let mut module = Module::new();
    let constant = module.add_constant(Constant::Number(1.0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(const_value(0, constant));

    assert_verify_error_contains(
        module_with_existing_module(module, [entry]),
        &["terminator", "unreachable"],
    );
}

fn branching_phi_module(mut finish_merge: impl FnMut(&mut BasicBlock)) -> Module {
    let mut module = Module::new();
    let zero = module.add_constant(Constant::Number(0.0));
    let one = module.add_constant(Constant::Number(1.0));

    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(const_value(0, zero));
    entry.set_terminator(Terminator::Branch {
        condition: ValueId(0),
        true_block: BasicBlockId(2),
        false_block: BasicBlockId(1),
    });

    let mut alternate = BasicBlock::new(BasicBlockId(1));
    alternate.push_instruction(const_value(1, one));
    alternate.set_terminator(Terminator::Jump {
        target: BasicBlockId(2),
    });

    let mut merge = BasicBlock::new(BasicBlockId(2));
    finish_merge(&mut merge);

    module_with_existing_module(module, [entry, alternate, merge])
}

fn module_with_existing_module<const N: usize>(
    mut module: Module,
    blocks: [BasicBlock; N],
) -> Module {
    let mut function = Function::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0));
    for block in blocks {
        function.push_block(block);
    }
    module.push_function(function);
    module
}

fn const_value(dest: u32, constant: ConstantId) -> Instruction {
    Instruction::Const {
        dest: ValueId(dest),
        constant,
    }
}

fn assert_verify_ok(module: Module) {
    if let Err(error) = module.verify() {
        panic!("expected verifier to accept module, got: {error}");
    }
}

fn assert_verify_error_contains(module: Module, fragments: &[&str]) {
    let Err(error) = module.verify() else {
        panic!("expected verifier to reject module");
    };
    let message = error.to_string();
    let lower_message = message.to_ascii_lowercase();
    for fragment in fragments {
        assert!(
            lower_message.contains(fragment),
            "expected verifier error to contain `{fragment}`, got `{message}`"
        );
    }
}
