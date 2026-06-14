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
