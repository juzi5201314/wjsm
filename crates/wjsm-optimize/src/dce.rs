//! 删除无副作用且结果未被使用的指令。

use std::collections::HashSet;

use wjsm_ir::{Function, FunctionId, Instruction, Program, ValueId};

use crate::ir_walk::{instruction_dest, terminator_uses};

pub fn run(program: &mut Program) {
    let count = program.functions().len();
    for index in 0..count {
        let Some(function) = program.function_mut(FunctionId(index as u32)) else {
            continue;
        };
        dce_function(function);
    }
}

fn dce_function(function: &mut Function) {
    let mut used = HashSet::new();
    for block in function.blocks() {
        for value in terminator_uses(block.terminator()) {
            used.insert(value);
        }
        for instruction in block.instructions() {
            for value in instruction.uses() {
                used.insert(value);
            }
        }
    }
    for block in function.blocks_mut() {
        let kept: Vec<Instruction> = block
            .instructions()
            .iter()
            .filter(|instruction| instruction_is_live(instruction, &used))
            .cloned()
            .collect();
        *block.instructions_mut() = kept;
    }
}

fn instruction_is_live(instruction: &Instruction, used: &HashSet<ValueId>) -> bool {
    if instruction_has_effect(instruction) {
        return true;
    }
    instruction_dest(instruction).is_none_or(|dest| used.contains(&dest))
}

fn instruction_has_effect(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::StoreVar { .. }
            | Instruction::SetProp { .. }
            | Instruction::SetElem { .. }
            | Instruction::StoreSlot { .. }
            | Instruction::Call { .. }
            | Instruction::SuperCall { .. }
            | Instruction::ConstructCall { .. }
            | Instruction::CallBuiltin { .. }
            | Instruction::DeleteProp { .. }
            | Instruction::SetProto { .. }
            | Instruction::PromiseResolve { .. }
            | Instruction::PromiseReject { .. }
            | Instruction::Suspend { .. }
            | Instruction::GeneratorSuspend { .. }
            | Instruction::ObjectSpread { .. }
            | Instruction::DebugCheck { .. }
            | Instruction::CreateDataProperty { .. }
    )
}

pub fn prune_unreachable(program: &mut Program) {
    let count = program.functions().len();
    for index in 0..count {
        let Some(function) = program.function_mut(FunctionId(index as u32)) else {
            continue;
        };
        prune_function(function);
    }
}

fn prune_function(function: &mut Function) {
    let mut reachable = HashSet::new();
    let mut stack = vec![function.entry()];
    while let Some(block_id) = stack.pop() {
        if !reachable.insert(block_id) {
            continue;
        }
        let Some(block) = function
            .blocks()
            .iter()
            .find(|block| block.id() == block_id)
        else {
            continue;
        };
        for successor in wjsm_ir::cfg::terminator_successors(block.terminator()) {
            stack.push(successor);
        }
    }
    let kept: Vec<_> = function
        .blocks()
        .iter()
        .filter(|block| reachable.contains(&block.id()))
        .cloned()
        .collect();
    function.replace_blocks(kept);
    compact_block_ids(function);
}

/// 删除中间不可达块后，令 `blocks[i].id == i`，并重写全部块引用。
/// 向量下标即 id 是 verify / `block_by_id` 的不变量。
fn compact_block_ids(function: &mut Function) {
    let max_old = function
        .blocks()
        .iter()
        .map(|block| block.id().0 as usize)
        .max()
        .unwrap_or(0);
    // 旧 id 稀疏、上界等于历史块数；用下标表代替 HashMap。
    let mut to_new = vec![None; max_old + 1];
    for (index, block) in function.blocks().iter().enumerate() {
        to_new[block.id().0 as usize] = Some(wjsm_ir::BasicBlockId(index as u32));
    }
    let mut remap = |id: wjsm_ir::BasicBlockId| {
        to_new
            .get(id.0 as usize)
            .copied()
            .flatten()
            .expect("reachable 块引用必须仍在压缩后的向量里")
    };
    function.set_entry(remap(function.entry()));
    for block in function.blocks_mut() {
        block.set_id(remap(block.id()));
        for instruction in block.instructions_mut() {
            instruction.remap_blocks(&mut remap);
        }
        block.terminator_mut().remap_blocks(&mut remap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Function, Program, Terminator};

    #[test]
    fn prune_compacts_block_ids_to_vector_order() {
        let mut program = Program::new();
        let mut function = Function::new("t", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(2),
        });
        let dead = BasicBlock::new_with_terminator(BasicBlockId(1), Terminator::Unreachable);
        let mut exit = BasicBlock::new(BasicBlockId(2));
        exit.set_terminator(Terminator::Return { value: None });
        function.push_block(entry);
        function.push_block(dead);
        function.push_block(exit);
        program.push_function(function);
        prune_unreachable(&mut program);
        let function = &program.functions()[0];
        assert_eq!(function.blocks().len(), 2);
        assert_eq!(function.blocks()[0].id(), BasicBlockId(0));
        assert_eq!(function.blocks()[1].id(), BasicBlockId(1));
        assert_eq!(
            function.blocks()[0].terminator(),
            &Terminator::Jump {
                target: BasicBlockId(1)
            }
        );
        program.verify().expect("compacted IR must verify");
    }
}
