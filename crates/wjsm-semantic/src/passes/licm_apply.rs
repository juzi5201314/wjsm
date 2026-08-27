//! licm pass 的变换阶段：建 pre-header、修 header phi、重定向外部前驱、
//! 把计划内的指令移入 pre-header。
//!
//! 正确性要点：
//! - 外部前驱（不在循环体内的块）全部重定向到 pre-header，回边保持指向
//!   header，循环结构不变；
//! - header 的 phi 把外部来源合并为 pre-header 来源（多外部前驱时在
//!   pre-header 新建合并 phi），内部（回边）来源原样保留；
//! - 被移动指令按「无操作数的 Const/LoadVar 在前、消费者在后」排序，
//!   依赖深度 ≤ 1，块内 def-before-use 因此成立。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{BasicBlock, BasicBlockId, Function, Instruction, PhiSource, Terminator, ValueId};

/// 一轮变换计划：对 `header` 的循环建 pre-header，把 `moves` 移入。
pub(crate) struct Plan {
    pub(crate) header: BasicBlockId,
    pub(crate) body: HashSet<BasicBlockId>,
    /// 待移动指令位置（块 id + 块内下标），已去重。
    pub(crate) moves: Vec<(BasicBlockId, usize)>,
}

pub(crate) fn apply_plan(function: &mut Function, plan: &Plan, next_value: &mut u32) {
    let preheader_id = BasicBlockId(function.blocks().len() as u32);
    let extracted = extract_moves(function, plan);
    let preheader_phis = split_header_phis(function, plan, preheader_id, next_value);
    retarget_external_predecessors(function, plan, preheader_id);
    push_preheader(
        function,
        plan.header,
        preheader_id,
        preheader_phis,
        extracted,
    );
}

/// 取出被移动指令。无操作数的 Const/LoadVar 在前，消费它们的 GetProp/Call
/// 在后（依赖深度 ≤ 1，组内保持原程序序）。
fn extract_moves(function: &mut Function, plan: &Plan) -> Vec<Instruction> {
    let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
    for (block, index) in &plan.moves {
        by_block.entry(*block).or_default().push(*index);
    }
    let mut extracted: Vec<(BasicBlockId, usize, Instruction)> = Vec::new();
    for (block_id, mut indices) in by_block {
        indices.sort_unstable_by(|left, right| right.cmp(left));
        let Some(block) = function.block_by_id_mut(block_id) else {
            continue;
        };
        for index in indices {
            let instruction = block.instructions_mut().remove(index);
            extracted.push((block_id, index, instruction));
        }
    }
    extracted.sort_by_key(|(block, index, instruction)| {
        let leaf = matches!(
            instruction,
            Instruction::Const { .. } | Instruction::LoadVar { .. }
        );
        (!leaf, block.0, *index)
    });
    extracted
        .into_iter()
        .map(|(_, _, instruction)| instruction)
        .collect()
}

/// header phi 修正：外部来源合并到 pre-header；多外部前驱时返回需要放进
/// pre-header 的合并 phi。
fn split_header_phis(
    function: &mut Function,
    plan: &Plan,
    preheader_id: BasicBlockId,
    next_value: &mut u32,
) -> Vec<Instruction> {
    let mut preheader_phis: Vec<Instruction> = Vec::new();
    let Some(header) = function.block_by_id_mut(plan.header) else {
        return preheader_phis;
    };
    for instruction in header.instructions_mut() {
        let Instruction::Phi { sources, .. } = instruction else {
            continue;
        };
        let (internal, external): (Vec<PhiSource>, Vec<PhiSource>) = sources
            .drain(..)
            .partition(|source| plan.body.contains(&source.predecessor));
        debug_assert!(!external.is_empty(), "可达循环头必有外部前驱");
        if external.len() == 1 {
            let mut merged = external;
            merged[0].predecessor = preheader_id;
            merged.extend(internal);
            *sources = merged;
        } else {
            let merged_value = ValueId(*next_value);
            *next_value += 1;
            preheader_phis.push(Instruction::Phi {
                dest: merged_value,
                sources: external,
            });
            let mut merged = vec![PhiSource {
                predecessor: preheader_id,
                value: merged_value,
            }];
            merged.extend(internal);
            *sources = merged;
        }
    }
    preheader_phis
}

/// 外部前驱重定向到 pre-header（回边在循环体内，保持指向 header）。
fn retarget_external_predecessors(
    function: &mut Function,
    plan: &Plan,
    preheader_id: BasicBlockId,
) {
    let header = plan.header;
    for block in function.blocks_mut() {
        if plan.body.contains(&block.id()) {
            continue;
        }
        block.terminator_mut().remap_blocks(&mut |target| {
            if target == header {
                preheader_id
            } else {
                target
            }
        });
    }
}

/// 追加 pre-header：合并 phi 在前，被移动指令随后。
fn push_preheader(
    function: &mut Function,
    header: BasicBlockId,
    preheader_id: BasicBlockId,
    preheader_phis: Vec<Instruction>,
    moved: Vec<Instruction>,
) {
    let mut preheader =
        BasicBlock::new_with_terminator(preheader_id, Terminator::Jump { target: header });
    for phi in preheader_phis {
        preheader.push_instruction(phi);
    }
    for instruction in moved {
        preheader.push_instruction(instruction);
    }
    function.push_block(preheader);
}
