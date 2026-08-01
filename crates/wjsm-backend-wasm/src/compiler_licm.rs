//! 循环不变量纯调用提升（wjsm 侧 LICM）。
//!
//! Cranelift 将 `call` 硬编码为有副作用（`trivially_has_side_effects` 里
//! `opcode.is_call() → true`），egraph LICM 只提升 pure 节点，因此 wasm 的
//! call 永远不会被 Cranelift 移出循环。而模块入口的 while 循环每迭代调用
//! `work()`（纯函数：can_throw=false、may_gc=false、参数全部常量）——
//! 该调用是循环不变量，可在 IR 层安全提升到循环头前的 preheader 执行一次。
//!
//! 变换（对每个模块入口函数的自然循环）：
//!   1. 计算循环体（从 header 正向可达 ∧ 反向可达 latch 的块）。
//!   2. 扫描循环体内 `Call`：callee 已知（direct_call_target）且
//!      `!may_gc(callee)` ∧ `!can_throw(callee)` ∧ 参数全部循环不变
//!      （Const 或定义在循环外）。
//!   3. 新建 preheader 块（append 到末尾，id = blocks.len()），把 call 及其
//!      循环体内的常量参数定义移动进去；header 的非回边前驱重定向到 preheader。
//!   4. 循环体内的 call 消失（结果 ValueId 的定义移到 preheader，SSA 仍合法：
//!      preheader 支配整个循环体）。
//!
//! 纯调用无副作用 → preheader 执行一次是合法变换（即使循环 0 次进入，多执行
//! 一次也不可观察）。调用结果被丢弃时 preheader 的调用仍保留（LICM 契约：
//! 每入口执行一次），不额外做 DCE。

use crate::analysis_f64::F64Analysis;
use crate::compiler_gc_analysis::GcAnalysis;
use crate::is_module_entry_ir_function;
use std::cmp::Reverse;
use std::collections::HashSet;
use wjsm_ir::{BasicBlock, BasicBlockId, FunctionId, Instruction, Module, Terminator, ValueId};

/// 指令的 dest（与 wjsm-ir verify 的 instruction_dest 相同的完整匹配）。
fn instruction_dest(ins: &Instruction) -> Option<ValueId> {
    match ins {
        Instruction::Const { dest, .. }
        | Instruction::Binary { dest, .. }
        | Instruction::Unary { dest, .. }
        | Instruction::Compare { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::StringConcatVa { dest, .. }
        | Instruction::LoadVar { dest, .. }
        | Instruction::NewObject { dest, .. }
        | Instruction::GetProp { dest, .. }
        | Instruction::DeleteProp { dest, .. }
        | Instruction::NewArray { dest, .. }
        | Instruction::GetElem { dest, .. }
        | Instruction::OptionalGetProp { dest, .. }
        | Instruction::OptionalGetElem { dest, .. }
        | Instruction::OptionalCall { dest, .. }
        | Instruction::ObjectSpread { dest, .. }
        | Instruction::GetSuperBase { dest }
        | Instruction::GetSuperConstructor { dest }
        | Instruction::NewPromise { dest }
        | Instruction::CollectRestArgs { dest, .. }
        | Instruction::IsException { dest, .. }
        | Instruction::EncodeException { dest, .. }
        | Instruction::ExceptionToObject { dest, .. } => Some(*dest),
        Instruction::CallBuiltin { dest: Some(dest), .. }
        | Instruction::Call { dest: Some(dest), .. }
        | Instruction::SuperCall { dest: Some(dest), .. }
        | Instruction::ConstructCall { dest: Some(dest), .. } => Some(*dest),
        _ => None,
    }
}

/// 查找所有回边（latch → header，target 序号 ≤ 源块）。
fn find_back_edges(blocks: &[BasicBlock]) -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let mut targets = Vec::new();
        match block.terminator() {
            Terminator::Jump { target } => targets.push(target.0 as usize),
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                targets.push(true_block.0 as usize);
                targets.push(false_block.0 as usize);
            }
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                targets.extend(cases.iter().map(|case| case.target.0 as usize));
                targets.push(default_block.0 as usize);
                targets.push(exit_block.0 as usize);
            }
            _ => {}
        }
        for t in targets {
            if t <= i {
                edges.push((i, t));
            }
        }
    }
    edges
}

