//! inline_for_ea pass：静态多块内联 + 守卫式推测方法内联。
//!
//! 本 pass 在 direct_call pass 之后运行，利用已标记的 `direct_callable` 函数：
//!
//! - **阶段 A（静态多块内联）**：遇到 `FunctionRef(f)` 的 `Call`/`ConstructCall`
//!   且 f 是 direct_callable 时，将 f 的**整个函数体**（多块）原地展开到调用处：
//!   调用块分裂为 B_pre/B_post，克隆体参数替换（$this/$env/形参），Return 改写为
//!   Jump(B_post)。构造器内联（`new` 语义：显式返回非 $this 对象跳过）后调用点
//!   下游的 `select_construct_result` 链（is_exception → is_js_object → phi）由
//!   cfg_fold 自动塌缩。
//!
//! - **阶段 C（守卫式推测方法内联）**：`Call` 的 callee 是 `GetProp`（常量键）
//!   且模块内同名函数唯一时，发射 `GuardSameFunction` 守卫 + 快路径克隆（callee
//!   体 + 消费区），失配回退原动态调用。语义无损。
//!
//! - 配合 cfg_fold pass（IsException/IsJsObject 折叠、常量分支折叠、死块中和、
//!   phi 塌缩、DCE）迭代至不动点。

use std::collections::{BTreeSet, HashMap, HashSet};

use wjsm_ir::{
    BasicBlock, BasicBlockId, Builtin, Constant, ConstantId, FunctionId, Instruction, Module,
    Terminator, ValueId, is_host_shared_variable,
};

use super::cfg_fold::{self, terminator_successors};
use crate::passes::direct_call::{instr_uses, instruction_dest, terminator_uses};

/// 计算函数内最大的 ValueId。
pub(crate) fn max_value_id_in_function(function: &wjsm_ir::Function) -> u32 {
    let mut max = 0u32;
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                max = max.max(dest.0);
            }
            for used in instr_uses(instruction) {
                max = max.max(used.0);
            }
        }
        for used in terminator_uses(block.terminator()) {
            max = max.max(used.0);
        }
    }
    max
}

/// 为指令中的 ValueId 添加偏移量。
fn add_offset_to_value_id(ins: &mut Instruction, offset: u32) {
    use Instruction::*;
    let add = |id: &mut ValueId| id.0 += offset;
    let add_vec = |ids: &mut [ValueId]| {
        for id in ids.iter_mut() {
            id.0 += offset;
        }
    };
    match ins {
        Const { dest, .. } => {
            add(dest);
        }
        Binary { dest, lhs, rhs, .. } => {
            add(dest);
            add(lhs);
            add(rhs);
        }
        Unary { dest, value, .. } => {
            add(dest);
            add(value);
        }
        Compare { dest, lhs, rhs, .. } => {
            add(dest);
            add(lhs);
            add(rhs);
        }
        Phi { dest, sources } => {
            add(dest);
            for source in sources.iter_mut() {
                source.value.0 += offset;
            }
        }
        CallBuiltin {
            dest,
            args,
            builtin: _,
        } => {
            if let Some(dest) = dest {
                add(dest);
            }
            add_vec(args);
        }
        StringConcatVa { dest, parts } => {
            add(dest);
            add_vec(parts);
        }
        LoadVar { dest, .. } => {
            add(dest);
        }
        StoreVar { value, .. } => {
            add(value);
        }
        Call {
            dest,
            callee,
            this_val,
            args,
        } => {
            if let Some(dest) = dest {
                add(dest);
            }
            add(callee);
            add(this_val);
            add_vec(args);
        }
        SuperCall {
            dest,
            callee,
            this_val,
            args,
            ..
        } => {
            if let Some(dest) = dest {
                add(dest);
            }
            add(callee);
            add(this_val);
            add_vec(args);
        }
        ConstructCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            if let Some(dest) = dest {
                add(dest);
            }
            add(callee);
            add(this_val);
            add_vec(args);
        }
        NewObject { dest, .. } => {
            add(dest);
        }
        GetProp { dest, object, key } => {
            add(dest);
            add(object);
            add(key);
        }
        SetProp {
            dest,
            object,
            key,
            value,
            ..
        } => {
            add(dest);
            add(object);
            add(key);
            add(value);
        }
        CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => {
            add(dest);
            add(object);
            add(key);
            add(value);
        }
        DeleteProp { dest, object, key } => {
            add(dest);
            add(object);
            add(key);
        }
        SetProto { object, value } => {
            add(object);
            add(value);
        }
        NewArray { dest, .. } => {
            add(dest);
        }
        CloneArrayTemplate { dest, .. } => {
            add(dest);
        }
        InitObjectLiteral { dest, values, .. } => {
            add(dest);
            for value in values {
                add(value);
            }
        }
        GetElem {
            dest,
            object,
            index,
        } => {
            add(dest);
            add(object);
            add(index);
        }
        ElemShapeGuard { dest, array, .. } => {
            add(dest);
            add(array);
        }
        GetElemGuarded {
            dest,
            object,
            index,
            guard,
        } => {
            add(dest);
            add(object);
            add(index);
            add(guard);
        }
        GetPropGuarded {
            dest,
            object,
            key,
            guard,
            ..
        } => {
            add(dest);
            add(object);
            add(key);
            add(guard);
        }
        SetElem {
            dest,
            object,
            index,
            value,
            ..
        } => {
            add(dest);
            add(object);
            add(index);
            add(value);
        }
        OptionalGetProp { dest, object, key } => {
            add(dest);
            add(object);
            add(key);
        }
        OptionalGetElem { dest, object, key } => {
            add(dest);
            add(object);
            add(key);
        }
        OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            add(dest);
            add(callee);
            add(this_val);
            add_vec(args);
        }
        ObjectSpread {
            dest,
            object,
            source,
        } => {
            add(dest);
            add(object);
            add(source);
        }
        GetSuperBase { dest } => add(dest),
        GetSuperConstructor { dest } => add(dest),
        NewPromise { dest } => add(dest),
        PromiseResolve { promise, value } => {
            add(promise);
            add(value);
        }
        PromiseReject { promise, reason } => {
            add(promise);
            add(reason);
        }
        Suspend { promise, .. } => {
            add(promise);
        }
        GeneratorSuspend { result, .. } => {
            add(result);
        }
        CollectRestArgs { dest, .. } => {
            add(dest);
        }
        IsException { dest, value } => {
            add(dest);
            add(value);
        }
        GuardSameFunction { dest, callee, .. } => {
            add(dest);
            add(callee);
        }
        EncodeException { dest, value } => {
            add(dest);
            add(value);
        }
        ExceptionToObject { dest, value } => {
            add(dest);
            add(value);
        }
        DebugCheck { .. } => {}
    }
}

/// 替换指令中所有 `old_val` 为 `new_val`。
pub(crate) fn replace_value_id(ins: &mut Instruction, old_val: ValueId, new_val: ValueId) {
    use Instruction::*;
    let rep = |id: &mut ValueId| {
        if *id == old_val {
            *id = new_val;
        }
    };
    let rep_vec = |ids: &mut [ValueId]| {
        for id in ids.iter_mut() {
            if *id == old_val {
                *id = new_val;
            }
        }
    };
    match ins {
        Const { .. } => {}
        Binary { dest, lhs, rhs, .. } => {
            rep(dest);
            rep(lhs);
            rep(rhs);
        }
        Unary { dest, value, .. } => {
            rep(dest);
            rep(value);
        }
        Compare { dest, lhs, rhs, .. } => {
            rep(dest);
            rep(lhs);
            rep(rhs);
        }
        Phi { dest, sources } => {
            rep(dest);
            for source in sources.iter_mut() {
                rep(&mut source.value);
            }
        }
        CallBuiltin {
            dest,
            args,
            builtin: _,
        } => {
            if let Some(dest) = dest {
                rep(dest);
            }
            rep_vec(args);
        }
        StringConcatVa { dest, parts } => {
            rep(dest);
            rep_vec(parts);
        }
        LoadVar { dest, .. } => rep(dest),
        StoreVar { value, .. } => rep(value),
        Call {
            dest,
            callee,
            this_val,
            args,
        } => {
            if let Some(dest) = dest {
                rep(dest);
            }
            rep(callee);
            rep(this_val);
            rep_vec(args);
        }
        SuperCall {
            dest,
            callee,
            this_val,
            args,
            ..
        } => {
            if let Some(dest) = dest {
                rep(dest);
            }
            rep(callee);
            rep(this_val);
            rep_vec(args);
        }
        ConstructCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            if let Some(dest) = dest {
                rep(dest);
            }
            rep(callee);
            rep(this_val);
            rep_vec(args);
        }
        NewObject { dest, .. } => rep(dest),
        GetProp { dest, object, key } => {
            rep(dest);
            rep(object);
            rep(key);
        }
        SetProp {
            dest,
            object,
            key,
            value,
            ..
        } => {
            rep(dest);
            rep(object);
            rep(key);
            rep(value);
        }
        CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => {
            rep(dest);
            rep(object);
            rep(key);
            rep(value);
        }
        DeleteProp { dest, object, key } => {
            rep(dest);
            rep(object);
            rep(key);
        }
        SetProto { object, value } => {
            rep(object);
            rep(value);
        }
        NewArray { dest, .. } => rep(dest),
        CloneArrayTemplate { dest, .. } => rep(dest),
        InitObjectLiteral { dest, values, .. } => {
            rep(dest);
            for value in values {
                rep(value);
            }
        }
        GetElem {
            dest,
            object,
            index,
        } => {
            rep(dest);
            rep(object);
            rep(index);
        }
        ElemShapeGuard { dest, array, .. } => {
            rep(dest);
            rep(array);
        }
        GetElemGuarded {
            dest,
            object,
            index,
            guard,
        } => {
            rep(dest);
            rep(object);
            rep(index);
            rep(guard);
        }
        GetPropGuarded {
            dest,
            object,
            key,
            guard,
            ..
        } => {
            rep(dest);
            rep(object);
            rep(key);
            rep(guard);
        }
        SetElem {
            dest,
            object,
            index,
            value,
            ..
        } => {
            rep(dest);
            rep(object);
            rep(index);
            rep(value);
        }
        OptionalGetProp { dest, object, key } => {
            rep(dest);
            rep(object);
            rep(key);
        }
        OptionalGetElem { dest, object, key } => {
            rep(dest);
            rep(object);
            rep(key);
        }
        OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            rep(dest);
            rep(callee);
            rep(this_val);
            rep_vec(args);
        }
        ObjectSpread {
            dest,
            object,
            source,
        } => {
            rep(dest);
            rep(object);
            rep(source);
        }
        GetSuperBase { dest } => rep(dest),
        GetSuperConstructor { dest } => rep(dest),
        NewPromise { dest } => rep(dest),
        PromiseResolve { promise, value } => {
            rep(promise);
            rep(value);
        }
        PromiseReject { promise, reason } => {
            rep(promise);
            rep(reason);
        }
        Suspend { promise, .. } => rep(promise),
        GeneratorSuspend { result, .. } => rep(result),
        CollectRestArgs { dest, .. } => rep(dest),
        IsException { dest, value } => {
            rep(dest);
            rep(value);
        }
        GuardSameFunction { dest, callee, .. } => {
            rep(dest);
            rep(callee);
        }
        EncodeException { dest, value } => {
            rep(dest);
            rep(value);
        }
        ExceptionToObject { dest, value } => {
            rep(dest);
            rep(value);
        }
        DebugCheck { .. } => {}
    }
}

