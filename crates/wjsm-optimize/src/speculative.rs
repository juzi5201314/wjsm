//! 把稳定反馈编成 Guard + LoadSlot / StoreSlot / Deopt。

use std::collections::HashMap;

use wjsm_ir::{
    BasicBlock, BasicBlockId, Constant, ConstantId, DeoptFrame, Function, FunctionId, Instruction,
    Program, Terminator, ValueId,
};

use crate::facts::{CallFact, ElemFact, PropFact, SpeculativeFacts};
use crate::{DeoptMap, DeoptMapEntry, SlotMap, SlotMapEntry};

pub fn rewrite_speculative(
    program: &mut Program,
    facts: &SpeculativeFacts,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
) {
    let mut call_consts = HashMap::new();
    for fact in &facts.calls {
        let mut targets = vec![fact.target_function];
        targets.extend(
            fact.poly_functions
                .iter()
                .copied()
                .take(fact.poly_len as usize),
        );
        for target in targets {
            if call_consts.contains_key(&target) {
                continue;
            }
            let id = program.add_constant(Constant::FunctionRef(FunctionId(target)));
            call_consts.insert(target, id);
        }
    }
    let net = program_instruction_count(program);
    let Some(function) = program.function_mut(facts.function) else {
        return;
    };
    rewrite_function(
        function,
        facts.function,
        facts,
        deopt_map,
        slot_map,
        &call_consts,
        net,
    );
}

fn program_instruction_count(program: &Program) -> usize {
    program
        .functions()
        .iter()
        .map(|function| {
            function
                .blocks()
                .iter()
                .map(|block| block.instructions().len())
                .sum::<usize>()
        })
        .sum()
}

fn rewrite_function(
    function: &mut Function,
    function_id: FunctionId,
    facts: &SpeculativeFacts,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    call_consts: &std::collections::HashMap<u32, ConstantId>,
    net_instructions: usize,
) {
    let mut next_value = max_value(function).saturating_add(1);
    let original_blocks: Vec<BasicBlockId> =
        function.blocks().iter().map(|block| block.id()).collect();
    for block_id in original_blocks {
        rewrite_block(
            function,
            function_id,
            facts,
            deopt_map,
            slot_map,
            block_id,
            &mut next_value,
            call_consts,
            net_instructions,
        );
    }
}

