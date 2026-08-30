//! phi / frame local / 布尔推断

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use wjsm_ir::{Instruction, ValueId, constants, value};
use wjsm_native_abi::NativeRuntimeOp;

pub(crate) fn switch_constant_immediate(constant: &Constant) -> Result<i64> {
    match constant {
        Constant::Number(number) => Ok(value::encode_f64(*number)),
        Constant::Bool(boolean) => Ok(value::encode_bool(*boolean)),
        Constant::Null => Ok(value::encode_null()),
        Constant::Undefined => Ok(value::encode_undefined()),
        Constant::FunctionRef(_) => bail!("function references are not valid switch keys"),
        Constant::NativeCallableEval => Ok(value::encode_native_callable_idx(0)),
        Constant::ModuleId(module) => Ok(value::encode_f64(f64::from(module.0))),
        Constant::String(_)
        | Constant::Utf16String(_)
        | Constant::BigInt(_)
        | Constant::RegExp { .. } => {
            bail!("materialized constants are not valid switch keys")
        }
        Constant::ArrayTemplate(_) => bail!("array templates are not valid switch keys"),
        Constant::ObjectTemplate { .. } => {
            bail!("object templates are not valid switch keys")
        }
        Constant::Uninitialized => bail!("uninitialized sentinel is not a valid switch key"),
    }
}

/// φ 边上的并行赋值：先按各自 dest 的表示读出全部 source，再统一写入。
///
/// 逐对选择表示，两端都是 typed 时不产出转换指令；循环回边因此不会把归纳变量
/// 打标再拆包。
pub(crate) fn define_phi_edge(
    builder: &mut FunctionBuilder<'_>,
    variables: &ValueRepr,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
    predecessor: BasicBlockId,
    target: BasicBlockId,
) -> Result<()> {
    if let Some(assignments) = phi_edges.get(&(predecessor, target)) {
        let values: Vec<_> = assignments
            .iter()
            .map(|(dest, source)| {
                use_value_as(builder, variables, variables.is_typed_value(*dest), *source)
            })
            .collect::<Result<_>>()?;
        for ((dest, _), value) in assignments.iter().zip(values) {
            define_value_as(builder, variables, *dest, value)?;
        }
    }
    Ok(())
}

pub(crate) fn collect_phi_edges(
    function: &wjsm_ir::Function,
) -> HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>> {
    let mut edges = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                for source in sources {
                    edges
                        .entry((source.predecessor, block.id()))
                        .or_insert_with(Vec::new)
                        .push((*dest, source.value));
                }
            }
        }
    }
    edges
}

pub(crate) fn infer_boolean_values(
    function: &wjsm_ir::Function,
    constants: &[Constant],
) -> HashSet<ValueId> {
    let mut booleans = HashSet::new();
    loop {
        let before = booleans.len();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let destination = match instruction {
                    Instruction::Const { dest, constant }
                        if matches!(
                            constants.get(
                                usize::try_from(constant.0).expect("constant index fits usize"),
                            ),
                            Some(Constant::Bool(_))
                        ) =>
                    {
                        Some(*dest)
                    }
                    Instruction::Compare { dest, .. }
                    | Instruction::IsException { dest, .. }
                    | Instruction::GuardSameFunction { dest, .. }
                    | Instruction::GuardTag { dest, .. }
                    | Instruction::GuardShape { dest, .. }
                    | Instruction::GuardElementsKind { dest, .. }
                    | Instruction::GuardCallTarget { dest, .. } => Some(*dest),
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not | UnaryOp::IsNullish,
                        ..
                    } => Some(*dest),
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin:
                            Builtin::AbstractCompare
                            | Builtin::AbstractEq
                            | Builtin::StrictEq
                            | Builtin::ToBoolean
                            | Builtin::IsCallable
                            | Builtin::IsJsObject
                            | Builtin::ArrayHasElement
                            | Builtin::ArrayIsPlain
                            | Builtin::ArraySpeciesDefault
                            | Builtin::ObjectIs,
                        ..
                    } => Some(*dest),
                    Instruction::Phi { dest, sources }
                        if !sources.is_empty()
                            && sources
                                .iter()
                                .all(|source| booleans.contains(&source.value)) =>
                    {
                        Some(*dest)
                    }
                    _ => None,
                };
                if let Some(destination) = destination {
                    booleans.insert(destination);
                }
            }
        }
        if booleans.len() == before {
            return booleans;
        }
    }
}

/// GC 不必扫描的 SSA：已证明 f64（常驻 xmm）与标记立即数布尔（不是堆句柄）。
///
/// Compare/守卫的 dest 是 tagged bool，live 跨过 Branch/回边时若仍记进 root
/// frame，纯数值循环也会 `atomic_rmw` 发布根。立即数不是堆指针，排除后
/// 这类函数可以走 safepoint-free。
pub(crate) fn non_heap_ssa_values(
    function: &wjsm_ir::Function,
    constants: &[Constant],
    f64_values: &HashSet<ValueId>,
) -> HashSet<ValueId> {
    let mut values = f64_values.clone();
    values.extend(infer_boolean_values(function, constants));
    values
}

pub(crate) fn collect_value_ids(function: &wjsm_ir::Function) -> HashSet<ValueId> {
    let mut ids = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            collect_instruction_values(instruction, &mut ids);
        }
        match block.terminator() {
            Terminator::Return { value: Some(value) } | Terminator::Throw { value } => {
                ids.insert(*value);
            }
            Terminator::Deopt { frames } => {
                for frame in frames {
                    ids.extend(frame.lives.iter().copied());
                }
            }
            Terminator::Branch { condition, .. }
            | Terminator::Switch {
                value: condition, ..
            } => {
                ids.insert(*condition);
            }
            Terminator::Return { value: None }
            | Terminator::Jump { .. }
            | Terminator::Unreachable => {}
        }
    }
    ids
}

