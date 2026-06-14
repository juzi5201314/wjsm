use wjsm_ir::value::{
    encode_bigint_handle, encode_bool, encode_bound_idx, encode_closure_idx, encode_exception,
    encode_f64, encode_function_idx, encode_handle, encode_native_callable_idx, encode_null,
    encode_object_handle, encode_proxy_handle, encode_regexp_handle, encode_runtime_string_handle,
    encode_scope_record_handle, encode_string_ptr, encode_symbol_handle, encode_undefined,
    tag_needs_root, TAG_ARRAY, TAG_ENUMERATOR, TAG_ITERATOR,
};
use wjsm_ir::value_ty::{infer_value_ty, ValueTy};
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, CompareOp, Constant, Function, Instruction, Module,
    Terminator, ValueId,
};
use wjsm_ir::liveness::compute_liveness;

#[test]
fn tag_needs_root_covers_all_handle_tags() {
    let handles: &[i64] = &[
        encode_object_handle(1),
        encode_handle(TAG_ARRAY, 2),
        encode_function_idx(3),
        encode_closure_idx(4),
        encode_bound_idx(5),
        encode_bigint_handle(6),
        encode_symbol_handle(7),
        encode_regexp_handle(8),
        encode_proxy_handle(9),
        encode_scope_record_handle(10),
        encode_native_callable_idx(11),
        encode_runtime_string_handle(12),
        encode_handle(TAG_ITERATOR, 13),
        encode_handle(TAG_ENUMERATOR, 14),
        encode_exception(15),
    ];

    for (i, val) in handles.iter().enumerate() {
        assert!(
            tag_needs_root(*val),
            "handle tag at index {i} (val={val:#018x}) should need rooting",
        );
    }
}

#[test]
fn tag_needs_root_rejects_scalars() {
    let scalars: &[i64] = &[
        encode_f64(3.14),
        encode_f64(0.0),
        encode_f64(-0.0),
        encode_undefined(),
        encode_null(),
        encode_bool(true),
        encode_bool(false),
        // Static string pointer (NOT a runtime handle): must NOT root.
        encode_string_ptr(0x1000),
    ];

    for (i, val) in scalars.iter().enumerate() {
        assert!(
            !tag_needs_root(*val),
            "scalar at index {i} (val={val:#018x}) should NOT need rooting",
        );
    }
}

// ── ValueTy type inference (T1.2) ────────────────────────────────────────

#[test]
fn value_ty_object_producing_ops_are_handle() {
    let mut module = Module::new();
    let mut f = Function::new("test", BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.push_instruction(Instruction::NewObject {
        dest: ValueId(0),
        capacity: 4,
    });
    bb.push_instruction(Instruction::NewArray {
        dest: ValueId(1),
        capacity: 4,
    });
    bb.push_instruction(Instruction::GetSuperBase { dest: ValueId(2) });
    bb.push_instruction(Instruction::GetSuperConstructor { dest: ValueId(3) });
    bb.push_instruction(Instruction::NewPromise { dest: ValueId(4) });
    bb.set_terminator(Terminator::Return {
        value: Some(ValueId(0)),
    });
    f.push_block(bb);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&ValueId(0)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(1)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(2)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(3)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(4)], ValueTy::Handle);
}

#[test]
fn value_ty_arithmetic_and_compare_are_scalar() {
    let mut module = Module::new();
    let c0 = module.add_constant(Constant::Number(1.0));
    let c1 = module.add_constant(Constant::Number(2.0));
    let mut f = Function::new("test", BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: c0,
    });
    bb.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: c1,
    });
    bb.push_instruction(Instruction::Binary {
        dest: ValueId(2),
        op: BinaryOp::Add,
        lhs: ValueId(0),
        rhs: ValueId(1),
    });
    bb.push_instruction(Instruction::Compare {
        dest: ValueId(3),
        op: CompareOp::StrictEq,
        lhs: ValueId(0),
        rhs: ValueId(1),
    });
    bb.set_terminator(Terminator::Return {
        value: Some(ValueId(2)),
    });
    f.push_block(bb);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&ValueId(2)], ValueTy::Scalar, "Binary arithmetic -> Scalar");
    assert_eq!(ty[&ValueId(3)], ValueTy::Scalar, "Compare -> Scalar");
}

