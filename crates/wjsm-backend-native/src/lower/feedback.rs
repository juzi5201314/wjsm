//! 反馈槽与 resume 目标判定：编号必须与 generic / overlay 一致。

#![allow(unused_imports)]
use std::collections::{HashMap, HashSet};

use wjsm_ir::{BasicBlockId, BinaryOp, CompareOp, Instruction, Program, UnaryOp, ValueId};

/// 每函数的反馈槽 plan：`(block, instruction) → 全局槽下标`。
///
/// 槽只按指令形态分配（Binary/Unary/Compare/CallBuiltin 与四种 call 系列全部计入，
/// 静态已证明 f64 或 typed-thunk 的指令不会写自己的槽），因此 base image 与运行时
/// 特化 overlay 对同一 Program 必然得到完全一致的编号——overlay 生成代码经由
/// vmctx 的 `feedback_slots_base` 继续写 base image 的槽，编号错位会把反馈记到
/// 别的调用点上。`LoadArgument`/`LoadCallEnv`/`FinishCall` 等内部 bookkeeping
/// 操作不分配槽；Shape IC 继续使用自己的 IC 槽。
#[derive(Debug, Default)]
pub(crate) struct FeedbackSitePlan {
    per_function: Vec<HashMap<(BasicBlockId, usize), u32>>,
    total: u32,
}

impl FeedbackSitePlan {
    pub(crate) fn total_slots(&self) -> u32 {
        self.total
    }

    pub(crate) fn function_slots(&self, index: usize) -> &HashMap<(BasicBlockId, usize), u32> {
        self.per_function
            .get(index)
            .expect("feedback plan covers every function")
    }
}

pub(crate) fn allocate_feedback_slots(program: &Program) -> FeedbackSitePlan {
    let mut per_function = Vec::with_capacity(program.functions().len());
    let mut slot_index = 0_u32;
    for function in program.functions() {
        let mut slots = HashMap::new();
        for block in function.blocks() {
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                if instruction_owns_feedback_slot(instruction) {
                    slots.insert((block.id(), instruction_index), slot_index);
                    slot_index += 1;
                }
            }
        }
        per_function.push(slots);
    }
    FeedbackSitePlan {
        per_function,
        total: slot_index,
    }
}

/// 判定一条指令是否需要独立的 generic resume pad。
///
/// 槽编号仍只看 [`instruction_owns_feedback_slot`]（base / overlay 对齐）。
/// pad 则可以看 `f64_values`：已证明 f64 的 LoadVar/StoreVar/算术/关系比较
/// 在 CLIF 里不会 deopt，切开独立块只会逼 Cranelift 在每条指令边界 spill xmm。
/// overlay 精确 deopt 仍落在 Guard / GetProp / 动态 Binary / GetElem 等锚点。
pub(crate) fn is_resume_target(instruction: &Instruction, f64_values: &HashSet<ValueId>) -> bool {
    if proven_f64_op_cannot_deopt(instruction, f64_values) {
        return false;
    }
    instruction_owns_feedback_slot(instruction)
        || matches!(
            instruction,
            Instruction::GuardTag { .. }
                | Instruction::GuardShape { .. }
                | Instruction::GuardElementsKind { .. }
                | Instruction::GuardCallTarget { .. }
                | Instruction::LoadSlot { .. }
                | Instruction::StoreSlot { .. }
        )
}

/// 静态已证明的 f64 热路径：lowering 只发原生浮点算子，没有 deopt 边。
fn proven_f64_op_cannot_deopt(instruction: &Instruction, f64_values: &HashSet<ValueId>) -> bool {
    match instruction {
        Instruction::LoadVar { dest, .. } => f64_values.contains(dest),
        Instruction::StoreVar { value, .. } => f64_values.contains(value),
        Instruction::Binary { dest, op, .. } => {
            f64_values.contains(dest)
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                )
        }
        Instruction::Unary { dest, op, .. } => {
            f64_values.contains(dest) && matches!(op, UnaryOp::Neg | UnaryOp::Pos)
        }
        Instruction::Compare { op, lhs, rhs, .. } => {
            op.is_relational() && f64_values.contains(lhs) && f64_values.contains(rhs)
        }
        _ => false,
    }
}

pub(crate) fn instruction_owns_feedback_slot(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Binary { .. }
            | Instruction::Unary { .. }
            | Instruction::Compare { .. }
            | Instruction::CallBuiltin { .. }
            | Instruction::Call { .. }
            | Instruction::SuperCall { .. }
            | Instruction::ConstructCall { .. }
            | Instruction::GetProp { .. }
            | Instruction::SetProp { .. }
            | Instruction::DeleteProp { .. }
            | Instruction::GetElem { .. }
            | Instruction::SetElem { .. }
            | Instruction::NewObject { .. }
            | Instruction::NewArray { .. }
            | Instruction::InitObjectLiteral { .. }
            | Instruction::LoadVar { .. }
            | Instruction::StoreVar { .. }
    )
}

