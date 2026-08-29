//! licm pass 的变换阶段：建 pre-header、修 header phi、重定向外部前驱、
//! 把计划内的指令移入 pre-header，并按 elem-guard 计划做成对指令替换。
//!
//! 正确性要点：
//! - 外部前驱（不在循环体内的块）全部重定向到 pre-header，回边保持指向
//!   header，循环结构不变；
//! - header 的 phi 把外部来源合并为 pre-header 来源（多外部前驱时在
//!   pre-header 新建合并 phi），内部（回边）来源原样保留；
//! - 被移动指令按「无操作数的 Const/LoadVar 在前、消费者在后」排序，
//!   依赖深度 ≤ 1，块内 def-before-use 因此成立；
//! - elem-guard 的循环体内替换是原地改写（不动块结构和下标），必须先于
//!   `extract_moves` 执行；`GuardElementsKind`（带模板）追加在 pre-header 被移动指令之后
//!   （其 array 操作数可能正是本轮或往轮被移动进 pre-header 的 LoadVar）。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlock, BasicBlockId, ConstantId, Function, Instruction, PhiSource, Terminator, ValueId,
};

/// 一轮变换计划：对 `header` 的循环建 pre-header，把 `moves` 移入，
/// 并按 `elem_guards` 替换循环体内的成对元素/属性访问。
pub(crate) struct Plan {
    pub(crate) header: BasicBlockId,
    pub(crate) body: HashSet<BasicBlockId>,
    /// 待移动指令位置（块 id + 块内下标），已去重。
    pub(crate) moves: Vec<(BasicBlockId, usize)>,
    /// elem-guard 外提计划（见 [`super::licm_elem_guard`]）。
    pub(crate) elem_guards: Vec<ElemGuard>,
}

/// 单个被守卫数组的替换计划：pre-header 插入一条带模板的 `GuardElementsKind`，
/// 循环体内的 `GetElem`/`GetProp` 带上共享闩锁。
pub(crate) struct ElemGuard {
    /// 循环不变的数组值（定义在循环体外）。
    pub(crate) array: ValueId,
    /// 元素统一的对象字面量模板。
    pub(crate) template: ConstantId,
    /// 待加上闩锁的 `GetElem` 位置。
    pub(crate) elem_sites: Vec<(BasicBlockId, usize)>,
    /// 待加上闩锁与模板的 `GetProp` 位置。
    pub(crate) prop_sites: Vec<(BasicBlockId, usize)>,
}

pub(crate) fn apply_plan(function: &mut Function, plan: &Plan, next_value: &mut u32) {
    let preheader_id = BasicBlockId(function.blocks().len() as u32);
    let guard_instructions = apply_elem_guards(function, plan, next_value);
    let extracted = extract_moves(function, plan);
    let preheader_phis = split_header_phis(function, plan, preheader_id, next_value);
    retarget_external_predecessors(function, plan, preheader_id);
    push_preheader(
        function,
        plan.header,
        preheader_id,
        preheader_phis,
        extracted,
        guard_instructions,
    );
}

/// 按计划原地替换循环体内的成对访问，返回要追加进 pre-header 的
/// 带模板的 `GuardElementsKind`（每个被守卫数组一条，dest 即共享守卫值）。
fn apply_elem_guards(
    function: &mut Function,
    plan: &Plan,
    next_value: &mut u32,
) -> Vec<Instruction> {
    let mut guards = Vec::new();
    for guard in &plan.elem_guards {
        let guard_value = ValueId(*next_value);
        *next_value += 1;
        for (block, index) in &guard.elem_sites {
            replace_site(function, *block, *index, |instruction| match instruction {
                Instruction::GetElem {
                    dest,
                    object,
                    index,
                    ..
                } => Some(Instruction::GetElem {
                    dest,
                    object,
                    index,
                    latch: Some(guard_value),
                }),
                _ => None,
            });
        }
        for (block, index) in &guard.prop_sites {
            replace_site(function, *block, *index, |instruction| match instruction {
                Instruction::GetProp {
                    dest, object, key, ..
                } => Some(Instruction::GetProp {
                    dest,
                    object,
                    key,
                    latch: Some(guard_value),
                    latch_template: Some(guard.template),
                }),
                _ => None,
            });
        }
        guards.push(Instruction::GuardElementsKind {
            dest: guard_value,
            array: guard.array,
            kind: wjsm_ir::constants::ARRAY_KIND_PACKED,
            template: Some(guard.template),
        });
    }
    guards
}

/// 原地替换指定位置的指令；`rewrite` 返回 `None`（站点形状与计划不符）时
/// 保持原指令不变——计划与函数体同轮生成，正常不可能失配。
fn replace_site(
    function: &mut Function,
    block: BasicBlockId,
    index: usize,
    rewrite: impl FnOnce(Instruction) -> Option<Instruction>,
) {
    let Some(slot) = function
        .block_by_id_mut(block)
        .and_then(|block| block.instructions_mut().get_mut(index))
    else {
        return;
    };
    if let Some(replaced) = rewrite(slot.clone()) {
        *slot = replaced;
    }
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

/// 追加 pre-header：合并 phi 在前，被移动指令随后，`GuardElementsKind` 收尾
/// （守卫的 array 操作数可能是刚移入的 LoadVar）。
fn push_preheader(
    function: &mut Function,
    header: BasicBlockId,
    preheader_id: BasicBlockId,
    preheader_phis: Vec<Instruction>,
    moved: Vec<Instruction>,
    guards: Vec<Instruction>,
) {
    let mut preheader =
        BasicBlock::new_with_terminator(preheader_id, Terminator::Jump { target: header });
    for phi in preheader_phis {
        preheader.push_instruction(phi);
    }
    for instruction in moved {
        preheader.push_instruction(instruction);
    }
    for guard in guards {
        preheader.push_instruction(guard);
    }
    function.push_block(preheader);
}