#[test]
fn value_ty_const_scalar_constants_are_scalar() {
    let mut module = Module::new();
    let n = module.add_constant(Constant::Number(3.14));
    let b = module.add_constant(Constant::Bool(true));
    let null = module.add_constant(Constant::Null);
    let und = module.add_constant(Constant::Undefined);
    let mut f = Function::new("test", BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: n,
    });
    bb.push_instruction(Instruction::Const {
        dest: ValueId(1),
        constant: b,
    });
    bb.push_instruction(Instruction::Const {
        dest: ValueId(2),
        constant: null,
    });
    bb.push_instruction(Instruction::Const {
        dest: ValueId(3),
        constant: und,
    });
    bb.set_terminator(Terminator::Return {
        value: Some(ValueId(0)),
    });
    f.push_block(bb);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&ValueId(0)], ValueTy::Scalar, "Const Number -> Scalar");
    assert_eq!(ty[&ValueId(1)], ValueTy::Scalar, "Const Bool -> Scalar");
    assert_eq!(ty[&ValueId(2)], ValueTy::Scalar, "Const Null -> Scalar");
    assert_eq!(ty[&ValueId(3)], ValueTy::Scalar, "Const Undefined -> Scalar");
}

#[test]
fn value_ty_const_handle_constants_and_polymorphic_are_handle() {
    let mut module = Module::new();
    let s = module.add_constant(Constant::String("hi".to_string())); // String const -> Handle
    let mut f = Function::new("test", BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: s,
    });
    bb.push_instruction(Instruction::NewObject {
        dest: ValueId(1),
        capacity: 4,
    });
    // GetProp is polymorphic -> Handle (conservative)
    bb.push_instruction(Instruction::GetProp {
        dest: ValueId(2),
        object: ValueId(1),
        key: ValueId(0),
    });
    bb.set_terminator(Terminator::Return {
        value: Some(ValueId(2)),
    });
    f.push_block(bb);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&ValueId(0)], ValueTy::Handle, "Const String -> Handle");
    assert_eq!(ty[&ValueId(2)], ValueTy::Handle, "GetProp polymorphic -> Handle");
}

// ── Liveness analysis (T1.3-T1.4) ────────────────────────────────────────
// 契约：live[(block_id, i)] = 紧邻指令 i 执行*前*活跃的 ValueId 集合。
//       live[(block_id, len)] = 块出口（= live_out）活跃集。

fn bb(id: u32) -> BasicBlockId {
    BasicBlockId(id)
}
fn v(id: u32) -> ValueId {
    ValueId(id)
}

#[test]
fn liveness_linear_live_value_survives_to_return() {
    // bb0: v0 = NewObject(cap 4); v1 = Const(num); return v0
    // safepoint 在 NewObject 后：v0 此刻未 def（NewObject 才 def），所以查询点在
    // NewObject *之前* live 集应为空；NewObject 之后（idx=1，即 Const 前）live={v0}
    let mut module = Module::new();
    let num = module.add_constant(Constant::Number(1.0));
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::NewObject {
        dest: v(0),
        capacity: 4,
    });
    b0.push_instruction(Instruction::Const {
        dest: v(1),
        constant: num,
    });
    b0.set_terminator(Terminator::Return { value: Some(v(0)) });
    f.push_block(b0);
    module.push_function(f);

    let live = compute_liveness(module.functions().last().unwrap());
    // 紧邻 Const(idx=1) 执行前：v0 活跃（被 return 用）；v1 还没 def
    let at_const = live.get(&(bb(0), 1)).unwrap();
    assert!(at_const.contains(&v(0)), "v0 live before Const: {at_const:?}");
    assert!(!at_const.contains(&v(1)), "v1 not yet defined: {at_const:?}");
}

#[test]
fn liveness_dead_value_not_live() {
    // bb0: v0 = NewObject; v1 = NewObject; return v0
    // v1 从未被用 -> 不应在任何点 live（除 def 后到下一个 def 前，但因无 use，dead）
    let mut module = Module::new();
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::NewObject {
        dest: v(0),
        capacity: 4,
    });
    b0.push_instruction(Instruction::NewObject {
        dest: v(1),
        capacity: 4,
    });
    b0.set_terminator(Terminator::Return { value: Some(v(0)) });
    f.push_block(b0);
    module.push_function(f);

    let live = compute_liveness(module.functions().last().unwrap());
    // 块出口 live 应只有 v0（v1 死）
    let out = live.get(&(bb(0), 2)).unwrap();
    assert!(out.contains(&v(0)) && !out.contains(&v(1)), "only v0 live at exit: {out:?}");
}

