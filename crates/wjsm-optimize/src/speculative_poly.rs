//! 2–4 态 shape / 调用目标 jumptable：链式 Guard，命中走对应槽/调用，全失配 Deopt。

use wjsm_ir::{
    BasicBlock, BasicBlockId, ConstantId, DeoptFrame, Function, FunctionId, Instruction, PhiSource,
    Terminator, ValueId,
};

use crate::facts::{CallFact, PropFact};
use crate::{DeoptMap, DeoptMapEntry, SlotMap, SlotMapEntry};

pub(crate) fn split_poly_get_prop(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: ValueId,
    object: ValueId,
    fact: &PropFact,
    next_value: &mut u32,
    generic_instruction: u32,
) {
    let pairs: Vec<(u32, u32)> = (0..fact.poly_len as usize)
        .map(|index| (fact.poly_shapes[index], fact.poly_slots[index]))
        .filter(|(shape, _)| *shape != 0)
        .collect();
    if pairs.len() < 2 {
        return;
    }
    let payloads: Vec<Instruction> = pairs
        .iter()
        .map(|(_, slot)| {
            let loaded = ValueId(*next_value);
            *next_value += 1;
            Instruction::LoadSlot {
                dest: loaded,
                object,
                index: *slot,
            }
        })
        .collect();
    let dests: Vec<ValueId> = payloads
        .iter()
        .filter_map(crate::ir_walk::instruction_dest)
        .collect();
    emit_jumptable(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction,
        object,
        &pairs
            .iter()
            .map(|(shape, _)| Instruction::GuardShape {
                dest: ValueId(0),
                object,
                shape_id: *shape,
            })
            .collect::<Vec<_>>(),
        &payloads,
        &dests,
        dest,
        next_value,
    );
}

pub(crate) fn split_poly_call(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: Option<ValueId>,
    callee: ValueId,
    fact: &CallFact,
    call_consts: &std::collections::HashMap<u32, ConstantId>,
    next_value: &mut u32,
    generic_instruction: u32,
    original: Instruction,
) {
    let targets: Vec<u32> = fact
        .poly_functions
        .iter()
        .copied()
        .take(fact.poly_len as usize)
        .filter(|id| *id != 0)
        .collect();
    if targets.len() < 2 {
        return;
    }
    let mut payloads = Vec::new();
    let mut dests = Vec::new();
    for target in &targets {
        let Some(constant) = call_consts.get(target).copied() else {
            continue;
        };
        let fn_val = ValueId(*next_value);
        *next_value += 1;
        let result = dest.map(|_| {
            let id = ValueId(*next_value);
            *next_value += 1;
            id
        });
        if let Some(id) = result {
            dests.push(id);
        }
        let mut call = original.clone();
        match &mut call {
            Instruction::Call {
                dest: call_dest,
                callee: call_callee,
                ..
            }
            | Instruction::ConstructCall {
                dest: call_dest,
                callee: call_callee,
                ..
            } => {
                *call_dest = result;
                *call_callee = fn_val;
            }
            _ => {}
        }
        payloads.push(Instruction::Const {
            dest: fn_val,
            constant,
        });
        payloads.push(call);
    }
    let guards: Vec<Instruction> = targets
        .iter()
        .map(|target| Instruction::GuardCallTarget {
            dest: ValueId(0),
            callee,
            function: FunctionId(*target),
        })
        .collect();
    // jumptable 每个目标两条 payload（Const+Call），按目标切开。
    emit_call_jumptable(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction,
        callee,
        &guards,
        &payloads,
        &dests,
        dest,
        next_value,
    );
}

