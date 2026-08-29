//! 指令边界 SSA 活跃集，供 deopt 物化与 generic resume 共用。

use std::collections::HashSet;

use wjsm_ir::{BasicBlockId, Function, FunctionId, Program, ValueId};

use crate::ir_walk::{instruction_dest, terminator_uses};

pub fn live_values_at(
    program: &Program,
    function: FunctionId,
    block: BasicBlockId,
    instruction: usize,
) -> Vec<ValueId> {
    let Some(function) = program.functions().get(function.0 as usize) else {
        return Vec::new();
    };
    live_in_at(function, block, instruction)
}

pub(crate) fn live_in_at(
    function: &Function,
    block_id: BasicBlockId,
    instruction: usize,
) -> Vec<ValueId> {
    let Some(block) = function
        .blocks()
        .iter()
        .find(|block| block.id() == block_id)
    else {
        return Vec::new();
    };
    let mut live: HashSet<ValueId> = terminator_uses(block.terminator()).into_iter().collect();
    let instructions = block.instructions();
    let mut index = instructions.len();
    while index > instruction {
        index -= 1;
        let current = &instructions[index];
        if let Some(dest) = instruction_dest(current) {
            live.remove(&dest);
        }
        for value in current.uses() {
            live.insert(value);
        }
    }
    let mut values: Vec<ValueId> = live.into_iter().collect();
    values.sort_by_key(|value| value.0);
    values
}