fn rewrite_block(
    function: &mut Function,
    function_id: FunctionId,
    facts: &SpeculativeFacts,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_id: BasicBlockId,
    next_value: &mut u32,
    call_consts: &std::collections::HashMap<u32, ConstantId>,
    net_instructions: usize,
) {
    let Some(block_index) = function
        .blocks()
        .iter()
        .position(|block| block.id() == block_id)
    else {
        return;
    };
    let instructions = function.blocks()[block_index].instructions().to_vec();
    for (instruction_index, instruction) in instructions.iter().enumerate().rev() {
        match instruction {
            Instruction::GetProp { dest, object, .. }
                if facts
                    .prop_at(block_id, instruction_index as u32)
                    .is_some_and(|fact| {
                        fact.own_data && fact.poly_len >= 1 && fact.poly_len <= 4
                    }) =>
            {
                let fact = facts
                    .prop_at(block_id, instruction_index as u32)
                    .copied()
                    .expect("prop fact");
                let poly = (0..fact.poly_len as usize)
                    .filter(|index| fact.poly_shapes[*index] != 0)
                    .count();
                if poly >= 2 {
                    crate::speculative_poly::split_poly_get_prop(
                        function,
                        function_id,
                        deopt_map,
                        slot_map,
                        block_index,
                        *dest,
                        *object,
                        &fact,
                        next_value,
                        instruction_index as u32,
                    );
                } else {
                    split_get_prop(
                        function,
                        function_id,
                        deopt_map,
                        slot_map,
                        block_index,
                        *dest,
                        *object,
                        &fact,
                        next_value,
                        instruction_index as u32,
                    );
                }
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            Instruction::SetProp {
                dest,
                object,
                value,
                ..
            } if facts
                .set_prop_at(block_id, instruction_index as u32)
                .is_some_and(|fact| fact.own_data && fact.poly_len >= 1 && fact.poly_len <= 4) =>
            {
                let fact = facts
                    .set_prop_at(block_id, instruction_index as u32)
                    .copied()
                    .expect("set prop fact");
                split_set_prop(
                    function,
                    function_id,
                    deopt_map,
                    slot_map,
                    block_index,
                    *dest,
                    *object,
                    *value,
                    &fact,
                    next_value,
                    instruction_index as u32,
                );
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            Instruction::Call { dest, callee, .. }
                if facts
                    .call_at(block_id, instruction_index as u32)
                    .is_some_and(|fact| fact.poly_len >= 1 && fact.poly_len <= 4)
                    || facts.call_at(block_id, instruction_index as u32).is_some() =>
            {
                let fact = *facts
                    .call_at(block_id, instruction_index as u32)
                    .expect("call fact");
                let poly = fact
                    .poly_functions
                    .iter()
                    .take(fact.poly_len as usize)
                    .filter(|id| **id != 0)
                    .count();
                if poly >= 2 {
                    crate::speculative_poly::split_poly_call(
                        function,
                        function_id,
                        deopt_map,
                        slot_map,
                        block_index,
                        *dest,
                        *callee,
                        &fact,
                        call_consts,
                        next_value,
                        instruction_index as u32,
                        instruction.clone(),
                    );
                } else {
                    split_call(
                        function,
                        function_id,
                        deopt_map,
                        slot_map,
                        block_index,
                        *dest,
                        *callee,
                        &fact,
                        call_consts,
                        next_value,
                        instruction_index as u32,
                        net_instructions,
                    );
                }
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            Instruction::Binary { dest, lhs, rhs, .. }
            | Instruction::Compare { dest, lhs, rhs, .. }
                if facts
                    .binary_at(block_id, instruction_index as u32)
                    .is_some() =>
            {
                let fact = *facts
                    .binary_at(block_id, instruction_index as u32)
                    .expect("binary fact");
                split_binary(
                    function,
                    function_id,
                    deopt_map,
                    slot_map,
                    block_index,
                    *dest,
                    *lhs,
                    *rhs,
                    &fact,
                    next_value,
                    instruction_index as u32,
                    instruction.clone(),
                );
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            Instruction::GetElem { dest, object, .. }
                if facts.elem_at(block_id, instruction_index as u32).is_some() =>
            {
                let fact = *facts
                    .elem_at(block_id, instruction_index as u32)
                    .expect("elem fact");
                split_get_elem(
                    function,
                    function_id,
                    deopt_map,
                    slot_map,
                    block_index,
                    *dest,
                    *object,
                    &fact,
                    next_value,
                    instruction_index as u32,
                );
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            Instruction::SetElem {
                dest,
                object,
                value,
                ..
            } if facts
                .set_elem_at(block_id, instruction_index as u32)
                .is_some() =>
            {
                let fact = *facts
                    .set_elem_at(block_id, instruction_index as u32)
                    .expect("set elem fact");
                split_set_elem(
                    function,
                    function_id,
                    deopt_map,
                    slot_map,
                    block_index,
                    *dest,
                    *object,
                    *value,
                    &fact,
                    next_value,
                    instruction_index as u32,
                );
                rewrite_block(
                    function,
                    function_id,
                    facts,
                    deopt_map,
                    slot_map,
                    block_id,
                    next_value,
                    call_consts,
                    net_instructions,
                );
                return;
            }
            _ => {}
        }
    }
}

fn split_get_prop(
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
    let prefix_len = generic_instruction as usize;
    split_access(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        prefix_len,
        generic_instruction,
        object,
        Instruction::GuardShape {
            dest: ValueId(*next_value),
            object,
            shape_id: fact.shape_id,
        },
        Instruction::LoadSlot {
            dest,
            object,
            index: fact.slot_index,
        },
        next_value,
    );
}

fn split_set_prop(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: ValueId,
    object: ValueId,
    value: ValueId,
    fact: &PropFact,
    next_value: &mut u32,
    generic_instruction: u32,
) {
    split_access(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction as usize,
        generic_instruction,
        object,
        Instruction::GuardShape {
            dest: ValueId(*next_value),
            object,
            shape_id: fact.shape_id,
        },
        Instruction::StoreSlot {
            dest: Some(dest),
            object,
            index: fact.slot_index,
            value,
            transition_shape: fact.transition_shape,
        },
        next_value,
    );
}

fn split_get_elem(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: ValueId,
    object: ValueId,
    fact: &ElemFact,
    next_value: &mut u32,
    generic_instruction: u32,
) {
    let all = function.blocks()[block_index].instructions().to_vec();
    let Instruction::GetElem { index, .. } = all[generic_instruction as usize] else {
        return;
    };
    let guard = if fact.typed_kind.is_some() {
        Instruction::GuardTag {
            dest: ValueId(*next_value),
            value: object,
            tag: 0x08,
        }
    } else {
        Instruction::GuardElementsKind {
            dest: ValueId(*next_value),
            array: object,
            kind: fact.elements_kind,
            template: None,
        }
    };
    split_access(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction as usize,
        generic_instruction,
        object,
        guard,
        Instruction::GetElem {
            dest,
            object,
            index,
            latch: None,
        },
        next_value,
    );
}

fn split_set_elem(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: ValueId,
    object: ValueId,
    value: ValueId,
    fact: &ElemFact,
    next_value: &mut u32,
    generic_instruction: u32,
) {
    let all = function.blocks()[block_index].instructions().to_vec();
    let Instruction::SetElem { index, strict, .. } = all[generic_instruction as usize] else {
        return;
    };
    let guard = if fact.typed_kind.is_some() {
        Instruction::GuardTag {
            dest: ValueId(*next_value),
            value: object,
            tag: 0x08,
        }
    } else {
        Instruction::GuardElementsKind {
            dest: ValueId(*next_value),
            array: object,
            kind: fact.elements_kind,
            template: None,
        }
    };
    split_access(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction as usize,
        generic_instruction,
        object,
        guard,
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
            strict,
        },
        next_value,
    );
}

fn split_call(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: Option<wjsm_ir::ValueId>,
    callee: ValueId,
    fact: &CallFact,
    call_consts: &std::collections::HashMap<u32, ConstantId>,
    next_value: &mut u32,
    generic_instruction: u32,
    net_instructions: usize,
) {
    let all = function.blocks()[block_index].instructions().to_vec();
    let Some(mut call) = all.get(generic_instruction as usize).cloned() else {
        return;
    };
    let target = fact.target_function;
    if fact.poly_len == 0 && target == 0 && fact.poly_functions.iter().all(|id| *id == 0) {
        return;
    }
    let _ = net_instructions;
    if let Some(constant) = call_consts.get(&target).copied() {
        let fn_val = ValueId(*next_value);
        *next_value += 1;
        match &mut call {
            Instruction::Call {
                callee: call_callee,
                ..
            }
            | Instruction::ConstructCall {
                callee: call_callee,
                ..
            } => {
                *call_callee = fn_val;
            }
            _ => {}
        }
        split_access(
            function,
            function_id,
            deopt_map,
            slot_map,
            block_index,
            generic_instruction as usize,
            generic_instruction,
            callee,
            Instruction::GuardCallTarget {
                dest: ValueId(*next_value),
                callee,
                function: FunctionId(target),
            },
            Instruction::Const {
                dest: fn_val,
                constant,
            },
            next_value,
        );
        // Const 占 fast 首条，Call 必须紧随其后：再把 Call 插进 fast 块。
        let fast_id = BasicBlockId(function.blocks().len() as u32 - 2);
        if let Some(fast) = function.block_by_id_mut(fast_id) {
            let mut instructions = fast.instructions().to_vec();
            if matches!(instructions.first(), Some(Instruction::Const { .. })) {
                instructions.insert(1, call);
                *fast.instructions_mut() = instructions;
            }
        }
        let _ = dest;
        return;
    }
    split_access(
        function,
        function_id,
        deopt_map,
        slot_map,
        block_index,
        generic_instruction as usize,
        generic_instruction,
        callee,
        Instruction::GuardCallTarget {
            dest: ValueId(*next_value),
            callee,
            function: FunctionId(target),
        },
        call,
        next_value,
    );
    let _ = dest;
}

fn split_binary(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    dest: ValueId,
    lhs: ValueId,
    rhs: ValueId,
    fact: &crate::facts::BinaryFact,
    next_value: &mut u32,
    generic_instruction: u32,
    binary: Instruction,
) {
    let _ = dest;
    let old_id = function.blocks()[block_index].id();
    let lives = {
        let mut lives = crate::liveness::live_in_at(function, old_id, generic_instruction as usize);
        for value in [lhs, rhs] {
            if !lives.contains(&value) {
                lives.push(value);
            }
        }
        lives
    };
    let terminator = function.blocks()[block_index].terminator().clone();
    let all = function.blocks()[block_index].instructions().to_vec();
    let prefix = all[..generic_instruction as usize].to_vec();
    let suffix = all[generic_instruction as usize + 1..].to_vec();
    let lhs_guard = ValueId(*next_value);
    *next_value += 1;
    let rhs_guard = ValueId(*next_value);
    *next_value += 1;
    let mid_id = BasicBlockId(function.blocks().len() as u32);
    let fast_id = BasicBlockId(function.blocks().len() as u32 + 1);
    let deopt_id = BasicBlockId(function.blocks().len() as u32 + 2);

    let mut head = BasicBlock::new(old_id);
    for instruction in prefix {
        head.push_instruction(instruction);
    }
    head.push_instruction(Instruction::GuardTag {
        dest: lhs_guard,
        value: lhs,
        tag: fact.lhs_tag,
    });
    head.set_terminator(Terminator::Branch {
        condition: lhs_guard,
        true_block: mid_id,
        false_block: deopt_id,
    });

    let mut mid = BasicBlock::new(mid_id);
    mid.push_instruction(Instruction::GuardTag {
        dest: rhs_guard,
        value: rhs,
        tag: fact.rhs_tag,
    });
    mid.set_terminator(Terminator::Branch {
        condition: rhs_guard,
        true_block: fast_id,
        false_block: deopt_id,
    });

    let mut fast = BasicBlock::new(fast_id);
    fast.push_instruction(binary);
    for instruction in suffix {
        fast.push_instruction(instruction);
    }
    fast.set_terminator(terminator);

    function.blocks_mut()[block_index] = head;
    function.push_block(mid);
    function.push_block(fast);
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
    slot_map.sites.push(SlotMapEntry {
        overlay_block: fast_id,
        overlay_instruction: 0,
        generic_block: old_id,
        generic_instruction,
    });
}

fn split_access(
    function: &mut Function,
    function_id: FunctionId,
    deopt_map: &mut DeoptMap,
    slot_map: &mut SlotMap,
    block_index: usize,
    prefix_len: usize,
    generic_instruction: u32,
    live_object: ValueId,
    guard: Instruction,
    fast_payload: Instruction,
    next_value: &mut u32,
) {
    let old_id = function.blocks()[block_index].id();
    let lives = {
        let mut lives = crate::liveness::live_in_at(function, old_id, prefix_len);
        if !lives.contains(&live_object) {
            lives.push(live_object);
        }
        lives
    };
    let terminator = function.blocks()[block_index].terminator().clone();
    let all = function.blocks()[block_index].instructions().to_vec();
    let prefix = all[..prefix_len].to_vec();
    let suffix = all[prefix_len + 1..].to_vec();
    let guard_dest = match &guard {
        Instruction::GuardShape { dest, .. }
        | Instruction::GuardElementsKind { dest, .. }
        | Instruction::GuardTag { dest, .. }
        | Instruction::GuardCallTarget { dest, .. } => *dest,
        _ => ValueId(*next_value),
    };
    *next_value = (*next_value).max(guard_dest.0 + 1);

    let fast_id = BasicBlockId(function.blocks().len() as u32);
    let deopt_id = BasicBlockId(function.blocks().len() as u32 + 1);

    let mut head = BasicBlock::new(old_id);
    for instruction in prefix {
        head.push_instruction(instruction);
    }
    let guard_index = head.instructions().len() as u32;
    head.push_instruction(guard);
    head.set_terminator(Terminator::Branch {
        condition: guard_dest,
        true_block: fast_id,
        false_block: deopt_id,
    });

    let mut fast = BasicBlock::new(fast_id);
    fast.push_instruction(fast_payload);
    for instruction in suffix {
        fast.push_instruction(instruction);
    }
    fast.set_terminator(terminator);

    let mut deopt = BasicBlock::new(deopt_id);
    deopt.set_terminator(Terminator::Deopt {
        frames: vec![DeoptFrame {
            function: function_id,
            block: old_id,
            instruction_index: generic_instruction,
            lives,
        }],
    });

    function.blocks_mut()[block_index] = head;
    function.push_block(fast);
    function.push_block(deopt);

    deopt_map.points.push(DeoptMapEntry {
        overlay_block: deopt_id,
        overlay_instruction: 0,
        generic_function: function_id,
        generic_block: old_id,
        generic_instruction,
    });
    slot_map.sites.push(SlotMapEntry {
        overlay_block: old_id,
        overlay_instruction: guard_index,
        generic_block: old_id,
        generic_instruction,
    });
    slot_map.sites.push(SlotMapEntry {
        overlay_block: fast_id,
        overlay_instruction: 0,
        generic_block: old_id,
        generic_instruction,
    });
}

fn max_value(function: &Function) -> u32 {
    let mut max = 0;
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction.dest() {
                max = max.max(dest.0);
            }
            for value in instruction.uses() {
                max = max.max(value.0);
            }
        }
    }
    max
}
