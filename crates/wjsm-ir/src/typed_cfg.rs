//! 按值类重建 CFG：折叠证明不抛的 `is_exception`，把 Number 关系比较改成 `Compare`。
//!
//! 原地替换与终止器改写。投机 overlay 的反馈槽对齐由 `wjsm-optimize` 的 `slot_map` 负责。

use std::collections::{HashMap, HashSet};

use crate::cfg::{self, ControlFlowGraph};
use crate::value_class::{FunctionSeeds, infer_function};
use crate::{
    BasicBlockId, Builtin, CompareOp, Constant, FunctionId, Instruction, Program, Terminator,
    ValueId,
};

/// 对整包每个函数跑值类折叠。`seeds` 缺省则只靠常量证明。
pub fn rewrite_program(program: &mut Program, seeds: &HashMap<u32, FunctionSeeds>) -> bool {
    let mut any = false;
    let function_count = program.functions().len();
    for index in 0..function_count {
        any |= rewrite_function(
            program,
            FunctionId(index as u32),
            seeds.get(&(index as u32)),
        );
    }
    any
}

/// 对单个函数原地改写。
pub fn rewrite_function(
    program: &mut Program,
    function_id: FunctionId,
    seeds: Option<&FunctionSeeds>,
) -> bool {
    let default_seeds = FunctionSeeds::default();
    let seeds = seeds.unwrap_or(&default_seeds);
    let classes = {
        let function = match program.functions().get(function_id.0 as usize) {
            Some(function) => function,
            None => return false,
        };
        let frame_locals = program.frame_local_variable_names(function);
        infer_function(program, function, &frame_locals, seeds)
    };

    let bool_false = const_id_or_add(program, Constant::Bool(false));
    let defs = {
        let function = program
            .functions()
            .get(function_id.0 as usize)
            .expect("function id checked");
        let mut defs = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Some(dest) = dest_of(instruction) {
                    defs.insert(dest, instruction.clone());
                }
            }
        }
        defs
    };

    let function = program
        .function_mut(function_id)
        .expect("function id checked");
    let mut changed = false;
    for block in function.blocks_mut() {
        for instruction in block.instructions_mut() {
            if let Instruction::IsException { dest, value } = instruction
                && (classes.cannot_throw(*value)
                    || is_always_non_exception(&defs, *value, &classes.numbers))
            {
                *instruction = Instruction::Const {
                    dest: *dest,
                    constant: bool_false,
                };
                changed = true;
            }
        }
    }

    changed |= rewrite_abstract_compares(program, function_id, &classes.numbers);
    changed |= fold_const_branches(program, function_id);
    changed
}

fn rewrite_abstract_compares(
    program: &mut Program,
    function_id: FunctionId,
    numbers: &HashSet<ValueId>,
) -> bool {
    let constants: Vec<Constant> = program.constants().to_vec();
    let defs = {
        let function = program
            .functions()
            .get(function_id.0 as usize)
            .expect("function");
        let mut defs = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Some(dest) = dest_of(instruction) {
                    defs.insert(dest, instruction.clone());
                }
            }
        }
        defs
    };
    let function = program.function_mut(function_id).expect("function");
    let mut changed = false;
    for block in function.blocks_mut() {
        for instruction in block.instructions_mut() {
            let Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::AbstractCompare,
                args,
            } = instruction
            else {
                continue;
            };
            if args.len() != 4 || !numbers.contains(&args[0]) || !numbers.contains(&args[1]) {
                continue;
            }
            let Some(op) = relational_op(&defs, &constants, args) else {
                continue;
            };
            *instruction = Instruction::Compare {
                dest: *dest,
                op,
                lhs: args[0],
                rhs: args[1],
            };
            changed = true;
        }
    }
    changed
}

fn relational_op(
    defs: &HashMap<ValueId, Instruction>,
    constants: &[Constant],
    args: &[ValueId],
) -> Option<CompareOp> {
    let reverse = const_bool(defs, constants, args[2])?;
    let invert = const_bool(defs, constants, args[3])?;
    Some(match (reverse, invert) {
        (false, false) => CompareOp::Lt,
        (true, false) => CompareOp::Gt,
        (true, true) => CompareOp::LtEq,
        (false, true) => CompareOp::GtEq,
    })
}

fn const_bool(
    defs: &HashMap<ValueId, Instruction>,
    constants: &[Constant],
    value: ValueId,
) -> Option<bool> {
    match defs.get(&value)? {
        Instruction::Const { constant, .. } => match constants.get(constant.0 as usize)? {
            Constant::Bool(flag) => Some(*flag),
            _ => None,
        },
        _ => None,
    }
}

fn fold_const_branches(program: &mut Program, function_id: FunctionId) -> bool {
    let constants: Vec<Constant> = program.constants().to_vec();
    let function = program.function_mut(function_id).expect("function");
    let mut const_bools = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction
                && let Some(Constant::Bool(flag)) = constants.get(constant.0 as usize)
            {
                const_bools.insert(*dest, *flag);
            }
        }
    }
    let mut changed = false;
    for block in function.blocks_mut() {
        if let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = block.terminator()
            && let Some(flag) = const_bools.get(condition).copied()
        {
            let target = if flag { *true_block } else { *false_block };
            block.set_terminator(Terminator::Jump { target });
            changed = true;
        }
    }
    if changed {
        neutralize_unreachable(function);
    }
    changed
}

fn neutralize_unreachable(function: &mut crate::Function) {
    let cfg = ControlFlowGraph::build(function);
    for (index, block) in function.blocks_mut().iter_mut().enumerate() {
        if !cfg.is_reachable(BasicBlockId(index as u32)) {
            block.set_terminator(Terminator::Unreachable);
        }
    }
}

