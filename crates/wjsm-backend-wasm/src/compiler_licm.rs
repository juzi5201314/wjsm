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
use std::cmp::Reverse;
use std::collections::HashSet;
use wjsm_ir::{
    BasicBlock, BasicBlockId, Builtin, FunctionId, Instruction, Module, Terminator, ValueId,
};

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
        | Instruction::GuardSameFunction { dest, .. }
        | Instruction::EncodeException { dest, .. }
        | Instruction::ExceptionToObject { dest, .. } => Some(*dest),
        Instruction::CallBuiltin {
            dest: Some(dest), ..
        }
        | Instruction::Call {
            dest: Some(dest), ..
        }
        | Instruction::SuperCall {
            dest: Some(dest), ..
        }
        | Instruction::ConstructCall {
            dest: Some(dest), ..
        } => Some(*dest),
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

/// 预构建前驱表：`preds[b]` = 所有跳转到 `b` 的块索引（O(V+E) 一次构建）。
/// 反向可达（谁可到达某块）沿此表遍历即 O(V+E)，替代每轮全模块扫描的 O(V²)。
fn build_preds(blocks: &[BasicBlock]) -> Vec<Vec<usize>> {
    let mut preds = vec![Vec::new(); blocks.len()];
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
            if let Some(p) = preds.get_mut(t) {
                p.push(i);
            }
        }
    }
    preds
}

/// 预构建 `ValueId → (块, 指令)` 定义位置索引（SSA：每个 value 恰有一个定义）。
/// 供 `all_loop_invariant` 与变换阶段 O(1) 查定义，替代全模块线性扫描。
fn build_value_defs(blocks: &[BasicBlock]) -> std::collections::HashMap<ValueId, (usize, usize)> {
    let mut defs = std::collections::HashMap::with_capacity(
        blocks.iter().map(|b| b.instructions().len()).sum(),
    );
    for (bi, block) in blocks.iter().enumerate() {
        for (ii, ins) in block.instructions().iter().enumerate() {
            if let Some(dest) = instruction_dest(ins) {
                defs.insert(dest, (bi, ii));
            }
        }
    }
    defs
}

/// 计算自然循环体：从 header 正向可达 ∧ 反向可达 latch 的块（含 header/latch）。
/// `preds` 为预构建前驱表（见 [`build_preds`]），反向可达沿表遍历保持 O(V+E)。
fn compute_loop_body(
    blocks: &[BasicBlock],
    preds: &[Vec<usize>],
    header: usize,
    latch: usize,
) -> HashSet<usize> {
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
    // 反向可达（谁能到达 latch）：沿前驱表遍历，O(V+E)。
    let mut can_reach_latch = HashSet::new();
    let mut stack = vec![latch];
    while let Some(b) = stack.pop() {
        if !can_reach_latch.insert(b) {
            continue;
        }
        stack.extend(preds[b].iter().copied());
    }
    reachable.intersection(&can_reach_latch).copied().collect()
}