#[test]
fn liveness_if_else_join_phi_edge_distribution() {
    // CFG:
    //   bb0: v0 = Const(0); Branch(v0) -> bb1 / bb2
    //   bb1: v1 = NewObject; Jump bb3
    //   bb2: v2 = NewObject; Jump bb3
    //   bb3: v3 = Phi[(bb1,v1),(bb2,v2)]; return v3
    //
    // Phi 边分发：v1 只对 bb1 的 live_out 有贡献，v2 只对 bb2 的 live_out。
    // 关键断言：bb1 出口 live 含 v1 但不含 v2；bb2 出口 live 含 v2 但不含 v1。
    let mut module = Module::new();
    let zero = module.add_constant(Constant::Number(0.0));
    let mut f = Function::new("test", bb(0));

    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: zero,
    });
    b0.set_terminator(Terminator::Branch {
        condition: v(0),
        true_block: bb(1),
        false_block: bb(2),
    });
    f.push_block(b0);

    let mut b1 = BasicBlock::new(bb(1));
    b1.push_instruction(Instruction::NewObject {
        dest: v(1),
        capacity: 4,
    });
    b1.set_terminator(Terminator::Jump { target: bb(3) });
    f.push_block(b1);

    let mut b2 = BasicBlock::new(bb(2));
    b2.push_instruction(Instruction::NewObject {
        dest: v(2),
        capacity: 4,
    });
    b2.set_terminator(Terminator::Jump { target: bb(3) });
    f.push_block(b2);

    let mut b3 = BasicBlock::new(bb(3));
    b3.push_instruction(Instruction::Phi {
        dest: v(3),
        sources: vec![
            wjsm_ir::PhiSource {
                predecessor: bb(1),
                value: v(1),
            },
            wjsm_ir::PhiSource {
                predecessor: bb(2),
                value: v(2),
            },
        ],
    });
    b3.set_terminator(Terminator::Return { value: Some(v(3)) });
    f.push_block(b3);
    module.push_function(f);

    let live = compute_liveness(module.functions().last().unwrap());
    let b1_out = live.get(&(bb(1), 1)).unwrap(); // bb1 出口（1 条指令后）
    let b2_out = live.get(&(bb(2), 1)).unwrap();
    assert!(
        b1_out.contains(&v(1)) && !b1_out.contains(&v(2)),
        "bb1 out: v1 live, v2 NOT: {b1_out:?}"
    );
    assert!(
        b2_out.contains(&v(2)) && !b2_out.contains(&v(1)),
        "bb2 out: v2 live, v1 NOT: {b2_out:?}"
    );
}

#[test]
fn liveness_loop_backedge_phi() {
    // CFG:
    //   bb0: v0 = Const(0); Jump bb1
    //   bb1: v1 = Phi[(bb0,v0),(bb1,v2)]; v3 = Const(10);
    //        Branch(v3) -> bb2 / bb3     (条件值不影响 liveness，用 const 简化)
    //   bb2: v2 = Const(1); Jump bb1
    //   bb3: return v1
    //
    // 关键：bb1 的 Phi 在 backedge 上消费 v2（来自 bb2），所以 bb2 出口 live 含 v2。
    //       bb0 出口 live 含 v0（Phi 源来自 bb0）。
    let mut module = Module::new();
    let c0 = module.add_constant(Constant::Number(0.0));
    let c1 = module.add_constant(Constant::Number(1.0));
    let c10 = module.add_constant(Constant::Number(10.0));
    let mut f = Function::new("test", bb(0));

    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: c0,
    });
    b0.set_terminator(Terminator::Jump { target: bb(1) });
    f.push_block(b0);

    let mut b1 = BasicBlock::new(bb(1));
    b1.push_instruction(Instruction::Phi {
        dest: v(1),
        sources: vec![
            wjsm_ir::PhiSource {
                predecessor: bb(0),
                value: v(0),
            },
            wjsm_ir::PhiSource {
                predecessor: bb(2),
                value: v(2),
            },
        ],
    });
    b1.push_instruction(Instruction::Const {
        dest: v(3),
        constant: c10,
    });
    b1.set_terminator(Terminator::Branch {
        condition: v(3),
        true_block: bb(2),
        false_block: bb(3),
    });
    f.push_block(b1);

    let mut b2 = BasicBlock::new(bb(2));
    b2.push_instruction(Instruction::Const {
        dest: v(2),
        constant: c1,
    });
    b2.set_terminator(Terminator::Jump { target: bb(1) });
    f.push_block(b2);

    let mut b3 = BasicBlock::new(bb(3));
    b3.set_terminator(Terminator::Return { value: Some(v(1)) });
    f.push_block(b3);
    module.push_function(f);

    let live = compute_liveness(module.functions().last().unwrap());
    let b0_out = live.get(&(bb(0), 1)).unwrap();
    let b2_out = live.get(&(bb(2), 1)).unwrap();
    assert!(
        b0_out.contains(&v(0)),
        "bb0 out: v0 live (Phi src from bb0): {b0_out:?}"
    );
    assert!(
        b2_out.contains(&v(2)),
        "bb2 out: v2 live (Phi src on backedge from bb2): {b2_out:?}"
    );
}
