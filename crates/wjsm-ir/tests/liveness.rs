use wjsm_ir::liveness::compute_liveness;
use wjsm_ir::value::{
    TAG_ARRAY, TAG_ENUMERATOR, TAG_ITERATOR, encode_bigint_handle, encode_bool, encode_bound_idx,
    encode_closure_idx, encode_exception, encode_f64, encode_function_idx, encode_handle,
    encode_native_callable_idx, encode_null, encode_object_handle, encode_proxy_handle,
    encode_regexp_handle, encode_runtime_string_handle, encode_scope_record_handle,
    encode_string_ptr, encode_symbol_handle, encode_undefined, tag_needs_root,
};
use wjsm_ir::value_ty::{ValueTy, infer_value_ty};
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, CompareOp, Constant, Function, Instruction, Module,
    Terminator, ValueId,
};

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
        encode_f64(3.15),
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
    assert_eq!(
        ty[&ValueId(2)],
        ValueTy::Scalar,
        "Binary arithmetic -> Scalar"
    );
    assert_eq!(ty[&ValueId(3)], ValueTy::Scalar, "Compare -> Scalar");
}

#[test]
fn value_ty_const_scalar_constants_are_scalar() {
    let mut module = Module::new();
    let n = module.add_constant(Constant::Number(3.15));
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
    assert_eq!(
        ty[&ValueId(3)],
        ValueTy::Scalar,
        "Const Undefined -> Scalar"
    );
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
    assert_eq!(
        ty[&ValueId(2)],
        ValueTy::Handle,
        "GetProp polymorphic -> Handle"
    );
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
    assert!(
        at_const.contains(&v(0)),
        "v0 live before Const: {at_const:?}"
    );
    assert!(
        !at_const.contains(&v(1)),
        "v1 not yet defined: {at_const:?}"
    );
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
    assert!(
        out.contains(&v(0)) && !out.contains(&v(1)),
        "only v0 live at exit: {out:?}"
    );
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

// ── Fixed-point propagation (Layer 1) ───────────────────────────────────
// 这些测试验证 value_ty.rs 的固定点迭代：StoreVar→LoadVar 传播、Phi 折叠、
// builtin 白名单、以及未被 StoreVar 的变量保守不降级。

#[test]
fn value_ty_storevar_to_loadvar_scalar_propagation() {
    // 级联污染核心场景：
    //   v0 = Const(Number)            // Scalar
    //   v1 = Binary(Add, v0, v0)      // Scalar
    //   StoreVar("$0.x", v1)          // 源 v1 是 Scalar
    //   v2 = LoadVar("$0.x")          // 应降为 Scalar（所有 StoreVar 源都是 Scalar）
    //   return v2
    // 若无固定点迭代，v2 会被判 Handle（漏降级，过度 spill）。
    let mut module = Module::new();
    let n = module.add_constant(Constant::Number(1.0));
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: n,
    });
    b0.push_instruction(Instruction::Binary {
        dest: v(1),
        op: BinaryOp::Add,
        lhs: v(0),
        rhs: v(0),
    });
    b0.push_instruction(Instruction::StoreVar {
        name: "$0.x".to_string(),
        value: v(1),
    });
    b0.push_instruction(Instruction::LoadVar {
        dest: v(2),
        name: "$0.x".to_string(),
    });
    b0.set_terminator(Terminator::Return { value: Some(v(2)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&v(0)], ValueTy::Scalar, "Const Number -> Scalar");
    assert_eq!(ty[&v(1)], ValueTy::Scalar, "Binary Add -> Scalar");
    assert_eq!(
        ty[&v(2)],
        ValueTy::Scalar,
        "LoadVar of all-Scalar StoreVar sources -> Scalar (fixed-point propagation)"
    );
}

