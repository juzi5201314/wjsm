//! 当反馈证明循环首轮 elements kind 与稳态不同时，剥一层循环体。
//!
//! 剥离副本保持 generic `GetElem`（覆盖首轮 kind）；原循环体随后由
//! `rewrite_speculative` 按稳态 kind 插守卫。只剥一层，且仅在存在
//! `first_kind != elements_kind` 的 GetElem 时执行。

use std::collections::{HashMap, HashSet};

use wjsm_ir::cfg::ControlFlowGraph;
use wjsm_ir::{
    BasicBlockId, Dominators, Function, Instruction, PhiSource, Program, ValueId, typed_cfg,
};

use crate::facts::SpeculativeFacts;
use crate::inline_for_ea::max_value_id_in_function;
use crate::ir_walk::{instruction_dest, terminator_uses};

pub fn peel_first_iteration(program: &mut Program, facts: &SpeculativeFacts) {
    let Some(function) = program.function_mut(facts.function) else {
        return;
    };
    let headers = typed_cfg::loop_headers(function);
    for header in headers {
        if !loop_needs_peel(function, header, facts) {
            continue;
        }
        peel_loop(function, header);
    }
}

fn loop_needs_peel(function: &Function, header: BasicBlockId, facts: &SpeculativeFacts) -> bool {
    let body = natural_loop(function, header);
    facts.get_elems.iter().any(|fact| {
        body.contains(&fact.block)
            && fact
                .first_kind
                .is_some_and(|first| first != fact.elements_kind)
            && function.block_by_id(fact.block).is_some_and(|block| {
                matches!(
                    block.instructions().get(fact.instruction_index as usize),
                    Some(Instruction::GetElem { .. })
                )
            })
    })
}

fn natural_loop(function: &Function, header: BasicBlockId) -> HashSet<BasicBlockId> {
    let cfg = ControlFlowGraph::build(function);
    let dom = Dominators::compute(function);
    let mut body = HashSet::from([header]);
    let mut stack: Vec<BasicBlockId> = cfg
        .predecessors(header)
        .iter()
        .copied()
        .filter(|pred| *pred != header && dom.dominates(header, *pred))
        .collect();
    while let Some(block) = stack.pop() {
        if !body.insert(block) {
            continue;
        }
        for pred in cfg.predecessors(block) {
            if *pred != header {
                stack.push(*pred);
            }
        }
    }
    body
}

