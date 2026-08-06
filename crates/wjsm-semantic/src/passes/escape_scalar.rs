//! 逃逸分析 + 标量替换（Escape Analysis + Scalar Replacement）pass。
//!
//! 识别函数内局部 `NewObject` 对象，如果对象不逃逸（所有使用都是 SetProp/GetProp
//! 常量字符串键、SetProto、CallBuiltin(IsJsObject)），则：
//!
//! 1. 把 GetProp 读取结果的使用处替换为 SetProp 写入的值；
//! 2. 删除 NewObject、SetProp、SetProto、CallBuiltin(IsJsObject) 指令。
//!
//! 保守策略：任何非上述模式的使用（Call、Return、StoreVar、Phi、LoadVar 等）→ 逃逸。

use std::collections::{HashMap, HashSet};
use wjsm_ir::{
    Constant, FunctionId, Instruction, Module, Terminator, ValueId, BasicBlockId,
};
use super::cfg_fold::terminator_successors;
use super::direct_call::{instr_uses, terminator_uses, collect_uses};

/// 计算函数的支配集（迭代数据流，O(n²) 可接受，块数 ≤512）。
///
/// `dom[b]` = 支配块 b 的块集合（含 b 自身）。入口块的支配集为 {entry}。
fn compute_dominators(function: &wjsm_ir::Function) -> Vec<HashSet<BasicBlockId>> {
    let n = function.blocks().len();
    let all: HashSet<BasicBlockId> = (0..n as u32).map(BasicBlockId).collect();
    let mut dom: Vec<HashSet<BasicBlockId>> = vec![all; n];
    let entry = function.entry();
    dom[entry.0 as usize] = HashSet::from([entry]);

    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..n {
            if b == entry.0 as usize {
                continue;
            }
            // 前驱集合。
            let preds: Vec<usize> = (0..n)
                .filter(|&p| {
                    terminator_successors(function.blocks()[p].terminator())
                        .contains(&BasicBlockId(b as u32))
                })
                .collect();
            if preds.is_empty() {
                continue;
            }
            let mut new_dom = dom[preds[0]].clone();
            for &p in &preds[1..] {
                new_dom.retain(|x| dom[p].contains(x));
            }
            new_dom.insert(BasicBlockId(b as u32));
            if new_dom != dom[b] {
                dom[b] = new_dom;
                changed = true;
            }
        }
    }
    dom
}

