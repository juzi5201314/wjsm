//! 逃逸分析 + 标量替换（Escape Analysis + Scalar Replacement）pass。
//!
//! 识别函数内局部 `NewObject` 对象，如果对象不逃逸（所有使用都是 SetProp/GetProp
//! 常量字符串键、SetProto、CallBuiltin(IsJsObject)），则：
//!
//! 1. 把 GetProp 读取结果的使用处替换为 SetProp 写入的值；
//! 2. 删除 NewObject、SetProp、SetProto、CallBuiltin(IsJsObject) 指令。
//!
//! 保守策略：任何非上述模式的使用（Call、Return、StoreVar、Phi、LoadVar 等）→ 逃逸。

use std::collections::HashMap;
use wjsm_ir::{
    Constant, FunctionId, Instruction, Module, Terminator, ValueId, BasicBlockId,
};
use super::direct_call::{instr_uses, terminator_uses, collect_uses, instruction_dest};

/// 运行逃逸分析 + 标量替换。
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

                // 构建 def 表和常量字符串表
                let mut _defs: HashMap<ValueId, &Instruction> = HashMap::new();
                let mut const_strings: HashMap<ValueId, String> = HashMap::new();
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Some(dest) = instruction_dest(instruction) {
                            _defs.insert(dest, instruction);
                        }
                        if let Instruction::Const { dest, constant } = instruction {
                            if let Some(Constant::String(s)) = constants_base.get(constant.0 as usize) {
                                const_strings.insert(*dest, s.clone());
                            }
                        }
                    }
                }

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
                let mut needs_replacement: Vec<(ValueId, BasicBlockId)> = Vec::new();

                for (candidate_dest, _capacity, _def_block) in &candidates {
                    // 检查终止器是否使用 candidate（Return、Branch 等）
                    if used_in_terminator(function, *candidate_dest) {
                        continue;
                    }

                    let uses = collect_uses(function, *candidate_dest);

                    let mut escapes = false;
                    let mut slot_assignments: Vec<(ValueId, ValueId)> = Vec::new();
                    let mut slot_reads: Vec<(ValueId, ValueId)> = Vec::new();

                    for use_instr in &uses {
                        match use_instr {
                            Instruction::SetProp { object, key, value }
                                if *object == *candidate_dest =>
                            {
                                if const_strings.contains_key(key) {
                                    slot_assignments.push((*key, *value));
                                } else {
                                    escapes = true;
                                }
                            }
                            Instruction::GetProp { dest, object, key }
                                if *object == *candidate_dest =>
                            {
                                if const_strings.contains_key(key) {
                                    slot_reads.push((*key, *dest));
                                } else {
                                    escapes = true;
                                }
                            }
                            Instruction::SetProto { object, .. }
                                if *object == *candidate_dest => {}
                            Instruction::CallBuiltin { builtin, .. }
                                if *builtin == wjsm_ir::Builtin::IsJsObject => {}
                            _ => {
                                escapes = true;
                            }
                        }
                    }

                    if !escapes && !slot_assignments.is_empty() {
                        let all_reads_have_assignments = slot_reads.iter().all(|(read_key, _)| {
                            slot_assignments.iter().any(|(k, _)| k == read_key)
                        });

                        if all_reads_have_assignments {
                            needs_replacement.push((*candidate_dest, *_def_block));
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
            let mut all_replacements: HashMap<ValueId, ValueId> = HashMap::new();
            let mut delete_targets: Vec<(BasicBlockId, usize)> = Vec::new();

            for (candidate_dest, _def_block) in &needs_replacement {
                let uses = collect_uses(function, *candidate_dest);

                let mut slot_assignments: HashMap<ValueId, ValueId> = HashMap::new();
                let mut slot_reads: Vec<(ValueId, ValueId)> = Vec::new();
                let mut escapes = false;

                for use_instr in &uses {
                    match use_instr {
                        Instruction::SetProp { object, key, value }
                            if *object == *candidate_dest =>
                        {
                            if const_strings.contains_key(key) {
                                slot_assignments.insert(*key, *value);
                            } else {
                                escapes = true;
                            }
                        }
                        Instruction::GetProp { dest, object, key }
                            if *object == *candidate_dest =>
                        {
                            if const_strings.contains_key(key) {
                                slot_reads.push((*key, *dest));
                            } else {
                                escapes = true;
                            }
                        }
                        Instruction::SetProto { object, .. }
                            if *object == *candidate_dest => {}
                        Instruction::CallBuiltin { builtin, .. }
                            if *builtin == wjsm_ir::Builtin::IsJsObject => {}
                        _ => {
                            escapes = true;
                        }
                    }
                }

                if escapes {
                    continue;
                }

                // 构建替换映射：GetProp dest → SetProp value
                for (read_key, read_dest) in &slot_reads {
                    if let Some(assigned_value) = slot_assignments.get(read_key) {
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