fn emit_call_jumptable(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    generic_instruction: u32,
    live_object: ValueId,
    guards: &[Instruction],
    payloads: &[Instruction],
    dests: &[ValueId],
    merge_dest: Option<ValueId>,
    next_value: &mut u32,
) {
    let n = guards.len();
    if n == 0 || payloads.len() != n * 2 {
        return;
    }
    let old_id = function.blocks()[block_index].id();
    let lives = {
        let mut lives = crate::liveness::live_in_at(function, old_id, generic_instruction as usize);
        if !lives.contains(&live_object) {
            lives.push(live_object);
        }
        lives
    };
    let terminator = function.blocks()[block_index].terminator().clone();
    let all = function.blocks()[block_index].instructions().to_vec();
    let prefix = all[..generic_instruction as usize].to_vec();
    let suffix = all[generic_instruction as usize + 1..].to_vec();

    // 追加顺序：try 链、fast 目标、merge、deopt。id 必须等于下标。
    let start = function.blocks().len() as u32;
    let try_ids: Vec<BasicBlockId> = (0..n.saturating_sub(1))
        .map(|index| BasicBlockId(start + index as u32))
        .collect();
    let fast_ids: Vec<BasicBlockId> = (0..n)
        .map(|index| BasicBlockId(start + (n as u32 - 1) + index as u32))
        .collect();
    let merge_id = BasicBlockId(start + (n as u32 - 1) + n as u32);
    let deopt_id = BasicBlockId(merge_id.0 + 1);

    let mut head = BasicBlock::new(old_id);
    for instruction in prefix {
        head.push_instruction(instruction);
    }
    let g0 = fill_guard_dest(guards[0].clone(), next_value);
    let g0_dest = crate::ir_walk::instruction_dest(&g0).expect("guard dest");
    head.push_instruction(g0);
    let next_miss = if n == 1 { deopt_id } else { try_ids[0] };
    head.set_terminator(Terminator::Branch {
        condition: g0_dest,
        true_block: fast_ids[0],
        false_block: next_miss,
    });
    function.blocks_mut()[block_index] = head;

    for index in 1..n {
        let mut try_block = BasicBlock::new(try_ids[index - 1]);
        let guard = fill_guard_dest(guards[index].clone(), next_value);
        let dest = crate::ir_walk::instruction_dest(&guard).expect("guard dest");
        try_block.push_instruction(guard);
        let miss = if index + 1 == n {
            deopt_id
        } else {
            try_ids[index]
        };
        try_block.set_terminator(Terminator::Branch {
            condition: dest,
            true_block: fast_ids[index],
            false_block: miss,
        });
        function.push_block(try_block);
    }

    for index in 0..n {
        let mut fast = BasicBlock::new(fast_ids[index]);
        fast.push_instruction(payloads[index * 2].clone());
        fast.push_instruction(payloads[index * 2 + 1].clone());
        fast.set_terminator(Terminator::Jump { target: merge_id });
        function.push_block(fast);
        slot_map.sites.push(SlotMapEntry {
            overlay_block: fast_ids[index],
            overlay_instruction: 1,
            generic_block: old_id,
            generic_instruction,
        });
    }

    let mut merge = BasicBlock::new(merge_id);
    if let Some(phi_dest) = merge_dest
        && dests.len() == n
    {
        merge.push_instruction(Instruction::Phi {
            dest: phi_dest,
            sources: dests
                .iter()
                .enumerate()
                .map(|(index, value)| PhiSource {
                    predecessor: fast_ids[index],
                    value: *value,
                })
                .collect(),
        });
    }
    for instruction in suffix {
        merge.push_instruction(instruction);
    }
    merge.set_terminator(terminator);
    function.push_block(merge);

    push_deopt(
        function,
        function_id,
        deopt_map,
        deopt_id,
        old_id,
        generic_instruction,
        lives,
    );
}