/// 替换终止器中所有 `old_val` 为 `new_val`。
pub(crate) fn replace_value_id_in_terminator(
    terminator: &mut Terminator,
    old_val: ValueId,
    new_val: ValueId,
) {
    match terminator {
        Terminator::Return { value: Some(v) } if *v == old_val => *v = new_val,
        Terminator::Branch { condition, .. } if *condition == old_val => *condition = new_val,
        Terminator::Switch { value, .. } if *value == old_val => *value = new_val,
        Terminator::Throw { value } if *value == old_val => *value = new_val,
        _ => {}
    }
}

/// 在函数中，将 `old_val` 的所有引用替换为 `new_val`。
pub(crate) fn replace_all_uses_of(
    function: &mut wjsm_ir::Function,
    old_val: ValueId,
    new_val: ValueId,
) {
    if old_val == new_val {
        return;
    }
    for block in function.blocks_mut() {
        for instr in block.instructions_mut() {
            replace_value_id(instr, old_val, new_val);
        }
        replace_value_id_in_terminator(block.terminator_mut(), old_val, new_val);
    }
}

/// 是否为 `$this` 族变量 IR 名。
fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

/// 是否为 `$env` 族变量 IR 名。
fn is_env_name(name: &str) -> bool {
    name == "$env" || name.ends_with(".$env")
}