pub(crate) fn collect_instruction_values(instruction: &Instruction, ids: &mut HashSet<ValueId>) {
    let mut instruction = instruction.clone();
    instruction.remap_values(&mut |value| {
        ids.insert(value);
        value
    });
}

pub(crate) fn frame_local_variables(names: &BTreeSet<&str>) -> HashMap<String, Variable> {
    names
        .iter()
        .map(|name| ((*name).to_owned(), Variable::from_u32(0)))
        .collect()
}

pub(crate) fn initialize_frame_locals(
    builder: &mut FunctionBuilder<'_>,
    locals: &mut HashMap<String, Variable>,
    repr: &ValueRepr,
) {
    for (name, variable) in locals.iter_mut() {
        *variable = builder.declare_var(repr.local_type(name));
        // typed 局部的资格保证入口定义到不了任何 load（见 `ValueRepr::plan`），
        // 这里的 0.0 只是给 Cranelift 的 SSA 构造一个确定的支配定义。
        let initial = if repr.is_typed_local(name) {
            builder.ins().f64const(0.0)
        } else {
            builder.ins().iconst(types::I64, value::encode_undefined())
        };
        builder.def_var(*variable, initial);
    }
}

pub(crate) fn boxed_local_order(names: &BTreeSet<&str>) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

pub(crate) fn frame_local_indices(order: &[String]) -> HashMap<String, usize> {
    order
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect()
}

pub(crate) fn pin_initialized_frame_locals(
    root_frame: &mut FrameLowering,
    builder: &mut FunctionBuilder<'_>,
    locals: &HashMap<String, Variable>,
    order: &[String],
) -> Result<()> {
    let values: Vec<ir::Value> = order
        .iter()
        .map(|name| {
            let variable = locals
                .get(name)
                .copied()
                .with_context(|| format!("frame-local variable {name} is missing"))?;
            Ok(builder.use_var(variable))
        })
        .collect::<Result<_>>()?;
    root_frame.pin_frame_locals(builder, &values)
}

pub(crate) fn boxed_frame_local_names<'a>(
    function: &'a wjsm_ir::Function,
    frame_locals: &BTreeSet<&'a str>,
    inferred_f64: &HashMap<FunctionId, HashSet<ValueId>>,
    index: usize,
) -> BTreeSet<&'a str> {
    let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
    let Some(f64_values) = inferred_f64.get(&function_id) else {
        return frame_locals.clone();
    };
    let mut f64_locals = BTreeSet::new();
    let mut mixed_locals = BTreeSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction
                && frame_locals.contains(name.as_str())
            {
                if f64_values.contains(value) {
                    if !mixed_locals.contains(name.as_str()) {
                        f64_locals.insert(name.as_str());
                    }
                } else {
                    f64_locals.remove(name.as_str());
                    mixed_locals.insert(name.as_str());
                }
            }
        }
    }
    frame_locals
        .iter()
        .copied()
        .filter(|name| !f64_locals.contains(name))
        .collect()
}

pub(crate) fn emit_number_or_proven_f64(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
    id: ValueId,
    f64_values: &HashSet<ValueId>,
) -> ir::Value {
    if f64_values.contains(&id) {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().icmp(ir::condcodes::IntCC::Equal, zero, zero)
    } else {
        emit_is_number(builder, encoded)
    }
}

pub(crate) fn binary_tag(op: BinaryOp) -> u16 {
    match op {
        BinaryOp::Add => 0,
        BinaryOp::Sub => 1,
        BinaryOp::Mul => 2,
        BinaryOp::Div => 3,
        BinaryOp::Mod => 4,
        BinaryOp::Exp => 5,
        BinaryOp::BitAnd => 6,
        BinaryOp::BitOr => 7,
        BinaryOp::BitXor => 8,
        BinaryOp::Shl => 9,
        BinaryOp::Shr => 10,
        BinaryOp::UShr => 11,
    }
}

pub(crate) fn unary_tag(op: UnaryOp) -> u16 {
    match op {
        UnaryOp::Not => 0,
        UnaryOp::Neg => 1,
        UnaryOp::Pos => 2,
        UnaryOp::BitNot => 3,
        UnaryOp::Void => 4,
        UnaryOp::IsNullish => 5,
        UnaryOp::Delete => 6,
    }
}

pub(crate) fn compare_tag(op: CompareOp) -> u16 {
    match op {
        CompareOp::StrictEq => 0,
        CompareOp::StrictNotEq => 1,
        CompareOp::Lt => 2,
        CompareOp::Gt => 3,
        CompareOp::LtEq => 4,
        CompareOp::GtEq => 5,
    }
}

pub(crate) fn libcall_name(libcall: ir::LibCall) -> String {
    use ir::LibCall;
    match libcall {
        LibCall::Memcpy => "wjsm_native_memory_copy".into(),
        LibCall::Memset => "wjsm_native_memory_fill".into(),
        LibCall::Memmove => "wjsm_native_memory_move".into(),
        LibCall::Memcmp => "wjsm_native_memory_compare".into(),
        forbidden => format!("__wjsm_forbidden_libcall_{forbidden:?}"),
    }
}