/// 判定变量 `name` 是否可转发：StoreVar 恰一次，且所有 LoadVar 位于 store 之后
/// （同块时指令索引更大；跨块时 store 块支配 load 块）。
fn can_forward_var(
    function: &wjsm_ir::Function,
    name: &str,
    store_block: BasicBlockId,
    store_idx: usize,
    dom: &[HashSet<BasicBlockId>],
) -> bool {
    for block in function.blocks() {
        for (idx, ins) in block.instructions().iter().enumerate() {
            match ins {
                Instruction::StoreVar { name: n, .. } if n == name => {
                    if block.id() != store_block || idx != store_idx {
                        return false;
                    }
                }
                Instruction::LoadVar { name: n, .. } if n == name => {
                    if block.id() == store_block {
                        if idx <= store_idx {
                            return false;
                        }
                    } else if !dom[block.id().0 as usize].contains(&store_block) {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// 查找函数内唯一的 `StoreVar(name)` 位置。
fn find_store_pos(
    function: &wjsm_ir::Function,
    name: &str,
) -> Option<(BasicBlockId, usize)> {
    for block in function.blocks() {
        for (idx, ins) in block.instructions().iter().enumerate() {
            if let Instruction::StoreVar { name: n, .. } = ins {
                if n == name {
                    return Some((block.id(), idx));
                }
            }
        }
    }
    None
}

/// 收集函数内所有 `LoadVar(name)` 的位置与 dest。
fn load_var_positions(
    function: &wjsm_ir::Function,
    name: &str,
) -> Vec<(BasicBlockId, usize, ValueId)> {
    let mut out = Vec::new();
    for block in function.blocks() {
        for (idx, ins) in block.instructions().iter().enumerate() {
            if let Instruction::LoadVar { dest, name: n } = ins {
                if n == name {
                    out.push((block.id(), idx, *dest));
                }
            }
        }
    }
    out
}

/// 分析候选对象的所有 use（含转发变量的代理读）。
///
/// 返回 (slot_assignments, slot_reads, escapes, forwarded_var)：
/// - `slot_assignments`：SetProp 常量键（字符串）→ 写入值；
/// - `slot_reads`：GetProp 常量键（直接读候选 / 代理读 LoadVar dest）→ 读取 dest；
/// - `escapes`：存在非白名单使用（Call/Return/Phi 等）；
/// - `forwarded_var`：候选经唯一 `StoreVar(name)` 转发（所有 LoadVar 位于 store 之后）。
fn analyze_candidate(
    function: &wjsm_ir::Function,
    candidate_dest: ValueId,
    const_strings: &HashMap<ValueId, String>,
    dom: &[HashSet<BasicBlockId>],
) -> (
    Vec<(String, ValueId)>,
    Vec<(String, ValueId)>,
    bool,
    Option<String>,
) {
    let uses = collect_uses(function, candidate_dest);
    let mut escapes = false;
    // 槽位按字符串 key 记录（内联克隆后同一属性名的 SetProp/GetProp 常量
    // ValueId 不同，按 ValueId 匹配会失效）。
    let mut slot_assignments: Vec<(String, ValueId)> = Vec::new();
    let mut slot_reads: Vec<(String, ValueId)> = Vec::new();
    let mut forwarded_var: Option<String> = None;

    for use_instr in &uses {
        match use_instr {
            Instruction::SetProp { object, key, value }
                if *object == candidate_dest =>
            {
                if let Some(s) = const_strings.get(key) {
                    slot_assignments.push((s.clone(), *value));
                } else {
                    escapes = true;
                }
            }
            Instruction::GetProp { dest, object, key }
                if *object == candidate_dest =>
            {
                if let Some(s) = const_strings.get(key) {
                    slot_reads.push((s.clone(), *dest));
                } else {
                    escapes = true;
                }
            }
            Instruction::SetProto { object, .. } if *object == candidate_dest => {}
            Instruction::CallBuiltin { builtin, .. }
                if *builtin == wjsm_ir::Builtin::IsJsObject => {}
            Instruction::StoreVar { name, value } if *value == candidate_dest => {
                // 变量转发：唯一 StoreVar + 所有 LoadVar 位于 store 之后 → 可转发。
                if let Some((sb, si)) = find_store_pos(function, name)
                    && can_forward_var(function, name, sb, si, dom)
                {
                    forwarded_var = Some(name.clone());
                } else {
                    escapes = true;
                }
            }
            _ => {
                escapes = true;
            }
        }
    }

    // 代理读：转发变量的 LoadVar dest 的 use 按 GetProp 常量键分类。
    if !escapes && let Some(var) = &forwarded_var {
        for (_, _, load_dest) in load_var_positions(function, var) {
            // `collect_uses` 只扫指令，不含终止器；`return o` / `throw o` /
            // `if (o)` 里的 LoadVar dest 必须单独判定为逃逸，否则删掉
            // NewObject/StoreVar/LoadVar 之后终止器会引用一个已不存在的 ValueId，
            // 产出悬空 SSA（后端按未定义 local 发射，类型随 local 布局漂移）。
            if used_in_terminator(function, load_dest) {
                escapes = true;
                break;
            }
            for use_instr in collect_uses(function, load_dest) {
                match use_instr {
                    Instruction::GetProp { dest, object, key }
                        if *object == load_dest =>
                    {
                        if let Some(s) = const_strings.get(key) {
                            slot_reads.push((s.clone(), *dest));
                        } else {
                            escapes = true;
                        }
                    }
                    _ => {
                        escapes = true;
                    }
                }
            }
        }
    }

    (slot_assignments, slot_reads, escapes, forwarded_var)
}
pub(crate) fn run(module: &mut Module) {
    // Eval 守卫：直接 eval 可以动态读写任意变量，保守禁用
    if module.functions().iter().any(|f| f.has_eval()) {
        return;
    }

    // 迭代直到无变化（一个替换可能创造新的候选）
    let mut any_change = true;
    while any_change {
        any_change = false;

        let func_count = module.functions().len();
        for fid in 0..func_count {
            let func_id = FunctionId(fid as u32);

            // ── Phase 1: 只读分析 ──
            // 先捕获常量表引用，避免与后续 module.function_mut() 冲突
            let constants_base: Vec<Constant> = module.constants().to_vec();

            let (needs_replacement, _) = {
                let function = &module.functions()[fid];

                // 构建常量字符串表与支配集。
                let mut const_strings: HashMap<ValueId, String> = HashMap::new();
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Instruction::Const { dest, constant } = instruction {
                            if let Some(Constant::String(s)) = constants_base.get(constant.0 as usize) {
                                const_strings.insert(*dest, s.clone());
                            }
                        }
                    }
                }
                let dom = compute_dominators(function);

                // 收集 NewObject 候选
                let mut candidates: Vec<(ValueId, u32, BasicBlockId)> = Vec::new();
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Instruction::NewObject { dest, capacity } = instruction {
                            candidates.push((*dest, *capacity, block.id()));
                        }
                    }
                }

                // 分析每个候选是否逃逸
                let mut needs_replacement: Vec<(ValueId, BasicBlockId, Option<String>)> =
                    Vec::new();

                for (candidate_dest, _capacity, _def_block) in &candidates {
                    // 检查终止器是否使用 candidate（Return、Branch 等）
                    if used_in_terminator(function, *candidate_dest) {
                        continue;
                    }

                    let (slot_assignments, slot_reads, escapes, forwarded_var) =
                        analyze_candidate(function, *candidate_dest, &const_strings, &dom);

                    if !escapes && !slot_assignments.is_empty() {
                        let all_reads_have_assignments = slot_reads.iter().all(|(read_key, _)| {
                            slot_assignments.iter().any(|(k, _)| k == read_key)
                        });

                        if all_reads_have_assignments {
                            needs_replacement
                                .push((*candidate_dest, *_def_block, forwarded_var));
                        }
                    }
                }

                (needs_replacement, const_strings)
            };

            if needs_replacement.is_empty() {
                continue;
            }

            // ── Phase 2: 应用替换 ──
            let function = module.function_mut(func_id).expect("function id must be valid");

            // 重建常量字符串表
            let mut const_strings: HashMap<ValueId, String> = HashMap::new();
            for block in function.blocks() {
                for instruction in block.instructions() {
                    if let Instruction::Const { dest, constant } = instruction {
                        if let Some(Constant::String(s)) = constants_base.get(constant.0 as usize) {
                            const_strings.insert(*dest, s.clone());
                        }
                    }
                }
            }

            // 分析每个候选并构建替换映射
            let dom = compute_dominators(function);
            let mut all_replacements: HashMap<ValueId, ValueId> = HashMap::new();
            let mut delete_targets: Vec<(BasicBlockId, usize)> = Vec::new();

            for (candidate_dest, _def_block, forwarded_var) in &needs_replacement {
                let (slot_assignments, slot_reads, escapes, _) =
                    analyze_candidate(function, *candidate_dest, &const_strings, &dom);

                if escapes {
                    continue;
                }

                // 构建替换映射：GetProp dest → SetProp value
                let assignment_map: HashMap<String, ValueId> =
                    slot_assignments.into_iter().collect();
                for (read_key, read_dest) in &slot_reads {
                    if let Some(assigned_value) = assignment_map.get(read_key) {
                        all_replacements.insert(*read_dest, *assigned_value);
                    }
                }

                // 记录 NewObject 指令删除
                for block in function.blocks() {
                    for (idx, instruction) in block.instructions().iter().enumerate() {
                        if let Instruction::NewObject { dest, .. } = instruction {
                            if *dest == *candidate_dest {
                                delete_targets.push((block.id(), idx));
                                any_change = true;
                            }
                        }
                    }
                }

                // 记录转发变量的 StoreVar/LoadVar 指令删除（代理读替换后变死）。
                if let Some(var) = forwarded_var {
                    for block in function.blocks() {
                        for (idx, instruction) in block.instructions().iter().enumerate() {
                            match instruction {
                                Instruction::StoreVar { name, .. }
                                | Instruction::LoadVar { name, .. }
                                    if name == var =>
                                {
                                    delete_targets.push((block.id(), idx));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // 记录 SetProp/SetProto/CallBuiltin 指令删除
                for block in function.blocks() {
                    for (idx, instruction) in block.instructions().iter().enumerate() {
                        match instruction {
                            Instruction::SetProp { object, .. }
                                if *object == *candidate_dest =>
                            {
                                delete_targets.push((block.id(), idx));
                            }
                            Instruction::SetProto { object, .. }
                                if *object == *candidate_dest =>
                            {
                                delete_targets.push((block.id(), idx));
                            }
                            Instruction::CallBuiltin { builtin, .. }
                                if *builtin == wjsm_ir::Builtin::IsJsObject =>
                            {
                                let instr_vals = instr_uses(instruction);
                                if instr_vals.contains(candidate_dest) {
                                    delete_targets.push((block.id(), idx));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            // 标量化的 GetProp 指令：dest 已被替换成常量，但指令本身未被删除。
            // 这使其 object 操作数（已删除的 NewObject dest）成为悬空引用，
            // wasm 后端将其映射到 local0（零值），导致 ic_backfill 和 obj_get
            // 每次都收到空对象，IC 永久退化为 MEGAMORPHIC。
            // 修复：凡 dest 在 all_replacements 中的 GetProp 一律加入删除列表。
            if !all_replacements.is_empty() {
                for block in function.blocks() {
                    for (idx, instruction) in block.instructions().iter().enumerate() {
                        if let Instruction::GetProp { dest, .. } = instruction {
                            if all_replacements.contains_key(dest) {
                                delete_targets.push((block.id(), idx));
                            }
                        }
                    }
                }
            }

            // 应用替换：遍历所有指令，替换 ValueId
            let change = apply_value_replacements(function, &all_replacements);
            any_change = any_change || change;

            // 删除指令（从后往前删，避免索引偏移）
            if !delete_targets.is_empty() {
                let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
                for (block_id, idx) in &delete_targets {
                    by_block.entry(*block_id).or_default().push(*idx);
                }
                for (block_id, mut indices) in by_block {
                    indices.sort_unstable_by(|a, b| b.cmp(a));
                    indices.dedup();
                    if let Some(block) = function.block_by_id_mut(block_id) {
                        let instrs = block.instructions_mut();
                        for idx in indices {
                            if idx < instrs.len() {
                                instrs.remove(idx);
                            }
                        }
                    }
                }
                any_change = true;
            }
        }
    }
}

/// 检查 ValueId 是否被任何 block 的终止器使用（逃逸）。
fn used_in_terminator(function: &wjsm_ir::Function, target: ValueId) -> bool {
    for block in function.blocks() {
        if terminator_uses(block.terminator()).contains(&target) {
            return true;
        }
    }
    false
}

/// 在函数的所有指令和终止器中替换 ValueId。
fn apply_value_replacements(
    function: &mut wjsm_ir::Function,
    replacements: &HashMap<ValueId, ValueId>,
) -> bool {
    if replacements.is_empty() {
        return false;
    }

    let mut changed = false;

    for block in function.blocks_mut() {
        for instruction in block.instructions_mut() {
            if replace_in_instruction(instruction, replacements) {
                changed = true;
            }
        }

        replace_in_terminator(block.terminator_mut(), replacements);
    }

    changed
}

fn replace_in_instruction(
    ins: &mut Instruction,
    replacements: &HashMap<ValueId, ValueId>,
) -> bool {
    let mut changed = false;

    match ins {
        Instruction::Const { .. }
        | Instruction::LoadVar { .. }
        | Instruction::NewObject { .. }
        | Instruction::NewArray { .. }
        | Instruction::GetSuperBase { .. }
        | Instruction::GetSuperConstructor { .. }
        | Instruction::NewPromise { .. }
        | Instruction::CollectRestArgs { .. }
        | Instruction::DebugCheck { .. } => {}
        Instruction::Binary { lhs, rhs, .. }
        | Instruction::Compare { lhs, rhs, .. } => {
            if let Some(new) = replacements.get(lhs) { *lhs = *new; changed = true; }
            if let Some(new) = replacements.get(rhs) { *rhs = *new; changed = true; }
        }
        Instruction::Unary { value, .. } => {
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::StringConcatVa { parts, .. } => {
            for part in parts.iter_mut() {
                if let Some(new) = replacements.get(part) { *part = *new; changed = true; }
            }
        }
        Instruction::GetProp { object, key, .. } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(key) { *key = *new; changed = true; }
        }
        Instruction::SetProp { object, key, value } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(key) { *key = *new; changed = true; }
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::SetProto { object, value } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::GetElem { object, index, .. } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(index) { *index = *new; changed = true; }
        }
        Instruction::SetElem { object, index, value, .. } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(index) { *index = *new; changed = true; }
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::OptionalGetProp { object, key, .. }
        | Instruction::OptionalGetElem { object, key, .. } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(key) { *key = *new; changed = true; }
        }
        Instruction::OptionalCall { callee, this_val, args, .. }
        | Instruction::Call { callee, this_val, args, .. }
        | Instruction::SuperCall { callee, this_val, args, .. } => {
            if let Some(new) = replacements.get(callee) { *callee = *new; changed = true; }
            if let Some(new) = replacements.get(this_val) { *this_val = *new; changed = true; }
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) { *arg = *new; changed = true; }
            }
        }
        Instruction::ConstructCall { callee, this_val, args, .. } => {
            if let Some(new) = replacements.get(callee) { *callee = *new; changed = true; }
            if let Some(new) = replacements.get(this_val) { *this_val = *new; changed = true; }
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) { *arg = *new; changed = true; }
            }
        }
        Instruction::CallBuiltin { args, .. } => {
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) { *arg = *new; changed = true; }
            }
        }
        Instruction::DeleteProp { object, key, .. } => {
            if let Some(new) = replacements.get(object) { *object = *new; changed = true; }
            if let Some(new) = replacements.get(key) { *key = *new; changed = true; }
        }
        Instruction::PromiseResolve { promise, value }
        | Instruction::PromiseReject { promise, reason: value } => {
            if let Some(new) = replacements.get(promise) { *promise = *new; changed = true; }
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::Suspend { promise, .. } => {
            if let Some(new) = replacements.get(promise) { *promise = *new; changed = true; }
        }
        Instruction::GeneratorSuspend { result, .. } => {
            if let Some(new) = replacements.get(result) { *result = *new; changed = true; }
        }
        Instruction::IsException { value, .. }
        | Instruction::EncodeException { value, .. }
        | Instruction::ExceptionToObject { value, .. } => {
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::GuardSameFunction { callee, .. } => {
            if let Some(new) = replacements.get(callee) { *callee = *new; changed = true; }
        }
        Instruction::ObjectSpread { source, .. } => {
            if let Some(new) = replacements.get(source) { *source = *new; changed = true; }
        }
        Instruction::StoreVar { value, .. } => {
            if let Some(new) = replacements.get(value) { *value = *new; changed = true; }
        }
        Instruction::Phi { sources, .. } => {
            for source in sources.iter_mut() {
                if let Some(new) = replacements.get(&source.value) {
                    source.value = *new;
                    changed = true;
                }
            }
        }
    }

    changed
}

fn replace_in_terminator(
    terminator: &mut Terminator,
    replacements: &HashMap<ValueId, ValueId>,
) {
    match terminator {
        Terminator::Return { value: Some(v) } => {
            if let Some(new) = replacements.get(v) { *v = *new; }
        }
        Terminator::Branch { condition, .. } => {
            if let Some(new) = replacements.get(condition) { *condition = *new; }
        }
        Terminator::Switch { value, .. } => {
            if let Some(new) = replacements.get(value) { *value = *new; }
        }
        Terminator::Throw { value } => {
            if let Some(new) = replacements.get(value) { *value = *new; }
        }
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {}
    }
}