/// 本条指令的 lowering 是否会读/写反馈槽指针。
///
/// 槽**编号**仍按 [`instruction_owns_feedback_slot`] 分配（base / overlay 对齐），
/// 但静态已证明的 f64 算术/比较、帧局部 LoadVar/StoreVar、以及 GetElem/SetElem
/// 的 lowering 从不写槽。为它们计算 `feedback_slots_base + slot×80` 会在每个
/// 热循环块留下一条死 load；Cranelift 目前不会消掉它。
pub(crate) fn lowering_uses_feedback_ptr(
    instruction: &Instruction,
    f64_values: &HashSet<ValueId>,
) -> bool {
    if !instruction_owns_feedback_slot(instruction) {
        return false;
    }
    match instruction {
        Instruction::LoadVar { .. }
        | Instruction::StoreVar { .. }
        | Instruction::GetElem { .. }
        | Instruction::SetElem { .. } => false,
        Instruction::Binary { dest, op, .. }
            if f64_values.contains(dest)
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
        {
            false
        }
        Instruction::Unary { dest, op, .. }
            if f64_values.contains(dest) && matches!(op, UnaryOp::Neg | UnaryOp::Pos) =>
        {
            false
        }
        Instruction::Compare { op, lhs, rhs, .. }
            if op.is_relational() && f64_values.contains(lhs) && f64_values.contains(rhs) =>
        {
            false
        }
        _ => true,
    }
}

/// Program 的反馈槽总数；cache 命中时与条目内记录的计数校验。
pub(crate) fn feedback_site_count(program: &Program) -> u32 {
    allocate_feedback_slots(program).total_slots()
}

/// 反馈槽下标 → 源级 callsite 表达式渲染。只有语义层挂了 `callsite` 的
/// `Call`/`ConstructCall` 站点有条目；宿主拒绝路径（callee 非 callable /
/// 非构造器）按 `(image, slot)` 查文案。编号必须与
/// [`allocate_feedback_slots`] 完全一致（同一遍历序、同一槽候选判定），
/// 故实现共享 `instruction_owns_feedback_slot` 并保持相同的循环结构。
pub(crate) fn callsites_by_feedback_slot(program: &Program) -> HashMap<u32, Box<str>> {
    let mut callsites = HashMap::new();
    let mut slot_index = 0_u32;
    for function in program.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if !instruction_owns_feedback_slot(instruction) {
                    continue;
                }
                if let Instruction::Call {
                    callsite: Some(text),
                    ..
                }
                | Instruction::ConstructCall {
                    callsite: Some(text),
                    ..
                } = instruction
                {
                    callsites.insert(slot_index, text.clone());
                }
                slot_index += 1;
            }
        }
    }
    callsites
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{CompareOp, Instruction, UnaryOp, ValueId};

    fn f64_set(ids: &[u32]) -> HashSet<ValueId> {
        ids.iter().copied().map(ValueId).collect()
    }

    #[test]
    fn proven_f64_arith_and_locals_skip_resume_pad() {
        let f64s = f64_set(&[0, 1, 2]);
        assert!(!is_resume_target(
            &Instruction::Binary {
                dest: ValueId(0),
                op: BinaryOp::Add,
                lhs: ValueId(1),
                rhs: ValueId(2),
            },
            &f64s
        ));
        assert!(!is_resume_target(
            &Instruction::LoadVar {
                dest: ValueId(0),
                name: "$1.i".into(),
            },
            &f64s
        ));
        assert!(!is_resume_target(
            &Instruction::StoreVar {
                name: "$1.s".into(),
                value: ValueId(0),
            },
            &f64s
        ));
        assert!(!is_resume_target(
            &Instruction::Compare {
                dest: ValueId(0),
                op: CompareOp::Lt,
                lhs: ValueId(1),
                rhs: ValueId(2),
            },
            &f64s
        ));
        assert!(!is_resume_target(
            &Instruction::Unary {
                dest: ValueId(0),
                op: UnaryOp::Neg,
                value: ValueId(1),
            },
            &f64s
        ));
    }

    #[test]
    fn dynamic_ops_and_guards_keep_resume_pad() {
        let empty = HashSet::new();
        let f64s = f64_set(&[0]);
        assert!(is_resume_target(
            &Instruction::Binary {
                dest: ValueId(0),
                op: BinaryOp::Add,
                lhs: ValueId(1),
                rhs: ValueId(2),
            },
            &empty
        ));
        assert!(is_resume_target(
            &Instruction::GuardTag {
                dest: ValueId(0),
                value: ValueId(1),
                tag: 1,
            },
            &f64s
        ));
        assert!(is_resume_target(
            &Instruction::GetElem {
                dest: ValueId(0),
                object: ValueId(1),
                index: ValueId(2),
                latch: None,
            },
            &f64s
        ));
    }
}
