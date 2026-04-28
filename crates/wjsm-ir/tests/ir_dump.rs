use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, Function, Instruction, Module,
    PhiSource, SwitchCaseTarget, Terminator, ValueId,
};

#[test]
fn textual_dump_includes_constants_blocks_and_builtin_calls() {
    let mut module = Module::new();
    let left = module.add_constant(Constant::Number(1.0));
    let right = module.add_constant(Constant::String("hello".to_string()));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.set_terminator(Terminator::Return { value: None });
    entry.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: left,
    });
    entry.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: right,
    });
    entry.push_instruction(Instruction::Binary {
        dest: ValueId(2),
        op: BinaryOp::Add,
        lhs: ValueId(0),
        rhs: ValueId(0),
    });
    entry.push_instruction(Instruction::CallBuiltin {
        dest: None,
        builtin: Builtin::ConsoleLog,
        args: vec![ValueId(1)],
    });
    function.push_block(entry);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = number(1)
    c1 = string(\"hello\")

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      %1 = const c1
      %2 = add %0, %0
      call builtin.console.log(%1)
      return
}
";

    assert_eq!(module.dump_text(), expected);
}

#[test]
fn textual_dump_includes_load_store_undefined() {
    let mut module = Module::new();
    let undef = module.add_constant(Constant::Undefined);
    let one = module.add_constant(Constant::Number(1.0));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.set_terminator(Terminator::Return { value: None });
    // let x; (implicit undefined)
    entry.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: undef,
    });
    entry.push_instruction(Instruction::StoreVar {
        name: "x".to_string(),
        value: ValueId(0),
    });
    // let y = 1;
    entry.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: one,
    });
    entry.push_instruction(Instruction::StoreVar {
        name: "y".to_string(),
        value: ValueId(1),
    });
    // y = y + 1  (load, add, store)
    entry.push_instruction(Instruction::LoadVar {
        dest: ValueId(2),
        name: "y".to_string(),
    });
    entry.push_instruction(Instruction::Const {
        dest: ValueId(3),
        constant: one,
    });
    entry.push_instruction(Instruction::Binary {
        dest: ValueId(4),
        op: BinaryOp::Add,
        lhs: ValueId(2),
        rhs: ValueId(3),
    });
    entry.push_instruction(Instruction::StoreVar {
        name: "y".to_string(),
        value: ValueId(4),
    });
    function.push_block(entry);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = undefined
    c1 = number(1)

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      store var x, %0
      %1 = const c1
      store var y, %1
      %2 = load var y
      %3 = const c1
      %4 = add %2, %3
      store var y, %4
      return
}
";

    assert_eq!(module.dump_text(), expected);
}

#[test]
fn textual_dump_includes_multi_block_cfg() {
    let mut module = Module::new();
    let cond = module.add_constant(Constant::Bool(true));
    let one = module.add_constant(Constant::Number(1.0));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: cond,
    });
    entry.set_terminator(Terminator::Branch {
        condition: ValueId(0),
        true_block: BasicBlockId(1),
        false_block: BasicBlockId(2),
    });

    let mut true_block = BasicBlock::new(BasicBlockId(1));
    true_block.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: one,
    });
    true_block.set_terminator(Terminator::Jump {
        target: BasicBlockId(3),
    });

    let mut false_block = BasicBlock::new(BasicBlockId(2));
    false_block.set_terminator(Terminator::Jump {
        target: BasicBlockId(3),
    });

    let mut merge = BasicBlock::new(BasicBlockId(3));
    merge.set_terminator(Terminator::Return { value: None });

    function.push_block(entry);
    function.push_block(true_block);
    function.push_block(false_block);
    function.push_block(merge);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = bool(true)
    c1 = number(1)

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      branch %0, bb1, bb2
    bb1:
      %1 = const c1
      jump bb3
    bb2:
      jump bb3
    bb3:
      return
}
";

    assert_eq!(module.dump_text(), expected);
}

#[test]
fn textual_dump_includes_phi_sources() {
    let mut module = Module::new();
    let left = module.add_constant(Constant::Number(1.0));
    let right = module.add_constant(Constant::Number(2.0));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut left_block = BasicBlock::new(BasicBlockId(0));
    left_block.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: left,
    });
    left_block.set_terminator(Terminator::Jump {
        target: BasicBlockId(2),
    });

    let mut right_block = BasicBlock::new(BasicBlockId(1));
    right_block.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: right,
    });
    right_block.set_terminator(Terminator::Jump {
        target: BasicBlockId(2),
    });

    let mut merge = BasicBlock::new(BasicBlockId(2));
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

    function.push_block(left_block);
    function.push_block(right_block);
    function.push_block(merge);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = number(1)
    c1 = number(2)

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      jump bb2
    bb1:
      %1 = const c1
      jump bb2
    bb2:
      %2 = phi [(bb0, %0), (bb1, %1)]
      return %2
}
";

    assert_eq!(module.dump_text(), expected);
}

#[test]
fn textual_dump_includes_switch_terminator() {
    let mut module = Module::new();
    let value = module.add_constant(Constant::Number(2.0));
    let one = module.add_constant(Constant::Number(1.0));
    let two = module.add_constant(Constant::Number(2.0));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: value,
    });
    entry.set_terminator(Terminator::Switch {
        value: ValueId(0),
        cases: vec![
            SwitchCaseTarget {
                constant: one,
                target: BasicBlockId(1),
            },
            SwitchCaseTarget {
                constant: two,
                target: BasicBlockId(2),
            },
        ],
        default_block: BasicBlockId(3),
        exit_block: BasicBlockId(4),
    });

    let mut case_one = BasicBlock::new(BasicBlockId(1));
    case_one.set_terminator(Terminator::Jump {
        target: BasicBlockId(4),
    });
    let mut case_two = BasicBlock::new(BasicBlockId(2));
    case_two.set_terminator(Terminator::Jump {
        target: BasicBlockId(4),
    });
    let mut default = BasicBlock::new(BasicBlockId(3));
    default.set_terminator(Terminator::Jump {
        target: BasicBlockId(4),
    });
    let mut exit = BasicBlock::new(BasicBlockId(4));
    exit.set_terminator(Terminator::Return { value: None });

    function.push_block(entry);
    function.push_block(case_one);
    function.push_block(case_two);
    function.push_block(default);
    function.push_block(exit);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = number(2)
    c1 = number(1)
    c2 = number(2)

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      switch %0 [case c1 -> bb1, case c2 -> bb2], default bb3, exit bb4
    bb1:
      jump bb4
    bb2:
      jump bb4
    bb3:
      jump bb4
    bb4:
      return
}
";

    assert_eq!(module.dump_text(), expected);
}

#[test]
fn textual_dump_includes_throw_terminator() {
    let mut module = Module::new();
    let message = module.add_constant(Constant::String("boom".to_string()));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0));
    entry.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: message,
    });
    entry.set_terminator(Terminator::Throw { value: ValueId(0) });
    function.push_block(entry);
    module.push_function(function);

    let expected = "\
module {
  constants:
    c0 = string(\"boom\")

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      throw %0
}
";

    assert_eq!(module.dump_text(), expected);
}