fn emit_jumptable(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    generic_instruction: u32,
    live_object: ValueId,
    guards_template: &[Instruction],
    payloads: &[Instruction],
    dests: &[ValueId],
    merge_dest: ValueId,
    next_value: &mut u32,
) {
    let n = payloads.len();
    if n < 2 || guards_template.len() != n {
        return;
    }
    let old_id = function.blocks()[block_index].id();
    let lives = {
        let mut lives = crate::liveness::live_in_at(function, old_id, generic_instruction as usize);
        if !lives.contains(&live_object) {
            lives.push(live_object);
        }
        lives
    };
    let terminator = function.blocks()[block_index].terminator().clone();
    let all = function.blocks()[block_index].instructions().to_vec();
    let prefix = all[..generic_instruction as usize].to_vec();
    let suffix = all[generic_instruction as usize + 1..].to_vec();

    let start = function.blocks().len() as u32;
    let try_ids: Vec<BasicBlockId> = (0..n.saturating_sub(1))
        .map(|index| BasicBlockId(start + index as u32))
        .collect();
    let fast_ids: Vec<BasicBlockId> = (0..n)
        .map(|index| BasicBlockId(start + (n as u32 - 1) + index as u32))
        .collect();
    let merge_id = BasicBlockId(start + (n as u32 - 1) + n as u32);
    let deopt_id = BasicBlockId(merge_id.0 + 1);

    let mut head = BasicBlock::new(old_id);
    for instruction in prefix {
        head.push_instruction(instruction);
    }
    let g0 = fill_guard_dest(guards_template[0].clone(), next_value);
    let g0_dest = crate::ir_walk::instruction_dest(&g0).expect("guard dest");
    head.push_instruction(g0);
    head.set_terminator(Terminator::Branch {
        condition: g0_dest,
        true_block: fast_ids[0],
        false_block: try_ids[0],
    });
    function.blocks_mut()[block_index] = head;

    for index in 1..n {
        let mut try_block = BasicBlock::new(try_ids[index - 1]);
        let guard = fill_guard_dest(guards_template[index].clone(), next_value);
        let dest = crate::ir_walk::instruction_dest(&guard).expect("guard dest");
        try_block.push_instruction(guard);
        let miss = if index + 1 == n {
            deopt_id
        } else {
            try_ids[index]
        };
        try_block.set_terminator(Terminator::Branch {
            condition: dest,
            true_block: fast_ids[index],
            false_block: miss,
        });
        function.push_block(try_block);
    }

    for index in 0..n {
        let mut fast = BasicBlock::new(fast_ids[index]);
        fast.push_instruction(payloads[index].clone());
        fast.set_terminator(Terminator::Jump { target: merge_id });
        function.push_block(fast);
        slot_map.sites.push(SlotMapEntry {
            overlay_block: fast_ids[index],
            overlay_instruction: 0,
            generic_block: old_id,
            generic_instruction,
        });
    }

    let mut merge = BasicBlock::new(merge_id);
    merge.push_instruction(Instruction::Phi {
        dest: merge_dest,
        sources: dests
            .iter()
            .enumerate()
            .map(|(index, value)| PhiSource {
                predecessor: fast_ids[index],
                value: *value,
            })
            .collect(),
    });
    for instruction in suffix {
        merge.push_instruction(instruction);
    }
    merge.set_terminator(terminator);
    function.push_block(merge);

    push_deopt(
        function,
        function_id,
        deopt_map,
        deopt_id,
        old_id,
        generic_instruction,
        lives,
    );
}

fn fill_guard_dest(mut guard: Instruction, next_value: &mut u32) -> Instruction {
    let dest = ValueId(*next_value);
    *next_value += 1;
    match &mut guard {
        Instruction::GuardShape {
            dest: guard_dest, ..
        }
        | Instruction::GuardCallTarget {
            dest: guard_dest, ..
        }
        | Instruction::GuardElementsKind {
            dest: guard_dest, ..
        }
        | Instruction::GuardTag {
            dest: guard_dest, ..
        } => *guard_dest = dest,
        _ => {}
    }
    guard
}

fn push_deopt(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    deopt_id: BasicBlockId,
    old_id: BasicBlockId,
    generic_instruction: u32,
    lives: Vec<ValueId>,
) {
    let mut deopt = BasicBlock::new(deopt_id);
    deopt.set_terminator(Terminator::Deopt {
        frames: vec![DeoptFrame {
            function: function_id,
            block: old_id,
            instruction_index: generic_instruction,
            lives,
        }],
    });
    function.push_block(deopt);
    deopt_map.points.push(DeoptMapEntry {
        overlay_block: deopt_id,
        overlay_instruction: 0,
        generic_function: function_id,
        generic_block: old_id,
        generic_instruction,
    });
}