fn dest_of(instruction: &Instruction) -> Option<ValueId> {
    match instruction {
        Instruction::Const { dest, .. }
        | Instruction::Binary { dest, .. }
        | Instruction::Unary { dest, .. }
        | Instruction::Compare { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::LoadVar { dest, .. }
        | Instruction::IsException { dest, .. } => Some(*dest),
        Instruction::CallBuiltin { dest, .. } => *dest,
        _ => None,
    }
}

fn is_always_non_exception(
    defs: &HashMap<ValueId, Instruction>,
    value: ValueId,
    numbers: &HashSet<ValueId>,
) -> bool {
    if numbers.contains(&value) {
        return true;
    }
    match defs.get(&value) {
        Some(
            Instruction::Const { .. }
            | Instruction::NewObject { .. }
            | Instruction::InitObjectLiteral { .. }
            | Instruction::Compare { .. }
            | Instruction::NewArray { .. },
        ) => true,
        _ => false,
    }
}

fn const_id_or_add(program: &mut Program, constant: Constant) -> crate::ConstantId {
    for (index, existing) in program.constants().iter().enumerate() {
        if *existing == constant {
            return crate::ConstantId(index as u32);
        }
    }
    program.add_constant(constant)
}

/// 循环头：存在被该块支配的回边前驱。
pub fn loop_headers(function: &crate::Function) -> Vec<BasicBlockId> {
    let cfg = ControlFlowGraph::build(function);
    let idom = cfg.immediate_dominators();
    let mut headers = Vec::new();
    for index in 0..function.blocks().len() {
        let header = BasicBlockId(index as u32);
        let has_backedge = cfg
            .predecessors(header)
            .iter()
            .any(|pred| dominates(&idom, header, *pred));
        if has_backedge {
            headers.push(header);
        }
    }
    headers
}

/// DFS 回边集合：见 [`ControlFlowGraph::dfs_back_edges`]。
pub fn dfs_back_edges(function: &crate::Function) -> HashSet<(BasicBlockId, BasicBlockId)> {
    ControlFlowGraph::build(function).dfs_back_edges()
}

/// 循环头入口处需要保存/恢复的 SSA：该块全部 φ dest。
pub fn loop_header_live_phis(function: &crate::Function, header: BasicBlockId) -> Vec<ValueId> {
    let Some(block) = function.blocks().get(header.0 as usize) else {
        return Vec::new();
    };
    block
        .instructions()
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::Phi { dest, .. } => Some(*dest),
            _ => None,
        })
        .collect()
}

fn dominates(
    idom: &[Option<BasicBlockId>],
    dominator: BasicBlockId,
    mut node: BasicBlockId,
) -> bool {
    if dominator == node {
        return true;
    }
    let mut seen = HashSet::new();
    while seen.insert(node) {
        match idom.get(node.0 as usize).copied().flatten() {
            Some(parent) if parent == dominator => return true,
            Some(parent) => node = parent,
            None => return false,
        }
    }
    false
}

pub fn terminator_successors(terminator: &Terminator) -> Vec<BasicBlockId> {
    cfg::terminator_successors(terminator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BasicBlock, BinaryOp, Function, UnaryOp};

    #[test]
    fn folds_number_add_is_exception_and_rewrites_lt() {
        let mut program = Program::new();
        let zero = program.add_constant(Constant::Number(0.0));
        let n = program.add_constant(Constant::Number(8.0));
        let one = program.add_constant(Constant::Number(1.0));
        let false_c = program.add_constant(Constant::Bool(false));
        let _true_c = program.add_constant(Constant::Bool(true));
        let mut function = Function::new("loop", BasicBlockId(0));
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: zero,
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::Phi {
            dest: ValueId(1),
            sources: vec![
                crate::PhiSource {
                    predecessor: BasicBlockId(0),
                    value: ValueId(0),
                },
                crate::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: ValueId(4),
                },
            ],
        });
        bb1.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: n,
        });
        bb1.push_instruction(Instruction::Const {
            dest: ValueId(10),
            constant: false_c,
        });
        bb1.push_instruction(Instruction::Const {
            dest: ValueId(11),
            constant: false_c,
        });
        bb1.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(3)),
            builtin: Builtin::AbstractCompare,
            args: vec![ValueId(1), ValueId(2), ValueId(10), ValueId(11)],
        });
        bb1.set_terminator(Terminator::Branch {
            condition: ValueId(3),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::Const {
            dest: ValueId(5),
            constant: one,
        });
        bb2.push_instruction(Instruction::Binary {
            dest: ValueId(4),
            op: BinaryOp::Add,
            lhs: ValueId(1),
            rhs: ValueId(5),
        });
        bb2.push_instruction(Instruction::IsException {
            dest: ValueId(6),
            value: ValueId(4),
        });
        bb2.set_terminator(Terminator::Branch {
            condition: ValueId(6),
            true_block: BasicBlockId(4),
            false_block: BasicBlockId(1),
        });
        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        let mut bb4 = BasicBlock::new(BasicBlockId(4));
        bb4.push_instruction(Instruction::Unary {
            dest: ValueId(7),
            op: UnaryOp::Pos,
            value: ValueId(4),
        });
        bb4.set_terminator(Terminator::Throw { value: ValueId(7) });
        function.push_block(bb0);
        function.push_block(bb1);
        function.push_block(bb2);
        function.push_block(bb3);
        function.push_block(bb4);
        program.push_function(function);

        assert!(rewrite_program(&mut program, &HashMap::new()));
        let dumped = program.dump_text();
        assert!(
            dumped.contains("lt %1, %2"),
            "relational compare should become Compare: {dumped}"
        );
        assert!(
            !dumped.contains("is_exception"),
            "number add must not keep is_exception: {dumped}"
        );
    }
}
