//! 指令/终止器 use-def 走访，供 Sound 优化与 semantic 语言 pass 共用。

use wjsm_ir::{Function, Instruction, Terminator, ValueId};

pub fn instr_uses(ins: &Instruction) -> Vec<ValueId> {
    ins.uses()
}

pub fn terminator_uses(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Return { value: Some(value) } => vec![*value],
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Switch { value, .. } => vec![*value],
        Terminator::Throw { value } => vec![*value],
        Terminator::Deopt { frames } => frames
            .iter()
            .flat_map(|frame| frame.lives.iter().copied())
            .collect(),
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {
            vec![]
        }
    }
}

pub fn collect_uses(function: &Function, target: ValueId) -> Vec<&Instruction> {
    let mut uses = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if instruction.uses().contains(&target) {
                uses.push(instruction);
            }
        }
    }
    uses
}

pub fn instruction_dest(ins: &Instruction) -> Option<ValueId> {
    ins.dest()
}
