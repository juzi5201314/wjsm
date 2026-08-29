//! 块内公共子表达式：相同 Const / LoadSlot / GuardShape 复用 dest。

use std::collections::HashMap;

use wjsm_ir::{Function, FunctionId, Instruction, Program, ValueId};

pub fn run(program: &mut Program) {
    let count = program.functions().len();
    for index in 0..count {
        let Some(function) = program.function_mut(FunctionId(index as u32)) else {
            continue;
        };
        gvn_function(function);
    }
}

fn gvn_function(function: &mut Function) {
    for block_index in 0..function.blocks().len() {
        let mut aliases: HashMap<ValueId, ValueId> = HashMap::new();
        let mut consts: HashMap<u32, ValueId> = HashMap::new();
        let mut slots: HashMap<(ValueId, u32), ValueId> = HashMap::new();
        let mut shapes: HashMap<(ValueId, u32), ValueId> = HashMap::new();
        let instructions = function.blocks()[block_index].instructions().to_vec();
        let mut kept = Vec::with_capacity(instructions.len());
        for mut instruction in instructions {
            remap_instruction(&mut instruction, &aliases);
            match &instruction {
                Instruction::Const { dest, constant } => {
                    if let Some(existing) = consts.get(&constant.0) {
                        aliases.insert(*dest, *existing);
                        continue;
                    }
                    consts.insert(constant.0, *dest);
                }
                Instruction::LoadSlot {
                    dest,
                    object,
                    index,
                } => {
                    if let Some(existing) = slots.get(&(*object, *index)) {
                        aliases.insert(*dest, *existing);
                        continue;
                    }
                    slots.insert((*object, *index), *dest);
                }
                Instruction::GuardShape {
                    dest,
                    object,
                    shape_id,
                } => {
                    if let Some(existing) = shapes.get(&(*object, *shape_id)) {
                        aliases.insert(*dest, *existing);
                        continue;
                    }
                    shapes.insert((*object, *shape_id), *dest);
                }
                Instruction::StoreSlot { object, index, .. } => {
                    slots.remove(&(*object, *index));
                }
                _ => {}
            }
            kept.push(instruction);
        }
        *function.blocks_mut()[block_index].instructions_mut() = kept;
        apply_aliases(function, &aliases);
    }
}

/// 块内删除的 CSE dest 可能被其它块使用，必须全函数改写。
fn apply_aliases(function: &mut Function, aliases: &HashMap<ValueId, ValueId>) {
    if aliases.is_empty() {
        return;
    }
    for block in function.blocks_mut() {
        for instruction in block.instructions_mut() {
            let dest = instruction.dest();
            instruction.remap_values(&mut |value| {
                if dest == Some(value) {
                    value
                } else {
                    resolve_alias(aliases, value)
                }
            });
        }
        block
            .terminator_mut()
            .remap_values(&mut |value| resolve_alias(aliases, value));
    }
}

fn resolve_alias(aliases: &HashMap<ValueId, ValueId>, mut value: ValueId) -> ValueId {
    let mut guard = 0_u32;
    while let Some(&next) = aliases.get(&value) {
        if next == value || guard >= 32 {
            break;
        }
        value = next;
        guard += 1;
    }
    value
}

fn remap_instruction(instruction: &mut Instruction, aliases: &HashMap<ValueId, ValueId>) {
    let dest = instruction.dest();
    instruction.remap_values(&mut |value| {
        if dest == Some(value) {
            value
        } else {
            resolve_alias(aliases, value)
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Constant, Function, Program, Terminator, ValueId};

    #[test]
    fn gvn_rewrites_eliminated_const_uses_in_other_blocks() {
        let mut program = Program::new();
        let c = program.add_constant(Constant::Number(1.0));
        let mut function = Function::new("t", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: c,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: c,
        });
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut exit = BasicBlock::new(BasicBlockId(1));
        exit.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(entry);
        function.push_block(exit);
        program.push_function(function);
        run(&mut program);
        program.verify().expect("GVN aliases must stay defined");
        let terminator = program.functions()[0].blocks()[1].terminator();
        assert_eq!(
            terminator,
            &Terminator::Return {
                value: Some(ValueId(0))
            }
        );
    }
}
