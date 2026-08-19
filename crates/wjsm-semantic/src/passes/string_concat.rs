//! 字符串加法链融合：把已证明只含原始值的字符串 `+` 子树收敛为一次
//! `StringConcatVa`。
//!
//! 融合仅作用于函数帧局部变量，并要求每个叶子都有精确原始类型。这样把
//! `ToPrimitive` 延迟到变长拼接调用不会重排用户可见副作用；对象、Symbol、
//! BigInt/Number 混合以及类型不稳定的局部变量都会保留原始逐步 `+`。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BinaryOp, Builtin, Constant, Function, FunctionId, Instruction, Module, UnaryOp, ValueId,
};

use super::direct_call::{instr_uses, instruction_dest, terminator_uses};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimitiveKind {
    BigInt,
    Bool,
    Null,
    Number,
    String,
    Undefined,
}

pub(crate) fn run(module: &mut Module) {
    let constants = module.constants().to_vec();
    let locals_by_function: Vec<HashSet<String>> = module
        .frame_local_variable_names_by_function()
        .into_iter()
        .map(|names| names.into_iter().map(str::to_owned).collect())
        .collect();

    for (index, local_names) in locals_by_function.into_iter().enumerate() {
        let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
        let Some(function) = module.functions().get(index) else {
            continue;
        };
        let (kinds, local_kinds) = infer_stable_kinds(function, &constants, &local_names);
        let Some(function) = module.function_mut(function_id) else {
            continue;
        };
        fuse_function(function, &kinds);
        lower_builders(function, &kinds, &local_kinds);
    }
}

fn infer_stable_kinds(
    function: &Function,
    constants: &[Constant],
    local_names: &HashSet<String>,
) -> (
    HashMap<ValueId, PrimitiveKind>,
    HashMap<String, PrimitiveKind>,
) {
    let definitions = collect_definitions(function);
    let stores = collect_local_stores(function, local_names);
    let mut candidates = seed_local_kinds(&stores, &definitions, constants);

    loop {
        let kinds = infer_value_kinds(function, constants, &candidates);
        let before = candidates.len();
        candidates.retain(|name, expected| {
            stores.get(name).is_some_and(|values| {
                values
                    .iter()
                    .all(|value| kinds.get(value) == Some(expected))
            })
        });
        if candidates.len() == before {
            return (kinds, candidates);
        }
    }
}

fn collect_definitions(function: &Function) -> HashMap<ValueId, &Instruction> {
    let mut definitions = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                definitions.insert(dest, instruction);
            }
        }
    }
    definitions
}

fn collect_local_stores(
    function: &Function,
    local_names: &HashSet<String>,
) -> HashMap<String, Vec<ValueId>> {
    let mut stores: HashMap<String, Vec<ValueId>> = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction
                && local_names.contains(name)
            {
                stores.entry(name.clone()).or_default().push(*value);
            }
        }
    }
    stores
}

fn seed_local_kinds(
    stores: &HashMap<String, Vec<ValueId>>,
    definitions: &HashMap<ValueId, &Instruction>,
    constants: &[Constant],
) -> HashMap<String, PrimitiveKind> {
    let mut candidates = HashMap::new();
    for (name, values) in stores {
        let mut seed = None;
        let mut conflict = false;
        for value in values {
            let Some(Instruction::Const { constant, .. }) = definitions.get(value) else {
                continue;
            };
            let Some(kind) = constants
                .get(usize::try_from(constant.0).expect("constant index fits usize"))
                .and_then(constant_kind)
            else {
                continue;
            };
            match seed {
                Some(current) if current != kind => {
                    conflict = true;
                    break;
                }
                Some(_) => {}
                None => seed = Some(kind),
            }
        }
        if !conflict && let Some(kind) = seed {
            candidates.insert(name.clone(), kind);
        }
    }
    candidates
}