/// 内联后 callee 栈帧消失：把仍留在 host 槽表上的函数局部写成 undefined，
/// 避免 IIFE 里的 WeakMap key / FinalizationRegistry target 被调用者一直钉住。
fn callee_dead_slot_names(callee: &wjsm_ir::Function) -> Vec<String> {
    let captured: HashSet<&str> = callee.captured_names().iter().map(String::as_str).collect();
    let mut names = BTreeSet::new();
    for block in callee.blocks() {
        for instruction in block.instructions() {
            let name = match instruction {
                Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                _ => continue,
            };
            if is_host_shared_variable(name) || is_this_name(name) || is_env_name(name) {
                continue;
            }
            if captured.contains(name.as_str()) {
                continue;
            }
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

fn store_dead_slots(block: &mut BasicBlock, names: &[String], undefined: ValueId) {
    for name in names {
        block.push_instruction(Instruction::StoreVar {
            name: name.clone(),
            value: undefined,
        });
    }
}

/// 函数体内是否含无法用简单替换内联的指令（super/rest/async 等）。
fn contains_excluded_instruction(function: &wjsm_ir::Function) -> bool {
    function.blocks().iter().any(|block| {
        block.instructions().iter().any(|ins| {
            matches!(
                ins,
                Instruction::SuperCall { .. }
                    | Instruction::GetSuperBase { .. }
                    | Instruction::GetSuperConstructor { .. }
                    | Instruction::CollectRestArgs { .. }
                    | Instruction::NewPromise { .. }
                    | Instruction::PromiseResolve { .. }
                    | Instruction::PromiseReject { .. }
                    | Instruction::Suspend { .. }
                    | Instruction::GeneratorSuspend { .. }
            )
        })
    })
}

/// 查找或追加 `Constant::Undefined` 常量。
pub(crate) fn undefined_const_id(module: &mut Module) -> ConstantId {
    for (i, c) in module.constants().iter().enumerate() {
        if matches!(c, Constant::Undefined) {
            return ConstantId(i as u32);
        }
    }
    module.add_constant(Constant::Undefined)
}

/// 阶段 A 的单轮候选。
#[derive(Debug, Clone)]
struct StaticInlineCandidate {
    func_idx: u32,
    block_idx: u32,
    instr_idx: usize,
    callee: FunctionId,
    is_construct: bool,
    this_val: ValueId,
    args: Vec<ValueId>,
    dest: Option<ValueId>,
    construct_object_returns: HashSet<BasicBlockId>,
    closure_env: Option<ValueId>,
}

fn classify_construct_return(
    definitions: &HashMap<ValueId, Instruction>,
    constants: &[Constant],
    value: ValueId,
) -> Option<bool> {
    match definitions.get(&value) {
        Some(Instruction::NewObject { .. })
        | Some(Instruction::NewArray { .. })
        | Some(Instruction::CloneArrayTemplate { .. })
        | Some(Instruction::InitObjectLiteral { .. }) => Some(true),
        Some(Instruction::LoadVar { name, .. }) if is_this_name(name) => Some(false),
        Some(
            Instruction::Binary { .. }
            | Instruction::Unary { .. }
            | Instruction::Compare { .. }
            | Instruction::StringConcatVa { .. },
        ) => Some(false),
        Some(Instruction::Const { constant, .. }) => match constants.get(constant.0 as usize) {
            Some(
                Constant::Number(_)
                | Constant::String(_)
                | Constant::Bool(_)
                | Constant::Null
                | Constant::Undefined,
            ) => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn compute_load_var_reaching(function: &wjsm_ir::Function) -> HashMap<ValueId, ValueId> {
    let mut block_out: HashMap<BasicBlockId, HashMap<String, Option<ValueId>>> = HashMap::new();
    let mut block_in: HashMap<BasicBlockId, HashMap<String, Option<ValueId>>> = HashMap::new();
    let mut load_reaching: HashMap<ValueId, ValueId> = HashMap::new();

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            let mut in_map: HashMap<String, Option<ValueId>> = HashMap::new();
            let mut first = true;
            for pred in function
                .blocks()
                .iter()
                .filter(|p| cfg_fold::terminator_successors(p.terminator()).contains(&block.id()))
            {
                if let Some(pred_out) = block_out.get(&pred.id()) {
                    if first {
                        in_map = pred_out.clone();
                        first = false;
                    } else {
                        // 对称 meet：任一 pred 缺失或不一致的键一律降为未知，
                        // 否则单边保留会把「某路径未定义」误判为确定值。
                        for (k, v) in pred_out {
                            match in_map.get_mut(k) {
                                Some(existing) => {
                                    if *existing != *v {
                                        *existing = None;
                                    }
                                }
                                None => {
                                    in_map.insert(k.clone(), None);
                                }
                            }
                        }
                        for (k, v) in in_map.iter_mut() {
                            if !pred_out.contains_key(k) {
                                *v = None;
                            }
                        }
                    }
                }
            }

            let mut current = in_map.clone();
            for instr in block.instructions() {
                if let Instruction::StoreVar { name, value } = instr {
                    current.insert(name.clone(), Some(*value));
                }
            }
            // 与旧 out 做只降不升的合并（absent → Some → None 单向）：
            // in 集每轮从零重建属非单调混沌迭代，环上可能出现 Some/None
            // 周期振荡永不收敛（with 分派挂在生成器循环头时实际触发）；
            // 强制下降后每个 (block, key) 至多变更两次，必然到达不动点。
            let descended = match block_out.get(&block.id()) {
                None => current,
                Some(old) => {
                    let mut merged = current;
                    for (k, v) in merged.iter_mut() {
                        match old.get(k) {
                            None => {}
                            Some(old_v) if old_v == v => {}
                            Some(_) => *v = None,
                        }
                    }
                    for k in old.keys() {
                        merged.entry(k.clone()).or_insert(None);
                    }
                    merged
                }
            };
            if block_out.get(&block.id()) != Some(&descended) {
                block_out.insert(block.id(), descended);
                changed = true;
            }
            block_in.insert(block.id(), in_map);
        }
    }

    for block in function.blocks() {
        let mut current = block_in.get(&block.id()).cloned().unwrap_or_default();
        for instr in block.instructions() {
            match instr {
                Instruction::StoreVar { name, value } => {
                    current.insert(name.clone(), Some(*value));
                }
                Instruction::LoadVar { dest, name } => {
                    if let Some(Some(reaching_val)) = current.get(name) {
                        load_reaching.insert(*dest, *reaching_val);
                    }
                }
                _ => {}
            }
        }
    }

    load_reaching
}

fn resolve_callee_id(
    defs: &HashMap<ValueId, Instruction>,
    constants: &[Constant],
    load_reaching: &HashMap<ValueId, ValueId>,
    callee: &ValueId,
) -> Option<(FunctionId, Option<ValueId>)> {
    let mut current = *callee;
    while let Some(reaching) = load_reaching.get(&current) {
        if *reaching == current {
            break;
        }
        current = *reaching;
    }

    match defs.get(&current) {
        Some(Instruction::Const { constant, .. }) => match constants.get(constant.0 as usize) {
            Some(Constant::FunctionRef(f)) => Some((*f, None)),
            _ => None,
        },
        Some(Instruction::CallBuiltin {
            builtin: Builtin::CreateClosure,
            args: closure_args,
            ..
        }) if closure_args.len() >= 2 => {
            let mut fn_val = closure_args[0];
            while let Some(reaching) = load_reaching.get(&fn_val) {
                if *reaching == fn_val {
                    break;
                }
                fn_val = *reaching;
            }
            let fn_ref = match defs.get(&fn_val) {
                Some(Instruction::Const { constant, .. }) => {
                    match constants.get(constant.0 as usize) {
                        Some(Constant::FunctionRef(f)) => *f,
                        _ => return None,
                    }
                }
                _ => return None,
            };
            Some((fn_ref, Some(closure_args[1])))
        }
        _ => None,
    }
}

/// 阶段 A：静态多块内联的一轮。返回是否发生了任何内联。
fn static_inline_round(module: &mut Module) -> bool {
    // ── 预收集（不可变借用阶段）──
    let constants_snapshot = module.constants().to_vec();
    let per_func_defs: Vec<HashMap<ValueId, Instruction>> = module
        .functions()
        .iter()
        .map(|f| {
            let mut defs = HashMap::new();
            for block in f.blocks() {
                for instr in block.instructions() {
                    if let Some(dest) = instruction_dest(instr) {
                        defs.insert(dest, instr.clone());
                    }
                }
            }
            defs
        })
        .collect();
    let per_func_load_reaching: Vec<HashMap<ValueId, ValueId>> = module
        .functions()
        .iter()
        .map(compute_load_var_reaching)
        .collect();
    let per_func_info: Vec<(bool, bool, usize)> = module
        .functions()
        .iter()
        .map(|f| (f.direct_callable(), f.has_eval(), f.blocks().len()))
        .collect();
    let per_func_max_value: Vec<u32> = module
        .functions()
        .iter()
        .map(max_value_id_in_function)
        .collect();

    // ── 候选收集 ──
    let mut candidates: Vec<StaticInlineCandidate> = Vec::new();
    for (func_idx, function) in module.functions().iter().enumerate() {
        for (block_idx, block) in function.blocks().iter().enumerate() {
            for (instr_idx, instr) in block.instructions().iter().enumerate() {
                let (is_construct, dest, callee, this_val, args) = match instr {
                    Instruction::ConstructCall {
                        dest,
                        callee,
                        this_val,
                        args,
                    } => (true, dest, callee, this_val, args),
                    Instruction::Call {
                        dest,
                        callee,
                        this_val,
                        args,
                    } => (false, dest, callee, this_val, args),
                    _ => continue,
                };
                let (callee_id, closure_env) = match resolve_callee_id(
                    &per_func_defs[func_idx],
                    &constants_snapshot,
                    &per_func_load_reaching[func_idx],
                    callee,
                ) {
                    Some(x) => x,
                    None => continue,
                };
                let callee_idx = callee_id.0 as usize;
                if callee_idx >= per_func_info.len() {
                    continue;
                }
                let (direct_callable, has_eval, num_blocks) = per_func_info[callee_idx];
                let can_call = direct_callable || closure_env.is_some();
                if !can_call || has_eval || num_blocks == 0 || callee_idx == func_idx {
                    continue;
                }
                let callee_func = &module.functions()[callee_idx];
                if callee_func.blocks().len() > 200 {
                    continue;
                }
                if contains_excluded_instruction(callee_func) {
                    continue;
                }
                // 至少含 1 个 Return（零 Return 的纯循环/throw 体内联会使 dest 无定义）。
                let has_return = callee_func
                    .blocks()
                    .iter()
                    .any(|b| matches!(b.terminator(), Terminator::Return { .. }));
                if !has_return {
                    continue;
                }
                let mut construct_object_returns = HashSet::new();
                if is_construct {
                    // 仅内联无异常终止器、且每个返回值都能由 IR 定义证明为
                    // `$this`/原语或对象。对象返回值必须在合流处保留为 `new`
                    // 的结果；无法分类时继续走通用构造器路径。
                    let returns_classified =
                        callee_func
                            .blocks()
                            .iter()
                            .all(|block| match block.terminator() {
                                Terminator::Return { value: None } => true,
                                Terminator::Return { value: Some(value) } => {
                                    match classify_construct_return(
                                        &per_func_defs[callee_idx],
                                        &constants_snapshot,
                                        *value,
                                    ) {
                                        Some(true) => {
                                            construct_object_returns.insert(block.id());
                                            true
                                        }
                                        Some(false) => true,
                                        None => false,
                                    }
                                }
                                Terminator::Throw { .. } => false,
                                _ => true,
                            });
                    if !returns_classified {
                        continue;
                    }
                }
                candidates.push(StaticInlineCandidate {
                    func_idx: func_idx as u32,
                    block_idx: block_idx as u32,
                    instr_idx,
                    callee: callee_id,
                    is_construct,
                    this_val: *this_val,
                    args: args.clone(),
                    dest: *dest,
                    construct_object_returns,
                    closure_env,
                });
            }
        }
    }

    if candidates.is_empty() {
        return false;
    }

    // ── 执行：按 (func, block, instr) 逆序处理，插入不使先前候选偏移 ──
    candidates.sort_by_key(|c| (c.func_idx, c.block_idx, c.instr_idx as u32));
    candidates.reverse();
    let mut current_max_value = per_func_max_value;
    let undefined_id = undefined_const_id(module);
    for candidate in candidates {
        inline_static_candidate(module, &candidate, &mut current_max_value, undefined_id);
    }
    true
}

/// 内联单个静态候选（调用块分裂 + 克隆 callee 体 + 参数替换 + Return 处理）。
fn inline_static_candidate(
    module: &mut Module,
    candidate: &StaticInlineCandidate,
    current_max_value: &mut [u32],
    undefined_id: ConstantId,
) {
    let func_idx = candidate.func_idx as usize;
    let block_idx = candidate.block_idx as usize;
    let instr_idx = candidate.instr_idx;
    let caller_id = FunctionId(candidate.func_idx);

    let callee_func = match module.function_mut(candidate.callee) {
        Some(f) => f.clone(),
        None => return,
    };

    let current_max = current_max_value[func_idx];
    let undefined_dest = ValueId(current_max + 1);
    let value_offset = current_max + 2;

    // ── 分裂调用块 ──
    // B_pre 保留调用前指令 + undefined 常量，终止器暂置 Jump(克隆入口)；
    // B_post 承接调用后指令与 B 原终止器（id = 原块数）。
    let (pre_instructions, post_instructions, orig_terminator) = {
        let block = &module.functions()[func_idx].blocks()[block_idx];
        let pre: Vec<Instruction> = block.instructions()[..instr_idx].to_vec();
        let post: Vec<Instruction> = block.instructions()[instr_idx + 1..].to_vec();
        (pre, post, block.terminator().clone())
    };
    let b_post_id = BasicBlockId(module.functions()[func_idx].blocks().len() as u32);

    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        let mut b_post = BasicBlock::new(b_post_id);
        for ins in post_instructions {
            b_post.push_instruction(ins);
        }
        b_post.set_terminator(orig_terminator);
        caller.push_block(b_post);
    }
    let block_offset = module.functions()[func_idx].blocks().len() as u32;
    let clone_entry = BasicBlockId(callee_func.entry().0 + block_offset);

    // ── 克隆 callee 全部块（块 id 与 ValueId 双偏移）──
    // 参数 LoadVar（$this/$env/形参）通常是纯参数读取：克隆时跳过（不产生指令），
    // 记录 (mapped dest → 实参值) 映射，克隆完成后全函数替换 use。
    // 注意必须跳过而非保留后替换 dest：替换 dest 会改写克隆体其他指令的
    // 定义（前一轮内联可能已造成值重用），破坏 SSA。
    //
    // 例外：形参名若被函数体内 `StoreVar` 覆写（默认参数等模式），同名 LoadVar
    // 读的是 store 后的值而非形参初始值——全部保留指令，并在克隆入口注入
    // `StoreVar { name, value: 实参 }` 提供初始值。
    let callee_params = callee_func.params().to_vec();
    let dead_slots = callee_dead_slot_names(&callee_func);
    let stored_names: HashSet<&str> = callee_func
        .blocks()
        .iter()
        .flat_map(|b| b.instructions())
        .filter_map(|ins| match ins {
            Instruction::StoreVar { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut param_subst: Vec<(ValueId, ValueId)> = Vec::new();
    let mut inject_subst: Vec<(String, ValueId)> = Vec::new();
    let mut cloned_blocks: Vec<BasicBlock> = Vec::with_capacity(callee_func.blocks().len());
    for cb in callee_func.blocks() {
        let mut clone = BasicBlock::new(BasicBlockId(cb.id().0 + block_offset));
        for ins in cb.instructions() {
            if let Instruction::LoadVar { dest, name } = ins {
                let mapped_dest = ValueId(dest.0 + value_offset);
                if is_this_name(name) {
                    param_subst.push((mapped_dest, candidate.this_val));
                    continue;
                }
                if is_env_name(name) {
                    let env_val = candidate.closure_env.unwrap_or(undefined_dest);
                    param_subst.push((mapped_dest, env_val));
                    continue;
                }
                if let Some((param_idx, _)) = callee_params
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.as_str() == name)
                    && param_idx >= 2
                {
                    let arg_idx = param_idx - 2;
                    let arg_value = if arg_idx < candidate.args.len() {
                        candidate.args[arg_idx]
                    } else {
                        // 实参不足 → undefined。
                        undefined_dest
                    };
                    if stored_names.contains(name.as_str()) {
                        // 形参被 StoreVar 覆写：保留 LoadVar，入口注入初始值。
                        inject_subst.push((name.clone(), arg_value));
                    } else {
                        param_subst.push((mapped_dest, arg_value));
                        continue;
                    }
                }
            }
            let mut ins = ins.clone();
            add_offset_to_value_id(&mut ins, value_offset);
            ins.remap_blocks(&mut |b| BasicBlockId(b.0 + block_offset));
            clone.push_instruction(ins);
        }
        let mut term = cb.terminator().clone();
        term.remap_values(&mut |v| ValueId(v.0 + value_offset));
        term.remap_blocks(&mut |b| BasicBlockId(b.0 + block_offset));
        clone.set_terminator(term);
        cloned_blocks.push(clone);
    }

    // ── Return/Throw 处理 ──
    // Return 全部改写为 Jump(B_post)；Call 记录返回值供 dest 替换/phi。
    // Throw（callee 的语句级异常传播）改写为「存异常到调用点的 catch 变量 +
    // 跳转 catch 处理块」——否则 throw 编译为 CreateException+Return 直接从
    // 调用者函数返回，绕过语句级 is_exception 分叉（try/catch 失效）。
    let exception_path = if !candidate.is_construct {
        candidate.dest.and_then(|d| {
            find_exception_path(&module.functions()[func_idx], block_idx, instr_idx, d)
        })
    } else {
        None
    };
    let mut return_records: Vec<(BasicBlockId, ValueId)> = Vec::new();
    let mut construct_return_records: Vec<(BasicBlockId, ValueId)> = Vec::new();
    for (original, clone) in callee_func.blocks().iter().zip(cloned_blocks.iter_mut()) {
        let term = clone.terminator().clone();
        match term {
            Terminator::Return { value } => {
                if candidate.is_construct {
                    let selected = if candidate.construct_object_returns.contains(&original.id()) {
                        value.unwrap_or(candidate.this_val)
                    } else {
                        candidate.this_val
                    };
                    construct_return_records.push((clone.id(), selected));
                } else {
                    let mapped = value.unwrap_or(undefined_dest);
                    return_records.push((clone.id(), mapped));
                }
                store_dead_slots(clone, &dead_slots, undefined_dest);
                clone.set_terminator(Terminator::Jump { target: b_post_id });
            }
            Terminator::Throw { value } => {
                store_dead_slots(clone, &dead_slots, undefined_dest);
                if let Some((tmp_name, catch_target)) = &exception_path {
                    clone.push_instruction(Instruction::StoreVar {
                        name: tmp_name.clone(),
                        value,
                    });
                    clone.set_terminator(Terminator::Jump {
                        target: *catch_target,
                    });
                }
                // 无异常路径（防御）：保留 throw（原语义，异常逃逸给调用者）。
            }
            _ => {}
        }
    }

    // 克隆体 max ValueId（记账用）。
    let mut clone_max = 0u32;
    for block in &cloned_blocks {
        for ins in block.instructions() {
            if let Some(d) = instruction_dest(ins) {
                clone_max = clone_max.max(d.0);
            }
            for used in instr_uses(ins) {
                clone_max = clone_max.max(used.0);
            }
        }
        for used in terminator_uses(block.terminator()) {
            clone_max = clone_max.max(used.0);
        }
    }
    current_max_value[func_idx] = undefined_dest.0.max(clone_max);

    // ── 重写 B_pre：调用前指令 + undefined 常量 + 注入的形参初始值；
    //    终止器 = Jump(克隆入口)。 ──
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        let b_pre = &mut caller.blocks_mut()[block_idx];
        *b_pre.instructions_mut() = pre_instructions;
        b_pre.push_instruction(Instruction::Const {
            dest: undefined_dest,
            constant: undefined_id,
        });
        for (name, value) in &inject_subst {
            b_pre.push_instruction(Instruction::StoreVar {
                name: name.clone(),
                value: *value,
            });
        }
        b_pre.set_terminator(Terminator::Jump {
            target: clone_entry,
        });
    }

    // ── 追加克隆块并应用替换 ──
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        for clone in cloned_blocks {
            caller.push_block(clone);
        }
        // 参数替换。
        for (old_val, new_val) in &param_subst {
            replace_all_uses_of(caller, *old_val, *new_val);
        }
        // 调用结果处理。返回值的记录可能来自参数 LoadVar 的 dest
        // （如 `return x`），必须经 param_subst 解析为实参值，否则悬空。
        let resolve_param = |v: ValueId| {
            param_subst
                .iter()
                .find(|(old, _)| *old == v)
                .map_or(v, |(_, new)| *new)
        };
        if let Some(call_dest) = candidate.dest {
            let records = if candidate.is_construct {
                &construct_return_records
            } else {
                &return_records
            };
            match records.len() {
                1 => {
                    replace_all_uses_of(caller, call_dest, resolve_param(records[0].1));
                }
                n if n >= 2 => {
                    // 多返回点：B_post 头部插入 phi，保持每条控制流路径的结果。
                    let b_post = &mut caller.blocks_mut()[b_post_id.0 as usize];
                    let sources = records
                        .iter()
                        .map(|(pred, value)| wjsm_ir::PhiSource {
                            predecessor: *pred,
                            value: resolve_param(*value),
                        })
                        .collect();
                    b_post.instructions_mut().insert(
                        0,
                        Instruction::Phi {
                            dest: call_dest,
                            sources,
                        },
                    );
                }
                _ => {}
            }
        }
    }
}

/// 定位调用点的语句级异常处理路径。
///
/// 调用点形态：`dest = call …; %e = is_exception dest; branch %e, bb_t, bb_n`，
/// 其中 bb_t（异常路径）以 `exception_value(dest)` → `store var $tmp.N, …` →
/// `jump bb_catch` 结束。内联 callee 后其 `Throw` 终止器必须改写为
/// `store var <tmp 名>, 异常对象; jump bb_catch`（异常对象已是 exception_value
/// 结果），否则 throw 编译为 CreateException+Return 直接从调用者函数返回，
/// 绕过语句级异常检查（try/catch 失效）。
///
/// 返回 (tmp 变量名, catch 处理块)。找不到（调用点无语句级检查）→ None。
pub(crate) fn find_exception_path(
    function: &wjsm_ir::Function,
    block_idx: usize,
    instr_idx: usize,
    call_dest: ValueId,
) -> Option<(String, BasicBlockId)> {
    let block = &function.blocks()[block_idx];
    // 调用点之后的指令中找 `is_exception(call_dest)`。
    let mut exc_dest = None;
    for ins in block.instructions().iter().skip(instr_idx + 1) {
        if let Instruction::IsException { dest, value } = ins
            && *value == call_dest
        {
            exc_dest = Some(*dest);
            break;
        }
    }
    let exc_dest = exc_dest?;
    // 终止器 branch 的 true 目标即异常路径（is_exception 为 true 时进入）。
    let true_block = match block.terminator() {
        Terminator::Branch {
            condition,
            true_block,
            ..
        } if *condition == exc_dest => *true_block,
        _ => return None,
    };
    let tblock = function.block_by_id(true_block)?;
    let mut store_name = None;
    for ins in tblock.instructions() {
        if let Instruction::StoreVar { name, .. } = ins {
            store_name = Some(name.clone());
            break;
        }
    }
    let catch_target = match tblock.terminator() {
        Terminator::Jump { target } => *target,
        _ => return None,
    };
    Some((store_name?, catch_target))
}

/// 块 `t` 是否为回边目标（存在索引更大的块跳转到它，即循环头）。
fn is_backedge_target(function: &wjsm_ir::Function, t: BasicBlockId) -> bool {
    function
        .blocks()
        .iter()
        .any(|b| b.id().0 > t.0 && terminator_successors(b.terminator()).contains(&t))
}

/// 阶段 C 的单轮候选。
#[derive(Debug, Clone)]
struct SpeculativeCandidate {
    func_idx: u32,
    block_idx: u32,
    instr_idx: usize,
    /// 守卫输入：GetProp 的 dest（运行时 callee 值）。
    guard_callee: ValueId,
    target: FunctionId,
    this_val: ValueId,
    args: Vec<ValueId>,
    /// Call 的 dest（R）。
    dest: ValueId,
    /// 区域块集 S（有序：调用块第一，其余 BFS 序）。
    region_blocks: Vec<BasicBlockId>,
}

/// 计算推测内联站点的消费区（闭包 T + 区域块集 S）。
///
/// 返回 None 表示约束不满足（入度 > 1 或出口使用 T 值），调用点跳过。
fn compute_region(
    function: &wjsm_ir::Function,
    call_block: BasicBlockId,
    dest: ValueId,
) -> Option<(HashSet<ValueId>, Vec<BasicBlockId>)> {
    // ── 闭包 T：初始 {R}，指令 use 含 T 值 → dest 入 T；
    //    StoreVar(V) 的 value ∈ T → 函数内所有 LoadVar(V) dest 入 T。 ──
    let mut closure: HashSet<ValueId> = HashSet::from([dest]);
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            for ins in block.instructions() {
                let mut uses = instr_uses(ins);
                if let Instruction::Phi { sources, .. } = ins {
                    uses.extend(sources.iter().map(|s| s.value));
                }
                let uses_closure = uses.iter().any(|u| closure.contains(u));
                if uses_closure
                    && let Some(d) = instruction_dest(ins)
                    && closure.insert(d)
                {
                    changed = true;
                }
                if let Instruction::StoreVar { name, value } = ins
                    && closure.contains(value)
                {
                    for block2 in function.blocks() {
                        for ins2 in block2.instructions() {
                            if let Instruction::LoadVar { dest: d, name: n } = ins2
                                && n == name
                                && closure.insert(*d)
                            {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // ── 区域块集 S：调用块起沿终止器后继收集含区域相关指令的块 ──
    let mut s_blocks: Vec<BasicBlockId> = Vec::new();
    let mut seen: HashSet<BasicBlockId> = HashSet::new();
    let mut stack = vec![call_block];
    while let Some(bid) = stack.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let block = match function.block_by_id(bid) {
            Some(b) => b,
            None => continue,
        };
        let contains_region_instr = block
            .instructions()
            .iter()
            .any(|ins| instruction_dest(ins).is_some_and(|d| closure.contains(&d)));
        if !contains_region_instr {
            continue;
        }
        s_blocks.push(bid);
        for succ in terminator_successors(block.terminator()) {
            stack.push(succ);
        }
    }
    if s_blocks.is_empty() {
        return None;
    }

    // ── 入度约束：S 中除调用块外每块入度 == 1（唯一前驱）──
    let mut preds: HashMap<BasicBlockId, usize> = HashMap::new();
    for block in function.blocks() {
        for succ in terminator_successors(block.terminator()) {
            *preds.entry(succ).or_insert(0) += 1;
        }
    }
    for &bid in &s_blocks {
        if bid != call_block && preds.get(&bid).copied().unwrap_or(0) != 1 {
            return None;
        }
    }

    // ── 出口检查：S 外的可达块不得使用 T 值 ──
    //
    // 必须做**传递**可达性，不能只看一层后继：S 的收集会在「不含区域定义」的块
    // 上剪枝（纯 `jump` 跳板就是这种块），于是真正消费 T 值的块可能隔着一两个
    // 跳板落在 S 外。只查一层会让它漏过检查——区域克隆不含它，克隆路径上该块
    // 的 T 值引用要么悬空、要么落回慢路径的定义，表现为「快路径丢掉了对累加器
    // 的写回」（`t += f()` 恒为初值）。
    let s_set: HashSet<BasicBlockId> = s_blocks.iter().copied().collect();
    let mut exit_seen: HashSet<BasicBlockId> = HashSet::new();
    let mut exit_stack: Vec<BasicBlockId> = Vec::new();
    for &bid in &s_blocks {
        let Some(block) = function.block_by_id(bid) else {
            continue;
        };
        for succ in terminator_successors(block.terminator()) {
            if !s_set.contains(&succ) {
                exit_stack.push(succ);
            }
        }
    }
    while let Some(bid) = exit_stack.pop() {
        if s_set.contains(&bid) || !exit_seen.insert(bid) {
            continue;
        }
        let Some(block) = function.block_by_id(bid) else {
            continue;
        };
        let uses_t = block
            .instructions()
            .iter()
            .any(|ins| instr_uses(ins).iter().any(|u| closure.contains(u)))
            || terminator_uses(block.terminator())
                .iter()
                .any(|u| closure.contains(u));
        if uses_t {
            return None;
        }
        for succ in terminator_successors(block.terminator()) {
            exit_stack.push(succ);
        }
    }

    Some((closure, s_blocks))
}

/// 在指定块集合内重命名变量（LoadVar/StoreVar 的 name 字段）。
fn rename_var_in_blocks(blocks: &mut [BasicBlock], old_name: &str, new_name: &str) {
    for block in blocks {
        for ins in block.instructions_mut() {
            if let Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } = ins
                && name == old_name
            {
                *name = new_name.to_string();
            }
        }
    }
}

/// 阶段 C：守卫式推测方法内联的单次执行（不在不动点循环内）。
///
/// 收集在变更之前快照，天然不重复内联同一站点；候选按 (func, block, instr)
/// 逆序处理，块追加不影响既有块 id 与调用前指令索引。
fn speculative_inline_round(module: &mut Module) -> bool {
    let constants_snapshot: Vec<Constant> = module.constants().to_vec();
    let per_func_defs: Vec<HashMap<ValueId, Instruction>> = module
        .functions()
        .iter()
        .map(|f| {
            let mut defs = HashMap::new();
            for block in f.blocks() {
                for instr in block.instructions() {
                    if let Some(dest) = instruction_dest(instr) {
                        defs.insert(dest, instr.clone());
                    }
                }
            }
            defs
        })
        .collect();
    let per_func_info: Vec<(bool, bool, usize)> = module
        .functions()
        .iter()
        .map(|f| (f.direct_callable(), f.has_eval(), f.blocks().len()))
        .collect();
    let per_func_max_value: Vec<u32> = module
        .functions()
        .iter()
        .map(max_value_id_in_function)
        .collect();

    let mut candidates: Vec<SpeculativeCandidate> = Vec::new();
    for (func_idx, function) in module.functions().iter().enumerate() {
        for (block_idx, block) in function.blocks().iter().enumerate() {
            for (instr_idx, instr) in block.instructions().iter().enumerate() {
                // 仅处理有 dest 的动态 Call；callee 须为常量键 GetProp 的结果。
                let (dest, callee, this_val, args) = match instr {
                    Instruction::Call {
                        dest: Some(dest),
                        callee,
                        this_val,
                        args,
                    } => (dest, callee, this_val, args),
                    _ => continue,
                };
                let key_name = match per_func_defs[func_idx].get(callee) {
                    Some(Instruction::GetProp { key, .. }) => {
                        match per_func_defs[func_idx].get(key) {
                            Some(Instruction::Const { constant, .. }) => {
                                match constants_snapshot.get(constant.0 as usize) {
                                    Some(Constant::String(s)) => s.clone(),
                                    _ => continue,
                                }
                            }
                            _ => continue,
                        }
                    }
                    _ => continue,
                };
                // 目标函数选择：模块内同名函数恰好 1 个。
                let matching: Vec<FunctionId> = module
                    .functions()
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        f.name() == key_name
                            || f.name()
                                .strip_suffix(&key_name)
                                .is_some_and(|pre| pre.ends_with('.'))
                    })
                    .map(|(i, _)| FunctionId(i as u32))
                    .collect();
                if matching.len() != 1 {
                    continue;
                }
                let target = matching[0];
                let target_idx = target.0 as usize;
                if target_idx >= per_func_info.len() {
                    continue;
                }
                let (direct_callable, has_eval, target_blocks) = per_func_info[target_idx];
                if !direct_callable
                    || has_eval
                    || target_idx == func_idx
                    || target_blocks > 64
                    || target_blocks == 0
                {
                    continue;
                }
                let target_func = &module.functions()[target_idx];
                if contains_excluded_instruction(target_func) {
                    continue;
                }
                // 至少含 1 个 Return。
                let return_count = target_func
                    .blocks()
                    .iter()
                    .filter(|b| matches!(b.terminator(), Terminator::Return { .. }))
                    .count();
                if return_count == 0 {
                    continue;
                }
                // 函数大小上限：当前块数 + F 块数 + 8 <= 512。
                if function.blocks().len() + target_blocks + 8 > 512 {
                    continue;
                }
                // 区域计算（闭包 + 区域块 + 约束检查）。
                let Some((_closure, region_blocks)) = compute_region(function, block.id(), *dest)
                else {
                    continue;
                };
                candidates.push(SpeculativeCandidate {
                    func_idx: func_idx as u32,
                    block_idx: block_idx as u32,
                    instr_idx,
                    guard_callee: *callee,
                    target,
                    this_val: *this_val,
                    args: args.clone(),
                    dest: *dest,
                    region_blocks,
                });
            }
        }
    }

    if candidates.is_empty() {
        return false;
    }

    // ── 消费区冲突过滤 ──
    // 调用点的消费区（S）若含**另一个候选**调用点，内联本站点会把已内联的
    // 嵌套调用点复制进克隆体；其快/慢路径绑定原始调用点的 SSA 参数，克隆体
    // 路径上未定义（语义破损且难以完整降级）。保守跳过：嵌套方法调用不内联。
    // object-props 的 scale 消费区无嵌套调用点，不受影响。
    let conflicting: HashSet<u32> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, cand)| {
            let conflict = candidates.iter().enumerate().any(|(j, other)| {
                i != j
                    && other.func_idx == cand.func_idx
                    && cand.region_blocks.contains(&BasicBlockId(other.block_idx))
            });
            conflict.then_some(i as u32)
        })
        .collect();
    if !conflicting.is_empty() {
        candidates = candidates
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !conflicting.contains(&(*i as u32)))
            .map(|(_, c)| c)
            .collect();
    }
    if candidates.is_empty() {
        return false;
    }

    // 逆序执行（追加块不影响既有块 id / 调用前指令索引）。
    candidates.sort_by_key(|c| (c.func_idx, c.block_idx, c.instr_idx as u32));
    candidates.reverse();
    let mut current_max_value = per_func_max_value;
    let undefined_id = undefined_const_id(module);
    let mut rename_seq = 0u32;
    for candidate in candidates {
        inline_speculative_candidate(
            module,
            &candidate,
            &mut current_max_value,
            undefined_id,
            &mut rename_seq,
        );
    }
    true
}

/// 内联单个守卫式推测站点。
fn inline_speculative_candidate(
    module: &mut Module,
    candidate: &SpeculativeCandidate,
    current_max_value: &mut [u32],
    undefined_id: ConstantId,
    rename_seq: &mut u32,
) {
    let func_idx = candidate.func_idx as usize;
    let block_idx = candidate.block_idx as usize;
    let instr_idx = candidate.instr_idx;
    let caller_id = FunctionId(candidate.func_idx);
    let call_block_id = BasicBlockId(candidate.block_idx);

    // ── 区域出口处理 ──
    // 区域克隆块追加在函数末尾，其指向 S 外目标的边全部是"索引在前"的后向边。
    // 两种出口目标：
    //   1. 循环头或"纯跳板（Jump）到循环头"（object-props 的 bb11 → 循环头 bb3）
    //      → 回边由循环豁免；跳板目标**纳入克隆**（克隆跳板仍 Jump 到循环头，
    //      后向边豁免），避免克隆体产生指向非循环头块的后向边（状态机降级 ~2x）。
    //   2. 其他非循环头出口（如 return/throw）→ needs_cfg_dispatch 判定降级为
    //      CFG 状态机，部分形态存在栈不平衡缺陷（values remaining）→
    //      保守跳过该调用点。
    let mut region_blocks = candidate.region_blocks.clone();
    {
        let function = &module.functions()[func_idx];
        // 扩展：跳板出口（Jump 到循环头）纳入克隆集。
        let mut s_set: HashSet<BasicBlockId> = region_blocks.iter().copied().collect();
        let mut queue: Vec<BasicBlockId> = region_blocks.clone();
        while let Some(bid) = queue.pop() {
            for succ in terminator_successors(function.blocks()[bid.0 as usize].terminator()) {
                if s_set.contains(&succ) {
                    continue;
                }
                let succ_block = &function.blocks()[succ.0 as usize];
                let is_loop_jump_plate = matches!(
                    succ_block.terminator(),
                    Terminator::Jump { target } if is_backedge_target(function, *target)
                );
                if is_loop_jump_plate {
                    s_set.insert(succ);
                    region_blocks.push(succ);
                    queue.push(succ);
                }
            }
        }
        // 剩余出口（非循环头、非跳板）→ 跳过。
        let mut bad_backedge = false;
        'region_exit: for &bid in &region_blocks {
            for succ in terminator_successors(function.blocks()[bid.0 as usize].terminator()) {
                if s_set.contains(&succ) {
                    continue;
                }
                if !is_backedge_target(function, succ) {
                    bad_backedge = true;
                    break 'region_exit;
                }
            }
        }
        if bad_backedge {
            return;
        }
    }

    let target_func = match module.function_mut(candidate.target) {
        Some(f) => f.clone(),
        None => return,
    };

    let current_max = current_max_value[func_idx];
    let guard_dest = ValueId(current_max + 1);
    let undefined_dest = ValueId(current_max + 2);
    let value_offset = current_max + 3;

    // ── 1. 分裂调用块 ──
    let (pre_instructions, call_instruction, post_instructions, orig_terminator) = {
        let block = &module.functions()[func_idx].blocks()[block_idx];
        let pre: Vec<Instruction> = block.instructions()[..instr_idx].to_vec();
        let call_ins = block.instructions()[instr_idx].clone();
        let post: Vec<Instruction> = block.instructions()[instr_idx + 1..].to_vec();
        (pre, call_ins, post, block.terminator().clone())
    };
    let b_slow_id = BasicBlockId(module.functions()[func_idx].blocks().len() as u32);
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        let mut b_slow = BasicBlock::new(b_slow_id);
        // 慢路径：原 Call 指令 + 调用后指令 + B 原终止器（值不变）。
        b_slow.push_instruction(call_instruction);
        for ins in &post_instructions {
            b_slow.push_instruction(ins.clone());
        }
        b_slow.set_terminator(orig_terminator.clone());
        caller.push_block(b_slow);
    }

    // ── 2. 快路径 callee 克隆 ──
    let callee_offset = module.functions()[func_idx].blocks().len() as u32;
    let fast_entry = BasicBlockId(target_func.entry().0 + callee_offset);
    // 区域克隆入口 = callee 克隆追加后的 blocks.len()。
    let region_entry = BasicBlockId(
        module.functions()[func_idx].blocks().len() as u32 + target_func.blocks().len() as u32,
    );

    // 快路径 callee 的 Throw（语句级异常传播）改写为「存异常到调用点的 catch
    // 变量 + 跳转 catch 处理块」，否则 throw 编译为 CreateException+Return
    // 直接从调用者函数返回，绕过语句级 is_exception 分叉（try/catch 失效）。
    let exception_path = find_exception_path(
        &module.functions()[func_idx],
        block_idx,
        instr_idx,
        candidate.dest,
    );

    // 参数替换映射（callee params 约定 [$env, $this, ...args]）：
    // 参数 LoadVar 是纯参数读取，克隆时跳过并记录 use 替换（同阶段 A，见
    // inline_static_candidate——保留后替换 dest 会破坏 SSA）。
    let callee_params = target_func.params().to_vec();
    let dead_slots = callee_dead_slot_names(&target_func);
    let stored_names: HashSet<&str> = target_func
        .blocks()
        .iter()
        .flat_map(|b| b.instructions())
        .filter_map(|ins| match ins {
            Instruction::StoreVar { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut param_subst: Vec<(ValueId, ValueId)> = Vec::new();
    let mut inject_subst: Vec<(String, ValueId)> = Vec::new();
    let mut callee_clones: Vec<BasicBlock> = Vec::with_capacity(target_func.blocks().len());
    let mut return_records: Vec<(BasicBlockId, ValueId)> = Vec::new();
    for cb in target_func.blocks() {
        let mut clone = BasicBlock::new(BasicBlockId(cb.id().0 + callee_offset));
        for ins in cb.instructions() {
            if let Instruction::LoadVar { dest, name } = ins {
                let mapped_dest = ValueId(dest.0 + value_offset);
                if is_this_name(name) {
                    param_subst.push((mapped_dest, candidate.this_val));
                    continue;
                }
                if is_env_name(name) {
                    param_subst.push((mapped_dest, undefined_dest));
                    continue;
                }
                if let Some((param_idx, _)) = callee_params
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.as_str() == name)
                    && param_idx >= 2
                {
                    let arg_idx = param_idx - 2;
                    let arg_value = if arg_idx < candidate.args.len() {
                        candidate.args[arg_idx]
                    } else {
                        undefined_dest
                    };
                    if stored_names.contains(name.as_str()) {
                        // 形参被 StoreVar 覆写（默认参数等）：保留 LoadVar，入口注入初始值。
                        inject_subst.push((name.clone(), arg_value));
                    } else {
                        param_subst.push((mapped_dest, arg_value));
                        continue;
                    }
                }
            }
            let mut ins = ins.clone();
            add_offset_to_value_id(&mut ins, value_offset);
            ins.remap_blocks(&mut |b| BasicBlockId(b.0 + callee_offset));
            clone.push_instruction(ins);
        }
        let mut term = cb.terminator().clone();
        term.remap_values(&mut |v| ValueId(v.0 + value_offset));
        term.remap_blocks(&mut |b| BasicBlockId(b.0 + callee_offset));
        match term {
            Terminator::Return { value } => {
                // Return → Jump(区域入口)；记录返回值（None → undefined）。
                let mapped = value.unwrap_or(undefined_dest);
                return_records.push((clone.id(), mapped));
                store_dead_slots(&mut clone, &dead_slots, undefined_dest);
                clone.set_terminator(Terminator::Jump {
                    target: region_entry,
                });
            }
            Terminator::Throw { value } => {
                store_dead_slots(&mut clone, &dead_slots, undefined_dest);
                if let Some((tmp_name, catch_target)) = &exception_path {
                    clone.push_instruction(Instruction::StoreVar {
                        name: tmp_name.clone(),
                        value,
                    });
                    clone.set_terminator(Terminator::Jump {
                        target: *catch_target,
                    });
                } else {
                    clone.set_terminator(Terminator::Throw { value });
                }
            }
            other => {
                clone.set_terminator(other);
            }
        }
        callee_clones.push(clone);
    }

    // ── 3. 区域克隆 ──
    // 块 id 分配：region_entry 起按 S 顺序（调用块尾部第一）。
    let mut block_map: HashMap<BasicBlockId, BasicBlockId> = HashMap::new();
    let mut next_id = region_entry;
    for &orig in &region_blocks {
        block_map.insert(orig, next_id);
        next_id = BasicBlockId(next_id.0 + 1);
    }
    let resolve_param = |v: ValueId| {
        param_subst
            .iter()
            .find(|(old, _)| *old == v)
            .map_or(v, |(_, new)| *new)
    };

    // 区域内定义的 ValueId：只有它们在克隆时需要重编号。
    //
    // 区域**使用**但在区域外定义的值（B_pre 里的 `load var`、更早块的计算结果）
    // 必须原样引用：给全部 ValueId 盲目加偏移会让它们指向从未定义的编号，产出
    // 悬空 SSA。后端把未定义 ValueId 当成未初始化 local 发射，于是累加器这类
    // 跨迭代变量会读到陈旧值（`t = t + f()` 只保留最后一轮），且具体表现随
    // local 布局漂移。
    let region_defs: HashSet<ValueId> = {
        let function = &module.functions()[func_idx];
        let mut defs = HashSet::new();
        for &orig in &region_blocks {
            if orig == call_block_id {
                // 调用块只克隆调用点之后的部分；调用点之前的定义留在 B_pre。
                for ins in &post_instructions {
                    if let Some(d) = instruction_dest(ins) {
                        defs.insert(d);
                    }
                }
            } else {
                for ins in function.blocks()[orig.0 as usize].instructions() {
                    if let Some(d) = instruction_dest(ins) {
                        defs.insert(d);
                    }
                }
            }
        }
        defs
    };

    let mut max_callee_val = guard_dest.0.max(undefined_dest.0);
    for block in &callee_clones {
        for ins in block.instructions() {
            if let Some(d) = instruction_dest(ins) {
                max_callee_val = max_callee_val.max(d.0);
            }
            for used in instr_uses(ins) {
                max_callee_val = max_callee_val.max(used.0);
            }
        }
        for used in terminator_uses(block.terminator()) {
            max_callee_val = max_callee_val.max(used.0);
        }
    }

    let (ret_mapped, phi_opt) = match return_records.len() {
        0 => (undefined_dest, None),
        1 => (resolve_param(return_records[0].1), None),
        _ => {
            let phi_dest = ValueId(max_callee_val + 1);
            let sources = return_records
                .iter()
                .map(|(pred, val)| wjsm_ir::PhiSource {
                    predecessor: *pred,
                    value: resolve_param(*val),
                })
                .collect();
            (phi_dest, Some((phi_dest, sources)))
        }
    };

    // 区域偏移必须高于 callee 克隆和 phi_dest 已占用的编号，否则两套克隆会撞号。
    let region_offset = max_callee_val + if phi_opt.is_some() { 2 } else { 1 };
    let remap_region_value = |v: ValueId| -> ValueId {
        if v == candidate.dest {
            // 调用结果 → 快路径 callee 克隆的返回值。
            ret_mapped
        } else if region_defs.contains(&v) {
            ValueId(v.0 + region_offset)
        } else {
            // 区域外定义（B_pre / 更早块）：克隆体与慢路径共用同一定义。
            v
        }
    };

    // 变量改名：函数内所有 StoreVar/LoadVar 都在 S 内 → 克隆中改名 V$ea{N}。
    let s_set: HashSet<BasicBlockId> = region_blocks.iter().copied().collect();
    let mut rename_map: HashMap<String, String> = HashMap::new();
    {
        let function = &module.functions()[func_idx];
        let mut var_names: HashSet<&str> = HashSet::new();
        for block in function.blocks() {
            for ins in block.instructions() {
                match ins {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => {
                        var_names.insert(name.as_str());
                    }
                    _ => {}
                }
            }
        }
        for name in var_names {
            let all_in_s = function.blocks().iter().all(|b| {
                b.instructions().iter().all(|ins| match ins {
                    Instruction::LoadVar { name: n, .. }
                    | Instruction::StoreVar { name: n, .. } => n != name || s_set.contains(&b.id()),
                    _ => true,
                })
            });
            if all_in_s {
                let new_name = format!("{name}$ea{rename_seq}");
                rename_map.insert(name.to_string(), new_name);
            }
        }
    }

    let bool_false_id = {
        let mut found = None;
        for (i, c) in module.constants().iter().enumerate() {
            if matches!(c, Constant::Bool(false)) {
                found = Some(ConstantId(i as u32));
                break;
            }
        }
        found.unwrap_or_else(|| module.add_constant(Constant::Bool(false)))
    };

    let mut region_clones: Vec<BasicBlock> = Vec::with_capacity(region_blocks.len());
    let mut phi_to_inject = phi_opt;
    for &orig in &region_blocks {
        let clone_id = block_map[&orig];
        let mut clone = BasicBlock::new(clone_id);
        let clone_instruction = |ins: &Instruction, clone: &mut BasicBlock| {
            // 消费区中的其他调用点若已被推测内联（克隆自含 GuardSameFunction
            // 的块），其快路径克隆绑定原始调用点的 SSA 参数，克隆体路径上
            // 未定义——降级为恒 false（走原始慢路径块，参数有效）。
            if let Instruction::GuardSameFunction { dest, .. } = ins {
                clone.push_instruction(Instruction::Const {
                    dest: remap_region_value(*dest),
                    constant: bool_false_id,
                });
                return;
            }
            let mut ins = ins.clone();
            ins.remap_values(&mut |v| remap_region_value(v));
            ins.remap_blocks(&mut |b: BasicBlockId| block_map.get(&b).copied().unwrap_or(b));
            clone.push_instruction(ins);
        };
        if orig == call_block_id {
            // 入口块头部注入多返回汇聚 Phi
            if let Some((phi_dest, phi_sources)) = phi_to_inject.take() {
                clone.push_instruction(Instruction::Phi {
                    dest: phi_dest,
                    sources: phi_sources,
                });
            }
            // 调用块尾部：调用后指令 + 原终止器。
            for ins in &post_instructions {
                clone_instruction(ins, &mut clone);
            }
            let mut term = orig_terminator.clone();
            term.remap_values(&mut |v| remap_region_value(v));
            term.remap_blocks(&mut |b: BasicBlockId| block_map.get(&b).copied().unwrap_or(b));
            clone.set_terminator(term);
        } else {
            let src = &module.functions()[func_idx].blocks()[orig.0 as usize];
            for ins in src.instructions() {
                clone_instruction(ins, &mut clone);
            }
            let mut term = src.terminator().clone();
            term.remap_values(&mut |v| remap_region_value(v));
            term.remap_blocks(&mut |b: BasicBlockId| block_map.get(&b).copied().unwrap_or(b));
            clone.set_terminator(term);
        }
        region_clones.push(clone);
    }
    // 变量改名（克隆体独立于慢路径变量）。
    for (old_name, new_name) in &rename_map {
        rename_var_in_blocks(&mut region_clones, old_name, new_name);
    }

    // 克隆体 max ValueId（记账）。
    let mut clone_max = ret_mapped.0;
    for block in callee_clones.iter().chain(region_clones.iter()) {
        for ins in block.instructions() {
            if let Some(d) = instruction_dest(ins) {
                clone_max = clone_max.max(d.0);
            }
            for used in instr_uses(ins) {
                clone_max = clone_max.max(used.0);
            }
        }
        for used in terminator_uses(block.terminator()) {
            clone_max = clone_max.max(used.0);
        }
    }
    current_max_value[func_idx] = guard_dest.0.max(undefined_dest.0).max(clone_max);

    // ── 4. 追加全部克隆块 + 应用替换 ──
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        for clone in callee_clones {
            caller.push_block(clone);
        }
        for clone in region_clones {
            caller.push_block(clone);
        }
        // callee 参数替换（mapped LoadVar dest 只存在于 callee 克隆，全函数替换安全）。
        for (old_val, new_val) in &param_subst {
            replace_all_uses_of(caller, *old_val, *new_val);
        }
    }

    // ── 5. B_pre 重写：调用前指令 + undefined + 注入初始值 + 守卫；
    //       branch %g, fast_entry, B_slow ──
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        let b_pre = &mut caller.blocks_mut()[block_idx];
        *b_pre.instructions_mut() = pre_instructions;
        b_pre.push_instruction(Instruction::Const {
            dest: undefined_dest,
            constant: undefined_id,
        });
        for (name, value) in &inject_subst {
            b_pre.push_instruction(Instruction::StoreVar {
                name: name.clone(),
                value: *value,
            });
        }
        b_pre.push_instruction(Instruction::GuardSameFunction {
            dest: guard_dest,
            callee: candidate.guard_callee,
            function: candidate.target,
        });
        b_pre.set_terminator(Terminator::Branch {
            condition: guard_dest,
            true_block: fast_entry,
            false_block: b_slow_id,
        });
    }

    // 快路径区域中 R 的 use 已替换为 ret_mapped（克隆时完成）。
    *rename_seq += rename_map.len() as u32;
}