/// 计算自然循环体：从 header 正向可达 ∧ 反向可达 latch 的块（含 header/latch）。
fn compute_loop_body(blocks: &[BasicBlock], header: usize, latch: usize) -> HashSet<usize> {
    // 正向可达（从 header 出发，沿所有 CFG 边）。
    let mut reachable = HashSet::new();
    let mut stack = vec![header];
    while let Some(b) = stack.pop() {
        if !reachable.insert(b) {
            continue;
        }
        match blocks[b].terminator() {
            Terminator::Jump { target } => stack.push(target.0 as usize),
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                stack.push(true_block.0 as usize);
                stack.push(false_block.0 as usize);
            }
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                stack.extend(cases.iter().map(|case| case.target.0 as usize));
                stack.push(default_block.0 as usize);
                stack.push(exit_block.0 as usize);
            }
            _ => {}
        }
    }
    // 反向可达（谁能到达 latch）。
    let mut can_reach_latch = HashSet::new();
    let mut stack = vec![latch];
    while let Some(b) = stack.pop() {
        if !can_reach_latch.insert(b) {
            continue;
        }
        for (p, block) in blocks.iter().enumerate() {
            let targets_here = match block.terminator() {
                Terminator::Jump { target } => vec![target.0 as usize],
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => vec![true_block.0 as usize, false_block.0 as usize],
                Terminator::Switch {
                    cases,
                    default_block,
                    exit_block,
                    ..
                } => {
                    let mut t: Vec<usize> =
                        cases.iter().map(|case| case.target.0 as usize).collect();
                    t.push(default_block.0 as usize);
                    t.push(exit_block.0 as usize);
                    t
                }
                _ => Vec::new(),
            };
            if targets_here.contains(&b) {
                stack.push(p);
            }
        }
    }
    reachable.intersection(&can_reach_latch).copied().collect()
}

/// 参数是否循环不变：存在定义且 def 是 `Const`（常量值不随迭代变化，可随 call
/// 移动），或 def 块不在循环体内（循环外定义，preheader 可引用）。
fn all_loop_invariant(
    blocks: &[BasicBlock],
    body: &HashSet<usize>,
    values: impl Iterator<Item = ValueId>,
) -> bool {
    for v in values {
        let def = blocks.iter().enumerate().find_map(|(b, block)| {
            block
                .instructions()
                .iter()
                .find(|ins| instruction_dest(ins) == Some(v))
                .map(|ins| (b, ins))
        });
        match def {
            None => return false, // 无定义（外来值）→ 保守不提升。
            Some((b, ins)) => {
                if matches!(ins, Instruction::Const { .. }) {
                    continue; // 常量不变。
                }
                if !body.contains(&b) {
                    continue; // 循环外定义不变。
                }
                return false; // 循环内非 Const → 参数随迭代变化。
            }
        }
    }
    true
}

/// 重定向所有「目标为 header 且源块不在循环体内」的边到 preheader。
/// `body` 必须是变换前（原快照）的循环体集合。
fn retarget_external_preds(
    blocks: &mut [BasicBlock],
    body: &HashSet<usize>,
    header: usize,
    preheader: BasicBlockId,
) {
    for (p, block) in blocks.iter_mut().enumerate() {
        if body.contains(&p) {
            continue; // 回边（循环体内）保留原目标。
        }
        match block.terminator_mut() {
            Terminator::Jump { target } if target.0 as usize == header => {
                *target = preheader;
            }
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                if true_block.0 as usize == header {
                    *true_block = preheader;
                }
                if false_block.0 as usize == header {
                    *false_block = preheader;
                }
            }
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                for case in cases.iter_mut() {
                    if case.target.0 as usize == header {
                        case.target = preheader;
                    }
                }
                if default_block.0 as usize == header {
                    *default_block = preheader;
                }
                if exit_block.0 as usize == header {
                    *exit_block = preheader;
                }
            }
            _ => {}
        }
    }
}

/// 单个提升计划：循环头、调用所在块与指令位置、以及原快照的循环体集合。
struct HoistPlan {
    header: usize,
    block: usize,
    instr: usize,
    /// 变换前的循环体（用于判断 pred 是否为回边、参数 def 是否在循环内）。
    body: HashSet<usize>,
}

