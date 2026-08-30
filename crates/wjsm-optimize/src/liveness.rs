//! 指令边界 SSA 活跃集，供 deopt 物化与 generic resume 共用。
//!
//! 必须做**全函数 CFG** 活跃性：循环头 OSR/deopt 恢复时，循环不变量（在头块
//! 内不再出现 use、只在后继体中使用）也必须进 resume 槽。块内逆向扫描会漏掉
//! 它们，Cranelift 在未定义 Variable 的 resume 前驱上会填 `iconst 0`，表现为
//! `array.has_element(0, 0)` / Map 路径 SIGSEGV。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Function, FunctionId, Instruction, Program, ValueId,
};
use wjsm_ir::typed_cfg::terminator_successors;

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
    let live_out = cfg_live_out(function)
        .remove(&block_id)
        .unwrap_or_default();
    let mut live = live_out;
    live.extend(terminator_uses(block.terminator()));
    let instructions = block.instructions();
    let mut index = instructions.len();
    while index > instruction {
        index -= 1;
        let current = &instructions[index];
        // φ 的 use 挂在前驱边上，块内不计入；dest 仍从 live 中剔除。
        if let Instruction::Phi { dest, .. } = current {
            live.remove(dest);
            continue;
        }
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

/// 各块 live-out（CFG 不动点；φ 源在边上，不进入后继 live-in）。
fn cfg_live_out(function: &Function) -> HashMap<BasicBlockId, HashSet<ValueId>> {
    let mut block_uses = HashMap::with_capacity(function.blocks().len());
    let mut block_defs = HashMap::with_capacity(function.blocks().len());
    let mut phi_defs = HashMap::with_capacity(function.blocks().len());
    let mut phi_edge_uses: HashMap<(BasicBlockId, BasicBlockId), HashSet<ValueId>> = HashMap::new();

    for block in function.blocks() {
        let mut uses = HashSet::new();
        let mut defs = HashSet::new();
        let mut block_phi_defs = HashSet::new();
        for instruction in block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                defs.insert(*dest);
                block_phi_defs.insert(*dest);
                for source in sources {
                    phi_edge_uses
                        .entry((source.predecessor, block.id()))
                        .or_default()
                        .insert(source.value);
                }
                continue;
            }
            for value in instruction.uses() {
                if !defs.contains(&value) {
                    uses.insert(value);
                }
            }
            if let Some(destination) = instruction_dest(instruction) {
                defs.insert(destination);
            }
        }
        for value in terminator_uses(block.terminator()) {
            if !defs.contains(&value) {
                uses.insert(value);
            }
        }
        block_uses.insert(block.id(), uses);
        block_defs.insert(block.id(), defs);
        phi_defs.insert(block.id(), block_phi_defs);
    }

    let mut live_in: HashMap<BasicBlockId, HashSet<ValueId>> = function
        .blocks()
        .iter()
        .map(|block| (block.id(), HashSet::new()))
        .collect();
    let mut live_out = live_in.clone();
    loop {
        let mut changed = false;
        for block in function.blocks().iter().rev() {
            let mut outgoing = HashSet::new();
            for successor in terminator_successors(block.terminator()) {
                if let Some(successor_live) = live_in.get(&successor) {
                    outgoing.extend(
                        successor_live
                            .iter()
                            .filter(|value| !phi_defs[&successor].contains(value))
                            .copied(),
                    );
                }
                if let Some(edge_uses) = phi_edge_uses.get(&(block.id(), successor)) {
                    outgoing.extend(edge_uses.iter().copied());
                }
            }

            let mut incoming = block_uses[&block.id()].clone();
            incoming.extend(
                outgoing
                    .iter()
                    .filter(|value| !block_defs[&block.id()].contains(value))
                    .copied(),
            );
            if live_out[&block.id()] != outgoing {
                live_out.insert(block.id(), outgoing);
                changed = true;
            }
            if live_in[&block.id()] != incoming {
                live_in.insert(block.id(), incoming);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    live_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{
        BasicBlock, BinaryOp, Builtin, Constant, Function, Instruction, PhiSource, Terminator,
    };

    /// 循环不变量在头块无直接 use 时，仍须出现在头指令的 resume live 集中。
    #[test]
    fn loop_invariant_is_live_at_header_body() {
        let mut program = Program::new();
        let zero = program.add_constant(Constant::Number(0.0));
        let one = program.add_constant(Constant::Number(1.0));
        let len_c = program.add_constant(Constant::Number(2.0));
        let mut function = Function::new("map_loop", BasicBlockId(0));

        // bb0: %arr = ...; %len = ...; %i0 = 0; jump bb1
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: zero, // 占位：当作 arr
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: len_c,
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: zero,
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        function.push_block(bb0);

        // bb1: %i = phi; %cond = cmp; branch body/exit
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::Phi {
            dest: ValueId(3),
            sources: vec![
                PhiSource {
                    predecessor: BasicBlockId(0),
                    value: ValueId(2),
                },
                PhiSource {
                    predecessor: BasicBlockId(2),
                    value: ValueId(5),
                },
            ],
        });
        bb1.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(4)),
            builtin: Builtin::AbstractCompare,
            args: vec![ValueId(3), ValueId(1), ValueId(2), ValueId(2)],
        });
        bb1.set_terminator(Terminator::Branch {
            condition: ValueId(4),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });
        function.push_block(bb1);

        // bb2: has_element(arr, i); i' = i+1; jump bb1
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(6)),
            builtin: Builtin::ArrayHasElement,
            args: vec![ValueId(0), ValueId(3)],
        });
        bb2.push_instruction(Instruction::Const {
            dest: ValueId(7),
            constant: one,
        });
        bb2.push_instruction(Instruction::Binary {
            dest: ValueId(5),
            op: BinaryOp::Add,
            lhs: ValueId(3),
            rhs: ValueId(7),
        });
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        function.push_block(bb2);

        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.set_terminator(Terminator::Return {
            value: Some(ValueId(0)),
        });
        function.push_block(bb3);

        let function_id = program.push_function(function);
        // 头块第一条非 φ 指令（AbstractCompare）处：arr / len / i 都须在 live 集。
        let lives = live_values_at(&program, function_id, BasicBlockId(1), 1);
        assert!(
            lives.contains(&ValueId(0)),
            "循环不变量 arr 必须在 header resume lives 中: {lives:?}"
        );
        assert!(
            lives.contains(&ValueId(1)),
            "循环不变量 len 必须在 header resume lives 中: {lives:?}"
        );
        assert!(
            lives.contains(&ValueId(3)),
            "循环 φ i 必须在 header resume lives 中: {lives:?}"
        );
    }
}