/// 运行 inline_for_ea pass。
///
/// 顺序：阶段 A（静态多块内联）+ cfg_fold 迭代至不动点 → 阶段 C（守卫式推测
/// 方法内联）→ 若阶段 C 发生内联则级联触发阶段 A + cfg_fold → 终轮 cfg_fold。
pub(crate) fn run(module: &mut Module) {
    // 全局守卫：eval 可动态变动绑定，禁用整个 pass。
    if module.functions().iter().any(|f| f.has_eval()) {
        return;
    }

    // 阶段 A 与 cfg_fold 迭代至不动点（≤4 轮）。
    let mut round = 0;
    loop {
        round += 1;
        let inlined = static_inline_round(module);
        cfg_fold::run(module);
        if !inlined || round >= 4 {
            break;
        }
    }

    // 阶段 C：守卫式推测方法内联。
    let spec_inlined = speculative_inline_round(module);
    cfg_fold::run(module);

    // 阶段 C 内联展开的方法体中可能暴露出新的 direct_call / 构造器调用，级联触发阶段 A
    if spec_inlined {
        let mut post_round = 0;
        loop {
            post_round += 1;
            let inlined = static_inline_round(module);
            cfg_fold::run(module);
            if !inlined || post_round >= 4 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{
        BasicBlock, BasicBlockId, BinaryOp, CompareOp, Constant, ConstantId, Function, FunctionId,
        Instruction, Module, Terminator, UnaryOp, ValueId,
    };

    /// 构造标准方法调用者 `$main`：单块依次 NewObject、取方法 `name`、实参 `arg`、
    /// Call、Return 调用结果。此形状触发守卫式推测方法内联站点。
    fn caller_with_method_call(module: &mut Module, name: &str, arg: ConstantId) -> Function {
        let c_name = module.add_constant(Constant::String(name.to_string()));
        let mut caller = Function::new("$main", BasicBlockId(0));
        caller.set_params(vec!["$env".to_string(), "$this".to_string()]);
        let mut bb = BasicBlock::new(BasicBlockId(0));
        let v_obj = ValueId(0);
        let v_key = ValueId(1);
        let v_callee = ValueId(2);
        let v_arg = ValueId(3);
        let v_res = ValueId(4);
        bb.push_instruction(Instruction::NewObject {
            dest: v_obj,
            capacity: 4,
        });
        bb.push_instruction(Instruction::Const {
            dest: v_key,
            constant: c_name,
        });
        bb.push_instruction(Instruction::GetProp {
            dest: v_callee,
            object: v_obj,
            key: v_key,
        });
        bb.push_instruction(Instruction::Const {
            dest: v_arg,
            constant: arg,
        });
        bb.push_instruction(Instruction::Call {
            dest: Some(v_res),
            callee: v_callee,
            this_val: v_obj,
            args: vec![v_arg],
        });
        bb.set_terminator(Terminator::Return { value: Some(v_res) });
        caller.push_block(bb);
        caller
    }

    /// 断言 caller 快路径内含指向 `target` 方法的 GuardSameFunction 守卫。
    fn assert_has_guard(caller: &Function, target: FunctionId) {
        let has_guard = caller.blocks().iter().any(|b| {
            b.instructions().iter().any(|ins| {
                matches!(
                    ins,
                    Instruction::GuardSameFunction {
                        function: fn_id,
                        ..
                    } if *fn_id == target
                )
            })
        });
        assert!(
            has_guard,
            "caller must have GuardSameFunction for target function {target:?}"
        );
    }

    #[test]
    fn test_classify_construct_return_primitives() {
        let mut defs = HashMap::new();
        let constants = vec![
            Constant::Number(42.0),
            Constant::String("hello".to_string()),
            Constant::Bool(true),
            Constant::Null,
            Constant::Undefined,
        ];

        let v_bin = ValueId(1);
        defs.insert(
            v_bin,
            Instruction::Binary {
                dest: v_bin,
                op: BinaryOp::Add,
                lhs: ValueId(10),
                rhs: ValueId(11),
            },
        );

        let v_un = ValueId(2);
        defs.insert(
            v_un,
            Instruction::Unary {
                dest: v_un,
                op: UnaryOp::Not,
                value: ValueId(12),
            },
        );

        let v_cmp = ValueId(3);
        defs.insert(
            v_cmp,
            Instruction::Compare {
                dest: v_cmp,
                op: CompareOp::StrictEq,
                lhs: ValueId(13),
                rhs: ValueId(14),
            },
        );

        let v_str = ValueId(4);
        defs.insert(
            v_str,
            Instruction::StringConcatVa {
                dest: v_str,
                parts: vec![ValueId(15), ValueId(16)],
            },
        );

        let v_obj = ValueId(5);
        defs.insert(
            v_obj,
            Instruction::NewObject {
                dest: v_obj,
                capacity: 4,
            },
        );

        let v_arr = ValueId(6);
        defs.insert(
            v_arr,
            Instruction::NewArray {
                dest: v_arr,
                capacity: 4,
            },
        );

        let v_tmpl = ValueId(7);
        defs.insert(
            v_tmpl,
            Instruction::CloneArrayTemplate {
                dest: v_tmpl,
                template: ConstantId(0),
            },
        );

        let v_this = ValueId(8);
        defs.insert(
            v_this,
            Instruction::LoadVar {
                dest: v_this,
                name: "$this".to_string(),
            },
        );

        // 原始值表达式均归类为 Some(false)（即返回 $this）
        assert_eq!(
            classify_construct_return(&defs, &constants, v_bin),
            Some(false)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_un),
            Some(false)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_cmp),
            Some(false)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_str),
            Some(false)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_this),
            Some(false)
        );

        // 对象/数组分配归类为 Some(true)
        assert_eq!(
            classify_construct_return(&defs, &constants, v_obj),
            Some(true)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_arr),
            Some(true)
        );
        assert_eq!(
            classify_construct_return(&defs, &constants, v_tmpl),
            Some(true)
        );
    }

    #[test]
    fn test_speculative_inline_multi_return() {
        let mut module = Module::new();

        // 常量表
        let c_10 = module.add_constant(Constant::Number(10.0));
        let c_20 = module.add_constant(Constant::Number(20.0));

        // 函数 0：目标方法 `Point.calc`，含两个返回块
        let mut target_func = Function::new("Point.calc", BasicBlockId(0));
        target_func.set_params(vec![
            "$env".to_string(),
            "$this".to_string(),
            "cond".to_string(),
        ]);
        target_func.set_direct_callable(true);

        // bb0: load cond -> branch
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        let v_cond = ValueId(0);
        bb0.push_instruction(Instruction::LoadVar {
            dest: v_cond,
            name: "cond".to_string(),
        });
        bb0.set_terminator(Terminator::Branch {
            condition: v_cond,
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        target_func.push_block(bb0);

        // bb1: return 10
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        let v_10 = ValueId(1);
        bb1.push_instruction(Instruction::Const {
            dest: v_10,
            constant: c_10,
        });
        bb1.set_terminator(Terminator::Return { value: Some(v_10) });
        target_func.push_block(bb1);

        // bb2: return 20
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        let v_20 = ValueId(2);
        bb2.push_instruction(Instruction::Const {
            dest: v_20,
            constant: c_20,
        });
        bb2.set_terminator(Terminator::Return { value: Some(v_20) });
        target_func.push_block(bb2);

        module.push_function(target_func);

        // 函数 1：caller
        let caller_func = caller_with_method_call(&mut module, "calc", c_10);
        module.push_function(caller_func);

        // 运行 inline_for_ea
        run(&mut module);

        let caller = &module.functions()[1];
        // 验证 caller 包含 GuardSameFunction 守卫
        assert_has_guard(caller, FunctionId(0));

        // 验证区域入口块包含汇合多返回分支的 Phi 指令
        let has_phi = caller.blocks().iter().any(|b| {
            b.instructions().iter().any(|ins| {
                if let Instruction::Phi { sources, .. } = ins {
                    sources.len() == 2
                } else {
                    false
                }
            })
        });
        assert!(
            has_phi,
            "caller must contain a 2-source Phi instruction for multi-return merge"
        );
    }

    #[test]
    fn test_speculative_inline_chained_with_static() {
        let mut module = Module::new();

        let c_fn0_ref = module.add_constant(Constant::FunctionRef(FunctionId(0)));
        let c_1 = module.add_constant(Constant::Number(1.0));
        let c_42 = module.add_constant(Constant::Number(42.0));

        // 函数 0：helper 函数 `add_one`
        let mut helper_func = Function::new("add_one", BasicBlockId(0));
        helper_func.set_params(vec![
            "$env".to_string(),
            "$this".to_string(),
            "x".to_string(),
        ]);
        helper_func.set_direct_callable(true);
        let mut h_bb0 = BasicBlock::new(BasicBlockId(0));
        let v_x = ValueId(0);
        let v_one = ValueId(1);
        let v_sum = ValueId(2);
        h_bb0.push_instruction(Instruction::LoadVar {
            dest: v_x,
            name: "x".to_string(),
        });
        h_bb0.push_instruction(Instruction::Const {
            dest: v_one,
            constant: c_1,
        });
        h_bb0.push_instruction(Instruction::Binary {
            dest: v_sum,
            op: BinaryOp::Add,
            lhs: v_x,
            rhs: v_one,
        });
        h_bb0.set_terminator(Terminator::Return { value: Some(v_sum) });
        helper_func.push_block(h_bb0);
        module.push_function(helper_func);

        // 函数 1：方法 `Point.calc`，调用 `add_one`
        let mut method_func = Function::new("Point.calc", BasicBlockId(0));
        method_func.set_params(vec![
            "$env".to_string(),
            "$this".to_string(),
            "n".to_string(),
        ]);
        method_func.set_direct_callable(true);
        let mut m_bb0 = BasicBlock::new(BasicBlockId(0));
        let v_fn0 = ValueId(0);
        let v_n = ValueId(1);
        let v_this = ValueId(2);
        let v_call_res = ValueId(3);
        m_bb0.push_instruction(Instruction::Const {
            dest: v_fn0,
            constant: c_fn0_ref,
        });
        m_bb0.push_instruction(Instruction::LoadVar {
            dest: v_n,
            name: "n".to_string(),
        });
        m_bb0.push_instruction(Instruction::LoadVar {
            dest: v_this,
            name: "$this".to_string(),
        });
        m_bb0.push_instruction(Instruction::Call {
            dest: Some(v_call_res),
            callee: v_fn0,
            this_val: v_this,
            args: vec![v_n],
        });
        m_bb0.set_terminator(Terminator::Return {
            value: Some(v_call_res),
        });
        method_func.push_block(m_bb0);
        module.push_function(method_func);

        // 函数 2：caller
        let caller_func = caller_with_method_call(&mut module, "calc", c_42);
        module.push_function(caller_func);

        // 运行 inline_for_ea
        run(&mut module);

        let caller = &module.functions()[2];
        // 验证 caller 包含 GuardSameFunction
        assert_has_guard(caller, FunctionId(1));

        // 验证快路径中 helper 调用也被级联内联为 Binary Add，无残留直接 Call
        let has_binary_add = caller.blocks().iter().any(|b| {
            b.instructions().iter().any(|ins| {
                matches!(
                    ins,
                    Instruction::Binary {
                        op: BinaryOp::Add,
                        ..
                    }
                )
            })
        });
        assert!(
            has_binary_add,
            "cascaded stage A must inline the helper into a binary add"
        );
    }
}