/// 参数是否循环不变：存在定义且 def 是 `Const`（常量值不随迭代变化，可随 call
/// 移动），或 def 块不在循环体内（循环外定义，preheader 可引用）。
/// `defs` 为预构建的 `ValueId → (块, 指令)` 定义位置索引（见 [`build_value_defs`]），
/// 将原本的全模块线性扫描降为 O(1) 查表。
fn all_loop_invariant(
    blocks: &[BasicBlock],
    body: &HashSet<usize>,
    defs: &std::collections::HashMap<ValueId, (usize, usize)>,
    values: impl Iterator<Item = ValueId>,
) -> bool {
    for v in values {
        let def = defs.get(&v);
        match def {
            None => return false, // 无定义（外来值）→ 保守不提升。
            Some((b, i)) => {
                if !body.contains(b) {
                    continue; // 循环外定义不变。
                }
                // 循环内定义：仅 `Const` 不变（常量值不随迭代变化）。
                let def_is_const = blocks
                    .get(*b)
                    .and_then(|bb| bb.instructions().get(*i))
                    .is_some_and(|ins| matches!(ins, Instruction::Const { .. }));
                if def_is_const {
                    continue;
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

/// builtin 是否为「确定纯」：不写持久状态、结果确定、不依赖时钟/RNG。
///
/// 白名单与 `builtin_may_throw` 的 f64 特例条件一致：仅已知 f64 实参的算术
/// builtin 可确定纯（无 ToNumeric/ToPrimitive/reentrant）。其余 builtin
/// （MathRandom/DateNow/ConsoleLog 等）一律视为非纯——若未来新增确定纯
/// builtin，需同步两处。
fn deterministic_pure_builtin(
    builtin: Builtin,
    f64: &F64Analysis,
    func_id: FunctionId,
    args: &[ValueId],
) -> bool {
    let all_f64 = |need: usize| {
        args.len() >= need
            && args
                .iter()
                .take(need)
                .all(|a| f64.value_known_f64(func_id, *a))
    };
    match builtin {
        Builtin::AbstractCompare if all_f64(2) => true,
        Builtin::F64Mod | Builtin::F64Exp if all_f64(2) => true,
        _ => false,
    }
}

/// 计算「可能写持久状态」函数表（false→true 单调不动点，上限 64 轮）。
///
/// 命中即 true：`StoreVar`（写变量）、非确定纯的 `CallBuiltin`、已知 callee
/// 传递其表值、unknown callee / `SuperCall` / 属性写 / 对象构造 / `Suspend`
/// （can_throw 已拒这些路径，此处为提升 gate 的双保险）。死异常块跳过
/// （与 `compute_can_throw` 同模式——折叠路径不构成真实状态写）。
fn compute_may_write_state(module: &Module, f64: &F64Analysis, gc: &GcAnalysis) -> Vec<bool> {
    let n = module.functions().len();
    let mut may_write = vec![false; n];
    for _ in 0..64 {
        let mut changed = false;
        for (fidx, f) in module.functions().iter().enumerate() {
            if may_write[fidx] {
                continue;
            }
            let func_id = FunctionId(fidx as u32);
            let mut writes = false;
            for (bidx, bb) in f.blocks().iter().enumerate() {
                if f64.is_dead_exception_block(func_id, bidx) {
                    continue;
                }
                for ins in bb.instructions() {
                    match ins {
                        Instruction::StoreVar { .. } => {
                            writes = true;
                            break;
                        }
                        Instruction::CallBuiltin { builtin, args, .. } => {
                            if !deterministic_pure_builtin(*builtin, f64, func_id, args) {
                                writes = true;
                                break;
                            }
                        }
                        Instruction::Call { callee, .. }
                        | Instruction::ConstructCall { callee, .. }
                        | Instruction::OptionalCall { callee, .. } => {
                            match gc.direct_call_target(func_id, *callee) {
                                Some(g) => {
                                    if may_write[g.0 as usize] {
                                        writes = true;
                                        break;
                                    }
                                }
                                // Unknown callee：可能写状态。
                                None => {
                                    writes = true;
                                    break;
                                }
                            }
                        }
                        Instruction::SuperCall { .. }
                        | Instruction::SetProp { .. }
                        | Instruction::SetElem { .. }
                        | Instruction::SetProto { .. }
                        | Instruction::DeleteProp { .. }
                        | Instruction::NewObject { .. }
                        | Instruction::NewArray { .. }
                        | Instruction::Suspend { .. }
                        | Instruction::GeneratorSuspend { .. } => {
                            writes = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if writes {
                    break;
                }
            }
            if writes {
                may_write[fidx] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    may_write
}

/// 计算「可能读持久状态」函数表（false→true 单调不动点，上限 64 轮）。
///
/// 命中即 true：`LoadVar` 读模块/闭包变量（非形参、非 `$env`/`$this`/
/// `xxx.$env`）、`GetProp`/`GetElem` 系、已知 callee 传递其表值、unknown
/// callee / `SuperCall`。死异常块跳过。
fn compute_may_read_global(module: &Module, f64: &F64Analysis, gc: &GcAnalysis) -> Vec<bool> {
    let n = module.functions().len();
    let mut may_read = vec![false; n];
    for _ in 0..64 {
        let mut changed = false;
        for (fidx, f) in module.functions().iter().enumerate() {
            if may_read[fidx] {
                continue;
            }
            let func_id = FunctionId(fidx as u32);
            let params: Vec<&str> = f.params().iter().map(|s| s.as_str()).collect();
            let mut reads = false;
            for (bidx, bb) in f.blocks().iter().enumerate() {
                if f64.is_dead_exception_block(func_id, bidx) {
                    continue;
                }
                for ins in bb.instructions() {
                    match ins {
                        Instruction::LoadVar { name, .. } => {
                            let s = name.as_str();
                            // 形参与环境/this 句柄不构成持久状态读。
                            if !params.contains(&s)
                                && s != "$env"
                                && s != "$this"
                                && !s.ends_with(".$env")
                            {
                                reads = true;
                                break;
                            }
                        }
                        Instruction::GetProp { .. }
                        | Instruction::GetElem { .. }
                        | Instruction::OptionalGetProp { .. }
                        | Instruction::OptionalGetElem { .. } => {
                            reads = true;
                            break;
                        }
                        Instruction::Call { callee, .. }
                        | Instruction::ConstructCall { callee, .. }
                        | Instruction::OptionalCall { callee, .. } => {
                            match gc.direct_call_target(func_id, *callee) {
                                Some(g) => {
                                    if may_read[g.0 as usize] {
                                        reads = true;
                                        break;
                                    }
                                }
                                // Unknown callee：可能读全局状态。
                                None => {
                                    reads = true;
                                    break;
                                }
                            }
                        }
                        Instruction::SuperCall { .. } => {
                            reads = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if reads {
                    break;
                }
            }
            if reads {
                may_read[fidx] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    may_read
}

/// 循环不变量纯调用提升主入口。
///
/// 遍历所有函数（模块入口与 JS 函数）的自然循环，提升循环体内满足
/// `!may_gc ∧ !can_throw ∧ 两表均 false ∧ 参数循环不变` 的直接调用。调用方
/// 须在变换前先跑 F64Analysis/GcAnalysis（本 pass 的 gate 消费其结果）；变换
/// 后应重新分析（preheader 的调用结果可能被循环内的 is_exception 消费）。
///
/// 返回每个函数被插入的 preheader 块索引（供 structured 编译在循环头前
/// 提前发射其指令）；无提升的函数不在返回中。
pub(crate) fn hoist_loop_invariant_pure_calls(
    module: &mut Module,
    f64: &F64Analysis,
    gc: &GcAnalysis,
) -> Vec<(FunctionId, HashSet<usize>)> {
    // A1：模块级「可能写/读持久状态」表（false→true 单调不动点）。
    let may_write_state = compute_may_write_state(module, f64, gc);
    let may_read_global = compute_may_read_global(module, f64, gc);

    let mut hoisted: Vec<(FunctionId, HashSet<usize>)> = Vec::new();
    for func_idx in 0..module.functions().len() {
        let func_id = FunctionId(func_idx as u32);
        let mut preheaders: HashSet<usize> = HashSet::new();

        // 收集阶段（只读快照）。
        let mut plans: Vec<HoistPlan> = Vec::new();
        {
            let function = &module.functions()[func_idx];
            let blocks = function.blocks();
            // 预构建前驱表与定义索引：把循环体计算与循环不变判定从 O(V²)/
            // 线性扫描降为 O(V+E)/O(1)（见 issue #342）。
            let preds = build_preds(blocks);
            let defs = build_value_defs(blocks);
            for (latch, header) in find_back_edges(blocks) {
                // C1：循环头含 Phi → 跳过该循环（phi 边在 preheader 重定向后
                // 需额外的值更新逻辑，泛化阶段不做）。模块入口循环头无 Phi。
                if blocks[header]
                    .instructions()
                    .iter()
                    .any(|ins| matches!(ins, Instruction::Phi { .. }))
                {
                    continue;
                }
                let body = compute_loop_body(blocks, &preds, header, latch);
                for &b in &body {
                    let block = &blocks[b];
                    for (i, ins) in block.instructions().iter().enumerate() {
                        let Instruction::Call {
                            callee,
                            this_val,
                            args,
                            ..
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
                        // A2：拒绝可能写/读持久状态的 callee——提升会改变其执行
                        // 次数（从每迭代一次变为每入口一次），破坏 ECMAScript 语义。
                        if may_write_state[callee_fn.0 as usize]
                            || may_read_global[callee_fn.0 as usize]
                        {
                            continue;
                        }
                        if !all_loop_invariant(
                            blocks,
                            &body,
                            &defs,
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
                    callee,
                    this_val,
                    args,
                    ..
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
                    // 只在循环体内查找 Const 定义（SSA 唯一性：定义要么在循环体内
                    // 要么在循环外；循环外定义保持原位，无需扫描全模块——与原实现
                    // 全模块扫描后 `!plan.body.contains` 即 break 的语义完全等价）。
                    for &bi in &plan.body {
                        if let Some(b) = function.blocks_mut().get_mut(bi)
                            && let Some(pos) = b.instructions_mut().iter().position(
                                |ins| matches!(ins, Instruction::Const { dest, .. } if *dest == v),
                            )
                        {
                            let def = b.instructions_mut().remove(pos);
                            moved.insert(v);
                            to_move.push(def);
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
            retarget_external_preds(function.blocks_mut(), &plan.body, plan.header, preheader_id);
            function.push_block(preheader);
            // 记录新 preheader 的块索引（append 后即 blocks 末位）。
            preheaders.insert(function.blocks().len() - 1);
        }
        if !preheaders.is_empty() {
            hoisted.push((func_id, preheaders));
        }
    }
    hoisted
}