#[test]
fn value_ty_loadvar_of_handle_store_stays_handle() {
    // LoadVar 的源是 Handle（NewObject），不应降级。
    //   v0 = NewObject
    //   StoreVar("$0.x", v0)    // 源 Handle
    //   v1 = LoadVar("$0.x")    // 必须保持 Handle
    //   return v1
    let mut module = Module::new();
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::NewObject {
        dest: v(0),
        capacity: 4,
    });
    b0.push_instruction(Instruction::StoreVar {
        name: "$0.x".to_string(),
        value: v(0),
    });
    b0.push_instruction(Instruction::LoadVar {
        dest: v(1),
        name: "$0.x".to_string(),
    });
    b0.set_terminator(Terminator::Return { value: Some(v(1)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&v(0)], ValueTy::Handle);
    assert_eq!(
        ty[&v(1)],
        ValueTy::Handle,
        "LoadVar of Handle StoreVar source must stay Handle"
    );
}

#[test]
fn value_ty_unstored_loadvar_stays_handle() {
    // 未被 StoreVar 的变量（函数参数 / 捕获变量）必须保守 Handle。
    //   v0 = LoadVar("$0.param")   // 从未被 StoreVar -> 不降级
    //   return v0
    let mut module = Module::new();
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::LoadVar {
        dest: v(0),
        name: "$0.param".to_string(),
    });
    b0.set_terminator(Terminator::Return { value: Some(v(0)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(
        ty[&v(0)],
        ValueTy::Handle,
        "LoadVar of never-stored variable (param/capture) must stay Handle"
    );
}

#[test]
fn value_ty_phi_all_scalar_sources_folds_to_scalar() {
    // Phi 折叠：两路来源都是 Scalar -> Phi dest Scalar。
    //   bb0: v0 = Const(0); Branch(v0) -> bb1 / bb2
    //   bb1: v1 = Const(1); Jump bb3
    //   bb2: v2 = Const(2); Jump bb3
    //   bb3: v3 = Phi[(bb1,v1),(bb2,v2)]; return v3
    // v1/v2 都是 Const Number(Scalar)，故 v3 应为 Scalar。
    let mut module = Module::new();
    let c0 = module.add_constant(Constant::Number(0.0));
    let c1 = module.add_constant(Constant::Number(1.0));
    let c2 = module.add_constant(Constant::Number(2.0));
    let mut f = Function::new("test", bb(0));

    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: c0,
    });
    b0.set_terminator(Terminator::Branch {
        condition: v(0),
        true_block: bb(1),
        false_block: bb(2),
    });
    f.push_block(b0);

    let mut b1 = BasicBlock::new(bb(1));
    b1.push_instruction(Instruction::Const {
        dest: v(1),
        constant: c1,
    });
    b1.set_terminator(Terminator::Jump { target: bb(3) });
    f.push_block(b1);

    let mut b2 = BasicBlock::new(bb(2));
    b2.push_instruction(Instruction::Const {
        dest: v(2),
        constant: c2,
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

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(
        ty[&v(3)],
        ValueTy::Scalar,
        "Phi with all-Scalar sources -> Scalar (folding)"
    );
}

#[test]
fn value_ty_phi_one_handle_source_stays_handle() {
    // Phi 任一来源 Handle -> 保持 Handle。
    //   bb1: v1 = NewObject (Handle)
    //   bb2: v2 = Const(1)  (Scalar)
    //   bb3: v3 = Phi[(bb1,v1),(bb2,v2)] -> Handle
    let mut module = Module::new();
    let c0 = module.add_constant(Constant::Number(0.0));
    let c1 = module.add_constant(Constant::Number(1.0));
    let mut f = Function::new("test", bb(0));

    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: c0,
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
    b2.push_instruction(Instruction::Const {
        dest: v(2),
        constant: c1,
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

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(
        ty[&v(3)],
        ValueTy::Handle,
        "Phi with any Handle source -> Handle (conservative)"
    );
}

#[test]
fn value_ty_scalar_builtin_whitelist_and_handle_default() {
    // 白名单 builtin（如 ArrayIsArray）-> Scalar；非白名单 -> Handle。
    //   v0 = Const(Number)  // arg
    //   v1 = CallBuiltin(ArrayIsArray, [v0])      // Scalar
    //   v2 = CallBuiltin(ArrayFrom, ...)          // non-whitelist -> Handle
    use wjsm_ir::Builtin;
    let mut module = Module::new();
    let n = module.add_constant(Constant::Number(1.0));
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: n,
    });
    b0.push_instruction(Instruction::CallBuiltin {
        dest: Some(v(1)),
        builtin: Builtin::ArrayIsArray,
        args: vec![v(0)],
    });
    b0.push_instruction(Instruction::CallBuiltin {
        dest: Some(v(2)),
        builtin: Builtin::ArrayFrom,
        args: vec![v(0)],
    });
    b0.set_terminator(Terminator::Return { value: Some(v(1)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(
        ty[&v(1)],
        ValueTy::Scalar,
        "ArrayIsArray is on scalar whitelist"
    );
    assert_eq!(
        ty[&v(2)],
        ValueTy::Handle,
        "ArrayFrom (allocates) is NOT on whitelist -> Handle"
    );
}

#[test]
fn value_ty_deleteprop_and_isexception_are_scalar() {
    // 两个曾误判 Handle 的 bug fix 验证。
    //   v0 = NewObject; v1 = Const(num key)
    //   v2 = DeleteProp(v0, v1)    // bool -> Scalar
    //   v3 = IsException(v2)       // bool -> Scalar
    let mut module = Module::new();
    let k = module.add_constant(Constant::Number(0.0));
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::NewObject {
        dest: v(0),
        capacity: 4,
    });
    b0.push_instruction(Instruction::Const {
        dest: v(1),
        constant: k,
    });
    b0.push_instruction(Instruction::DeleteProp {
        dest: v(2),
        object: v(0),
        key: v(1),
    });
    b0.push_instruction(Instruction::IsException {
        dest: v(3),
        value: v(2),
    });
    b0.set_terminator(Terminator::Return { value: Some(v(2)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&v(2)], ValueTy::Scalar, "DeleteProp -> bool -> Scalar");
    assert_eq!(ty[&v(3)], ValueTy::Scalar, "IsException -> bool -> Scalar");
}

#[test]
fn value_ty_encode_exception_stays_handle() {
    // EncodeException 必须保持 Handle（TAG_EXCEPTION needs_root=true，
    // low32 携带真实对象 handle）。修正 report.md 的风险判断。
    //   v0 = NewObject
    //   v1 = EncodeException(v0)  // Handle
    let mut module = Module::new();
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::NewObject {
        dest: v(0),
        capacity: 4,
    });
    b0.push_instruction(Instruction::EncodeException {
        dest: v(1),
        value: v(0),
    });
    b0.set_terminator(Terminator::Return { value: Some(v(1)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(
        ty[&v(1)],
        ValueTy::Handle,
        "EncodeException must stay Handle (TAG_EXCEPTION needs root)"
    );
}

#[test]
fn value_ty_fixed_point_chains_through_phi_and_loadvar() {
    // 链式传播收敛性测试（验证不动点迭代至少需要 2 轮）：
    //   v0 = Const(Number)            // Scalar           (round 0)
    //   StoreVar("$0.x", v0)
    //   v1 = LoadVar("$0.x")          // -> Scalar round 1
    //   v2 = Phi[(bb0,v1)]            // -> Scalar round 2 (consumes v1)
    //   return v2
    // 单遍分析会把 v1 和 v2 都判 Handle；需要 2 轮迭代才收敛。
    let mut module = Module::new();
    let n = module.add_constant(Constant::Number(42.0));
    let mut f = Function::new("test", bb(0));
    let mut b0 = BasicBlock::new(bb(0));
    b0.push_instruction(Instruction::Const {
        dest: v(0),
        constant: n,
    });
    b0.push_instruction(Instruction::StoreVar {
        name: "$0.x".to_string(),
        value: v(0),
    });
    b0.push_instruction(Instruction::LoadVar {
        dest: v(1),
        name: "$0.x".to_string(),
    });
    b0.push_instruction(Instruction::Phi {
        dest: v(2),
        sources: vec![wjsm_ir::PhiSource {
            predecessor: bb(0),
            value: v(1),
        }],
    });
    b0.set_terminator(Terminator::Return { value: Some(v(2)) });
    f.push_block(b0);
    module.push_function(f);

    let ty = infer_value_ty(&module, module.functions().last().unwrap());
    assert_eq!(ty[&v(1)], ValueTy::Scalar, "LoadVar propagated round 1");
    assert_eq!(
        ty[&v(2)],
        ValueTy::Scalar,
        "Phi consuming propagated LoadVar folds round 2 (chain converges)"
    );
}