/// 循环不变量纯调用提升主入口。
///
/// 只处理模块入口函数（其 while 循环走 cfg_dispatch 编译路径）。调用方须在
/// 变换前先跑 F64Analysis/GcAnalysis（本 pass 的 gate 消费其结果）；变换后
/// 应重新分析（preheader 的调用结果可能被循环内的 is_exception 消费）。
pub(crate) fn hoist_loop_invariant_pure_calls(
    module: &mut Module,
    f64: &F64Analysis,
    gc: &GcAnalysis,
) {
    for func_idx in 0..module.functions().len() {
        let func_id = FunctionId(func_idx as u32);
        if !is_module_entry_ir_function(module.functions()[func_idx].name()) {
            continue;
        }

        // 收集阶段（只读快照）。
        let mut plans: Vec<HoistPlan> = Vec::new();
        {
            let function = &module.functions()[func_idx];
            let blocks = function.blocks();
            for (latch, header) in find_back_edges(blocks) {
                let body = compute_loop_body(blocks, header, latch);
                for &b in &body {
                    let block = &blocks[b];
                    for (i, ins) in block.instructions().iter().enumerate() {
                        let Instruction::Call {
                            callee, this_val, args, ..
                        } = ins
                        else {
                            continue;
                        };
                        let Some(callee_fn) = gc.direct_call_target(func_id, *callee) else {
                            continue;
                        };
                        if gc.function_may_gc(callee_fn) || f64.function_can_throw(callee_fn) {
                            continue;
                        }
                        if !all_loop_invariant(
                            blocks,
                            &body,
                            std::iter::once(*callee)
                                .chain(std::iter::once(*this_val))
                                .chain(args.iter().copied()),
                        ) {
                            continue;
                        }
                        plans.push(HoistPlan {
                            header,
                            block: b,
                            instr: i,
                            body: body.clone(),
                        });
                    }
                }
            }
        }

        if plans.is_empty() {
            continue;
        }

        // 同一块内多个候选按指令序号降序处理（remove 不影响更早的候选），
        // 并对 (block, instr) 去重——重叠循环（如 warmup 的内/外环）会把同一
        // 个 call 收集多次，只保留第一个（提升到最外层 preheader）。
        plans.sort_by_key(|plan| (plan.block, Reverse(plan.instr)));
        plans.dedup_by(|a, b| a.block == b.block && a.instr == b.instr);

        // 变换阶段（可变）。
        let Some(function) = module.function_mut(func_id) else {
            continue;
        };
        for plan in plans {
            let header_id = BasicBlockId(plan.header as u32);
            let preheader_id = BasicBlockId(function.blocks().len() as u32);
            let mut preheader = BasicBlock::new_with_terminator(
                preheader_id,
                Terminator::Jump { target: header_id },
            );

            // 移动 call 及其循环体内定义的常量参数（callee/this_val/args 的
            // def 若在原循环体内且为 Const，一并移到 preheader，保证 SSA 合法；
            // 循环外定义保持原位，preheader 直接引用）。
            let mut to_move: Vec<Instruction> = Vec::new();
            {
                let call = function.blocks_mut()[plan.block]
                    .instructions_mut()
                    .remove(plan.instr);
                let mut deps: Vec<ValueId> = Vec::new();
                if let Instruction::Call {
                    callee, this_val, args, ..
                } = &call
                {
                    deps.push(*callee);
                    deps.push(*this_val);
                    deps.extend(args.iter().copied());
                }
                let mut moved: HashSet<ValueId> = HashSet::new();
                for v in deps {
                    if moved.contains(&v) {
                        continue;
                    }
                    for (bi, b) in function.blocks_mut().iter_mut().enumerate() {
                        if let Some(pos) = b.instructions_mut().iter().position(|ins| {
                            matches!(ins, Instruction::Const { dest, .. } if *dest == v)
                        }) {
                            // 只在循环体内的 Const 定义随 call 移动；循环外的
                            // （或已被前一个候选移到 preheader 的）保持原位。
                            if plan.body.contains(&bi) {
                                let def = b.instructions_mut().remove(pos);
                                moved.insert(v);
                                to_move.push(def);
                            }
                            break;
                        }
                    }
                }
                to_move.push(call);
            }

            // preheader 指令顺序：参数定义在前，call 在后。
            for ins in to_move {
                preheader.push_instruction(ins);
            }

            // 先重定向（blocks 里还没有新 preheader，避免 preheader 的
            // Jump(header) 被误改成 Jump(自己) 形成自环），再追加 preheader。
            retarget_external_preds(
                function.blocks_mut(),
                &plan.body,
                plan.header,
                preheader_id,
            );
            function.push_block(preheader);
        }
    }
}