fn infer_value_kinds(
    function: &Function,
    constants: &[Constant],
    local_kinds: &HashMap<String, PrimitiveKind>,
) -> HashMap<ValueId, PrimitiveKind> {
    let instruction_count: usize = function
        .blocks()
        .iter()
        .map(|block| block.instructions().len())
        .sum();
    let mut kinds = HashMap::new();

    for _ in 0..=instruction_count {
        let mut changed = false;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Some((dest, kind)) =
                    infer_instruction_kind(instruction, constants, local_kinds, &kinds)
                else {
                    continue;
                };
                if kinds.insert(dest, kind) != Some(kind) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    kinds
}

fn infer_instruction_kind(
    instruction: &Instruction,
    constants: &[Constant],
    local_kinds: &HashMap<String, PrimitiveKind>,
    value_kinds: &HashMap<ValueId, PrimitiveKind>,
) -> Option<(ValueId, PrimitiveKind)> {
    match instruction {
        Instruction::Const { dest, constant } => constants
            .get(usize::try_from(constant.0).ok()?)
            .and_then(constant_kind)
            .map(|kind| (*dest, kind)),
        Instruction::LoadVar { dest, name } => {
            local_kinds.get(name).copied().map(|kind| (*dest, kind))
        }
        Instruction::Binary { dest, op, lhs, rhs } => {
            binary_kind(*op, *value_kinds.get(lhs)?, *value_kinds.get(rhs)?)
                .map(|kind| (*dest, kind))
        }
        Instruction::Unary { dest, op, value } => {
            unary_kind(*op, *value_kinds.get(value)?).map(|kind| (*dest, kind))
        }
        Instruction::Phi { dest, sources } => {
            let first = *value_kinds.get(&sources.first()?.value)?;
            sources
                .iter()
                .all(|source| value_kinds.get(&source.value) == Some(&first))
                .then_some((*dest, first))
        }
        Instruction::StringConcatVa { dest, parts } => parts
            .iter()
            .all(|part| value_kinds.contains_key(part))
            .then_some((*dest, PrimitiveKind::String)),
        _ => None,
    }
}

fn constant_kind(constant: &Constant) -> Option<PrimitiveKind> {
    Some(match constant {
        Constant::BigInt(_) => PrimitiveKind::BigInt,
        Constant::Bool(_) => PrimitiveKind::Bool,
        Constant::Null => PrimitiveKind::Null,
        Constant::Number(_) => PrimitiveKind::Number,
        Constant::String(_) => PrimitiveKind::String,
        Constant::Undefined => PrimitiveKind::Undefined,
        Constant::FunctionRef(_)
        | Constant::NativeCallableEval
        | Constant::RegExp { .. }
        | Constant::ModuleId(_) => return None,
    })
}

fn binary_kind(
    operation: BinaryOp,
    lhs: PrimitiveKind,
    rhs: PrimitiveKind,
) -> Option<PrimitiveKind> {
    if operation == BinaryOp::Add {
        if lhs == PrimitiveKind::String || rhs == PrimitiveKind::String {
            return Some(PrimitiveKind::String);
        }
        if lhs == PrimitiveKind::BigInt || rhs == PrimitiveKind::BigInt {
            return (lhs == PrimitiveKind::BigInt && rhs == PrimitiveKind::BigInt)
                .then_some(PrimitiveKind::BigInt);
        }
        return Some(PrimitiveKind::Number);
    }
    (lhs == PrimitiveKind::Number && rhs == PrimitiveKind::Number).then_some(PrimitiveKind::Number)
}

fn unary_kind(operation: UnaryOp, value: PrimitiveKind) -> Option<PrimitiveKind> {
    match operation {
        UnaryOp::Neg | UnaryOp::Pos if value == PrimitiveKind::Number => {
            Some(PrimitiveKind::Number)
        }
        UnaryOp::Not | UnaryOp::IsNullish => Some(PrimitiveKind::Bool),
        UnaryOp::Void => Some(PrimitiveKind::Undefined),
        UnaryOp::BitNot | UnaryOp::Delete | UnaryOp::Neg | UnaryOp::Pos => None,
    }
}

fn fuse_function(function: &mut Function, kinds: &HashMap<ValueId, PrimitiveKind>) {
    let use_counts = collect_use_counts(function);
    for block in function.blocks_mut() {
        let instructions = block.instructions();
        let definitions: HashMap<ValueId, usize> = instructions
            .iter()
            .enumerate()
            .filter_map(|(index, instruction)| {
                instruction_dest(instruction).map(|dest| (dest, index))
            })
            .collect();
        let mut removed = HashSet::new();
        let mut replacements = HashMap::new();

        for (root_index, instruction) in instructions.iter().enumerate() {
            let Instruction::Binary {
                dest,
                op: BinaryOp::Add,
                lhs,
                rhs,
            } = instruction
            else {
                continue;
            };
            if kinds.get(dest) != Some(&PrimitiveKind::String) {
                continue;
            }
            let mut parts = Vec::new();
            let mut nested = HashSet::new();
            if collect_concat_parts(
                *lhs,
                root_index,
                instructions,
                &definitions,
                &use_counts,
                kinds,
                &mut parts,
                &mut nested,
            ) && collect_concat_parts(
                *rhs,
                root_index,
                instructions,
                &definitions,
                &use_counts,
                kinds,
                &mut parts,
                &mut nested,
            ) && !nested.is_empty()
            {
                removed.extend(nested);
                replacements.insert(
                    root_index,
                    Instruction::StringConcatVa { dest: *dest, parts },
                );
            }
        }

        if removed.is_empty() {
            continue;
        }
        let original = std::mem::take(block.instructions_mut());
        let rewritten = original
            .into_iter()
            .enumerate()
            .filter_map(|(index, instruction)| {
                if removed.contains(&index) {
                    None
                } else {
                    Some(replacements.remove(&index).unwrap_or(instruction))
                }
            })
            .collect();
        *block.instructions_mut() = rewritten;
    }
}

fn lower_builders(
    function: &mut Function,
    kinds: &HashMap<ValueId, PrimitiveKind>,
    local_kinds: &HashMap<String, PrimitiveKind>,
) {
    let definitions = collect_definitions(function);
    let use_counts = collect_use_counts(function);
    let mut append_by_dest = HashMap::new();
    let mut append_loads = HashSet::new();

    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::StringConcatVa { dest, parts } = instruction else {
                continue;
            };
            let Some(first) = parts.first() else {
                continue;
            };
            let Some(Instruction::LoadVar { name, .. }) = definitions.get(first) else {
                continue;
            };
            if local_kinds.get(name) != Some(&PrimitiveKind::String)
                || kinds.get(dest) != Some(&PrimitiveKind::String)
                || parts.iter().any(|part| !kinds.contains_key(part))
                || use_counts.get(first) != Some(&1)
                || !append_result_is_private(function, *dest, name)
            {
                continue;
            }
            append_by_dest.insert(*dest, name.clone());
            append_loads.insert(*first);
        }
    }

    if append_by_dest.is_empty() {
        return;
    }
    let builder_names: HashSet<String> = append_by_dest.values().cloned().collect();
    for block in function.blocks_mut() {
        let original = std::mem::take(block.instructions_mut());
        let mut rewritten = Vec::with_capacity(original.len() + 1);
        for instruction in original {
            if let Instruction::StringConcatVa { dest, parts } = &instruction
                && append_by_dest.contains_key(dest)
            {
                rewritten.push(Instruction::CallBuiltin {
                    dest: Some(*dest),
                    builtin: Builtin::StringBuilderAppend,
                    args: parts.clone(),
                });
                continue;
            }
            let finish = match &instruction {
                Instruction::LoadVar { dest, name }
                    if builder_names.contains(name) && !append_loads.contains(dest) =>
                {
                    Some(*dest)
                }
                _ => None,
            };
            rewritten.push(instruction);
            if let Some(dest) = finish {
                rewritten.push(Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::StringBuilderFinish,
                    args: vec![dest],
                });
            }
        }
        *block.instructions_mut() = rewritten;
    }
}

