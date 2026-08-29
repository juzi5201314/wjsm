//! 反馈槽与 resume 目标判定：编号必须与 generic / overlay 一致。

#![allow(unused_imports)]
use std::collections::HashMap;

use wjsm_ir::{BasicBlockId, Instruction, Program};

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

/// 判定一条指令是否是「可观察动态语义」的反馈槽候选。
///
/// 只看指令形态、不看 `infer_f64_values` 的结论：静态证明会随特化种子变化，
/// 若槽编号依赖分析结果，base 与 overlay 的编号就会错位。
pub(crate) fn is_resume_target(instruction: &Instruction) -> bool {
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