fn peel_loop(function: &mut Function, header: BasicBlockId) {
    let body = natural_loop(function, header);
    let mut next_value = max_value_id_in_function(function).saturating_add(1);
    let mut block_map = HashMap::new();
    let mut clones = Vec::new();
    let mut ordered: Vec<BasicBlockId> = body.iter().copied().collect();
    ordered.sort_by_key(|id| id.0);
    for old_id in &ordered {
        let new_id = BasicBlockId(function.blocks().len() as u32 + clones.len() as u32);
        block_map.insert(*old_id, new_id);
        let Some(source) = function.block_by_id(*old_id) else {
            return;
        };
        clones.push(source.clone());
    }
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
    for clone in &mut clones {
        for instruction in clone.instructions_mut() {
            if let Some(dest) = instruction_dest(instruction) {
                let mapped = ValueId(next_value);
                next_value += 1;
                value_map.insert(dest, mapped);
            }
        }
    }
    let mut remap_value = |value: ValueId| *value_map.get(&value).unwrap_or(&value);
    let peel_header = *block_map.get(&header).expect("header cloned");
    for (clone, old_id) in clones.iter_mut().zip(ordered.iter()) {
        clone.set_id(*block_map.get(old_id).expect("clone id"));
        for instruction in clone.instructions_mut() {
            instruction.remap_values(&mut remap_value);
            if let Instruction::Phi { sources, .. } = instruction {
                if *old_id == header {
                    sources.retain(|source| !body.contains(&source.predecessor));
                } else {
                    for source in sources.iter_mut() {
                        if body.contains(&source.predecessor) {
                            source.predecessor = *block_map
                                .get(&source.predecessor)
                                .unwrap_or(&source.predecessor);
                        }
                    }
                }
            }
        }
        clone.terminator_mut().remap_values(&mut remap_value);
        clone.terminator_mut().remap_blocks(&mut |block| {
            if block == header {
                header
            } else {
                *block_map.get(&block).unwrap_or(&block)
            }
        });
    }
    let outside: Vec<BasicBlockId> = function
        .blocks()
        .iter()
        .map(|block| block.id())
        .filter(|id| !body.contains(id))
        .collect();
    for id in outside {
        if let Some(block) = function.block_by_id_mut(id) {
            block.terminator_mut().remap_blocks(&mut |target| {
                if target == header {
                    peel_header
                } else {
                    target
                }
            });
        }
    }
    if let Some(original) = function.block_by_id_mut(header) {
        for instruction in original.instructions_mut() {
            if let Instruction::Phi { sources, .. } = instruction {
                let extras: Vec<PhiSource> = sources
                    .iter()
                    .filter(|source| body.contains(&source.predecessor))
                    .filter_map(|source| {
                        let peel_pred = *block_map.get(&source.predecessor)?;
                        Some(PhiSource {
                            predecessor: peel_pred,
                            value: remap_value(source.value),
                        })
                    })
                    .collect();
                sources.extend(extras);
            }
        }
    }
    for clone in clones {
        function.push_block(clone);
    }
    insert_lcssa(function, &body, &block_map, &value_map, &mut next_value);
}

/// 剥离后循环出口同时接到 peel 与原循环头，循环内 SSA 必须在出口汇合。
fn insert_lcssa(
    function: &mut Function,
    body: &HashSet<BasicBlockId>,
    block_map: &HashMap<BasicBlockId, BasicBlockId>,
    value_map: &HashMap<ValueId, ValueId>,
    next_value: &mut u32,
) {
    let peel: HashSet<BasicBlockId> = block_map.values().copied().collect();
    let mut loopish = body.clone();
    loopish.extend(peel.iter().copied());
    let mut defined = HashSet::new();
    for id in body {
        let Some(block) = function.block_by_id(*id) else {
            continue;
        };
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defined.insert(dest);
            }
        }
    }
    let mut outside_uses: HashMap<ValueId, HashSet<BasicBlockId>> = HashMap::new();
    for block in function.blocks() {
        if loopish.contains(&block.id()) {
            continue;
        }
        for instruction in block.instructions() {
            for used in instruction.uses() {
                if defined.contains(&used) {
                    outside_uses.entry(used).or_default().insert(block.id());
                }
            }
        }
        for used in terminator_uses(block.terminator()) {
            if defined.contains(&used) {
                outside_uses.entry(used).or_default().insert(block.id());
            }
        }
    }
    let cfg = ControlFlowGraph::build(function);
    for (value, use_blocks) in outside_uses {
        for use_block in use_blocks {
            let sources: Vec<PhiSource> = cfg
                .predecessors(use_block)
                .iter()
                .map(|pred| {
                    let incoming = if peel.contains(pred) {
                        *value_map.get(&value).unwrap_or(&value)
                    } else {
                        value
                    };
                    PhiSource {
                        predecessor: *pred,
                        value: incoming,
                    }
                })
                .collect();
            if sources.is_empty() {
                continue;
            }
            let dest = ValueId(*next_value);
            *next_value += 1;
            let Some(block) = function.block_by_id_mut(use_block) else {
                continue;
            };
            block
                .instructions_mut()
                .insert(0, Instruction::Phi { dest, sources });
            for instruction in block.instructions_mut().iter_mut().skip(1) {
                instruction.remap_values(&mut |used| {
                    if used == value { dest } else { used }
                });
            }
            block.terminator_mut().remap_values(&mut |used| {
                if used == value { dest } else { used }
            });
        }
    }
}
