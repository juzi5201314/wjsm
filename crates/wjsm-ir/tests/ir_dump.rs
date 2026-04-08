use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, Function, Instruction, Module,
    Terminator, ValueId,
};

#[test]
fn textual_dump_includes_constants_blocks_and_builtin_calls() {
    let mut module = Module::new();
    let left = module.add_constant(Constant::Number(1.0));
    let right = module.add_constant(Constant::String("hello".to_string()));

    let mut function = Function::new("main", BasicBlockId(0));
    let mut entry = BasicBlock::new(BasicBlockId(0), Terminator::Return { value: None });
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