fn append_result_is_private(function: &Function, dest: ValueId, name: &str) -> bool {
    for block in function.blocks() {
        for instruction in block.instructions() {
            if !instr_uses(instruction).contains(&dest) {
                continue;
            }
            let allowed = match instruction {
                Instruction::StoreVar {
                    name: store_name,
                    value,
                } => store_name == name && *value == dest,
                Instruction::IsException { value, .. } => *value == dest,
                Instruction::CallBuiltin {
                    builtin: Builtin::ExceptionValue,
                    args,
                    ..
                } => args == &[dest],
                _ => false,
            };
            if !allowed {
                return false;
            }
        }
        if terminator_uses(block.terminator()).contains(&dest) {
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn collect_concat_parts(
    value: ValueId,
    before: usize,
    instructions: &[Instruction],
    definitions: &HashMap<ValueId, usize>,
    use_counts: &HashMap<ValueId, usize>,
    kinds: &HashMap<ValueId, PrimitiveKind>,
    parts: &mut Vec<ValueId>,
    removed: &mut HashSet<usize>,
) -> bool {
    let Some(kind) = kinds.get(&value) else {
        return false;
    };
    if *kind == PrimitiveKind::String
        && let Some(&index) = definitions.get(&value)
        && index < before
        && use_counts.get(&value) == Some(&1)
    {
        match &instructions[index] {
            Instruction::Binary {
                op: BinaryOp::Add,
                lhs,
                rhs,
                ..
            } => {
                let mut child_parts = Vec::new();
                let mut child_removed = HashSet::new();
                if collect_concat_parts(
                    *lhs,
                    index,
                    instructions,
                    definitions,
                    use_counts,
                    kinds,
                    &mut child_parts,
                    &mut child_removed,
                ) && collect_concat_parts(
                    *rhs,
                    index,
                    instructions,
                    definitions,
                    use_counts,
                    kinds,
                    &mut child_parts,
                    &mut child_removed,
                ) {
                    parts.extend(child_parts);
                    removed.extend(child_removed);
                    removed.insert(index);
                    return true;
                }
            }
            Instruction::StringConcatVa {
                parts: child_parts, ..
            } if child_parts.iter().all(|part| kinds.contains_key(part)) => {
                parts.extend(child_parts.iter().copied());
                removed.insert(index);
                return true;
            }
            _ => {}
        }
    }
    parts.push(value);
    true
}

fn collect_use_counts(function: &Function) -> HashMap<ValueId, usize> {
    let mut counts = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            for value in instr_uses(instruction) {
                *counts.entry(value).or_insert(0) += 1;
            }
            if let Instruction::Phi { sources, .. } = instruction {
                for source in sources {
                    *counts.entry(source.value).or_insert(0) += 1;
                }
            }
        }
        for value in terminator_uses(block.terminator()) {
            *counts.entry(value).or_insert(0) += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, ConstantId, Terminator};

    #[test]
    fn fuses_primitive_string_accumulator_chain() {
        let constants = vec![
            Constant::String(String::new()),
            Constant::Number(0.0),
            Constant::String("x".into()),
        ];
        let mut function = Function::new("work", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$1.s".into(),
            value: ValueId(0),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: ConstantId(1),
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(1),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$1.s".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(3),
            constant: ConstantId(2),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$1.i".into(),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(5),
            op: BinaryOp::Add,
            lhs: ValueId(3),
            rhs: ValueId(4),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(6),
            op: BinaryOp::Add,
            lhs: ValueId(2),
            rhs: ValueId(5),
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$1.s".into(),
            value: ValueId(6),
        });
        block.push_instruction(Instruction::IsException {
            dest: ValueId(7),
            value: ValueId(6),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(8),
            name: "$1.s".into(),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(8)),
        });
        function.push_block(block);

        let locals = HashSet::from(["$1.s".to_owned(), "$1.i".to_owned()]);
        let (kinds, local_kinds) = infer_stable_kinds(&function, &constants, &locals);
        fuse_function(&mut function, &kinds);
        lower_builders(&mut function, &kinds, &local_kinds);

        assert!(function.blocks()[0].instructions().iter().any(|instruction| {
            matches!(instruction, Instruction::CallBuiltin { dest: Some(ValueId(6)), builtin: Builtin::StringBuilderAppend, args } if args == &[ValueId(2), ValueId(3), ValueId(4)])
        }));
        assert!(function.blocks()[0].instructions().iter().any(|instruction| {
            matches!(instruction, Instruction::CallBuiltin { dest: None, builtin: Builtin::StringBuilderFinish, args } if args == &[ValueId(8)])
        }));
        assert!(
            !function.blocks()[0]
                .instructions()
                .iter()
                .any(|instruction| { instruction_dest(instruction) == Some(ValueId(5)) })
        );
    }

    #[test]
    fn keeps_numeric_subexpression_grouped() {
        let constants = vec![
            Constant::String("x".into()),
            Constant::Number(1.0),
            Constant::Number(2.0),
        ];
        let mut function = Function::new("grouped", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        for (value, constant) in [(0, 0), (1, 1), (2, 2)] {
            block.push_instruction(Instruction::Const {
                dest: ValueId(value),
                constant: ConstantId(constant),
            });
        }
        block.push_instruction(Instruction::Binary {
            dest: ValueId(3),
            op: BinaryOp::Add,
            lhs: ValueId(1),
            rhs: ValueId(2),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(4),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(3),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(4)),
        });
        function.push_block(block);

        let (kinds, _) = infer_stable_kinds(&function, &constants, &HashSet::new());
        fuse_function(&mut function, &kinds);

        assert!(matches!(
            function.blocks()[0].instructions()[3],
            Instruction::Binary {
                dest: ValueId(3),
                ..
            }
        ));
        assert!(matches!(
            function.blocks()[0].instructions()[4],
            Instruction::Binary {
                dest: ValueId(4),
                ..
            }
        ));
    }
}
