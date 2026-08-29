use wjsm_ir::{
    BasicBlock, BasicBlockId, Function, FunctionId, Instruction, Program, Terminator, ValueId,
};
use wjsm_optimize::{
    DeoptMap, PropFact, SlotMap, SpeculativeFacts, optimize_speculative, rewrite_speculative,
};

fn empty_facts(function: FunctionId) -> SpeculativeFacts {
    SpeculativeFacts {
        function,
        param_tags: Box::new([]),
        extra_number_values: Vec::new(),
        get_props: Vec::new(),
        set_props: Vec::new(),
        get_elems: Vec::new(),
        calls: Vec::new(),
        binaries: Vec::new(),
    }
}

#[test]
fn monomorphic_getprop_becomes_guard_and_load_slot() {
    let mut program = Program::new();
    let key = program.add_constant(wjsm_ir::Constant::String("x".into()));
    let mut function = Function::new("hot", BasicBlockId(0));
    function.set_params(vec!["$env".into(), "$this".into(), "o".into()]);
    let mut block = BasicBlock::new(BasicBlockId(0));
    block.push_instruction(Instruction::LoadVar {
        dest: ValueId(0),
        name: "o".into(),
    });
    block.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: key,
    });
    block.push_instruction(Instruction::GetProp {
        dest: ValueId(2),
        object: ValueId(0),
        key: ValueId(1),
        latch: None,
        latch_template: None,
    });
    block.set_terminator(Terminator::Return {
        value: Some(ValueId(2)),
    });
    function.push_block(block);
    program.push_function(function);

    let mut facts = empty_facts(FunctionId(0));
    facts.get_props.push(PropFact {
        block: BasicBlockId(0),
        instruction_index: 2,
        shape_id: 7,
        slot_index: 0,
        proto_generation: 0,
        expected_proto: 0,
        own_data: true,
        poly_len: 1,
        poly_shapes: [7, 0, 0, 0],
        poly_slots: [0, 0, 0, 0],
        transition_shape: None,
    });
    let mut deopt_map = DeoptMap::default();
    let mut slot_map = SlotMap::default();
    rewrite_speculative(&mut program, &facts, &mut deopt_map, &mut slot_map);
    let rewritten = &program.functions()[0];
    let has_get_prop = rewritten.blocks().iter().any(|block| {
        block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::GetProp { .. }))
    });
    let has_guard = rewritten.blocks().iter().any(|block| {
        block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::GuardShape { shape_id: 7, .. }))
    });
    let has_load = rewritten.blocks().iter().any(|block| {
        block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadSlot { index: 0, .. }))
    });
    let has_deopt = rewritten
        .blocks()
        .iter()
        .any(|block| matches!(block.terminator(), Terminator::Deopt { .. }));
    assert!(!has_get_prop);
    assert!(has_guard);
    assert!(has_load);
    assert!(has_deopt);
    assert!(!deopt_map.points.is_empty());
}

#[test]
fn optimize_speculative_keeps_program_verifiable() {
    let mut program = Program::new();
    let mut function = Function::new("main", BasicBlockId(0));
    let mut block = BasicBlock::new(BasicBlockId(0));
    block.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: program.add_constant(wjsm_ir::Constant::Number(1.0)),
    });
    block.set_terminator(Terminator::Return {
        value: Some(ValueId(0)),
    });
    function.push_block(block);
    program.push_function(function);
    let facts = empty_facts(FunctionId(0));
    let unit = optimize_speculative(&mut program, &facts);
    unit.program
        .verify()
        .expect("sound+speculative IR remains verifiable");
}

#[test]
fn monomorphic_call_becomes_guard_call_target() {
    let mut program = Program::new();
    let mut callee = Function::new("inner", BasicBlockId(0));
    let mut callee_block = BasicBlock::new(BasicBlockId(0));
    callee_block.set_terminator(Terminator::Return { value: None });
    callee.push_block(callee_block);
    callee.set_params(vec!["$env".into(), "$this".into()]);
    program.push_function(callee);

    let mut function = Function::new("hot", BasicBlockId(0));
    function.set_params(vec!["$env".into(), "$this".into(), "f".into()]);
    let mut block = BasicBlock::new(BasicBlockId(0));
    block.push_instruction(Instruction::LoadVar {
        dest: ValueId(0),
        name: "f".into(),
    });
    block.push_instruction(Instruction::Call {
        dest: None,
        callee: ValueId(0),
        this_val: ValueId(0),
        args: vec![],
        callsite: None,
    });
    block.set_terminator(Terminator::Return { value: None });
    function.push_block(block);
    program.push_function(function);

    let mut facts = empty_facts(FunctionId(1));
    facts.calls.push(wjsm_optimize::CallFact {
        block: BasicBlockId(0),
        instruction_index: 1,
        target_function: 0,
        target_image_id: 0,
        this_tag: 0,
        this_shape: 0,
        construct: false,
        poly_len: 1,
        poly_functions: [0, 0, 0, 0],
    });
    let mut deopt_map = DeoptMap::default();
    let mut slot_map = SlotMap::default();
    rewrite_speculative(&mut program, &facts, &mut deopt_map, &mut slot_map);
    let rewritten = &program.functions()[1];
    let has_guard = rewritten.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::GuardCallTarget {
                    function: FunctionId(0),
                    ..
                }
            )
        })
    });
    assert!(has_guard);
    assert!(!deopt_map.points.is_empty());
}

#[test]
fn polymorphic_getprop_emits_shape_jumptable() {
    let mut program = Program::new();
    let key = program.add_constant(wjsm_ir::Constant::String("x".into()));
    let mut function = Function::new("hot", BasicBlockId(0));
    function.set_params(vec!["$env".into(), "$this".into(), "o".into()]);
    let mut block = BasicBlock::new(BasicBlockId(0));
    block.push_instruction(Instruction::LoadVar {
        dest: ValueId(0),
        name: "o".into(),
    });
    block.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: key,
    });
    block.push_instruction(Instruction::GetProp {
        dest: ValueId(2),
        object: ValueId(0),
        key: ValueId(1),
        latch: None,
        latch_template: None,
    });
    block.set_terminator(Terminator::Return {
        value: Some(ValueId(2)),
    });
    function.push_block(block);
    program.push_function(function);

    let mut facts = empty_facts(FunctionId(0));
    facts.get_props.push(PropFact {
        block: BasicBlockId(0),
        instruction_index: 2,
        shape_id: 7,
        slot_index: 0,
        proto_generation: 0,
        expected_proto: 0,
        own_data: true,
        poly_len: 2,
        poly_shapes: [7, 9, 0, 0],
        poly_slots: [0, 0, 0, 0],
        transition_shape: None,
    });
    let mut deopt_map = DeoptMap::default();
    let mut slot_map = SlotMap::default();
    rewrite_speculative(&mut program, &facts, &mut deopt_map, &mut slot_map);
    program.verify().expect("poly jumptable IR must verify");
    let guards: Vec<u32> = program.functions()[0]
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::GuardShape { shape_id, .. } => Some(*shape_id),
            _ => None,
        })
        .collect();
    assert!(guards.contains(&7));
    assert!(guards.contains(&9));
    assert!(
        program.functions()[0]
            .blocks()
            .iter()
            .any(|block| matches!(block.terminator(), Terminator::Deopt { .. }))
    );
}
