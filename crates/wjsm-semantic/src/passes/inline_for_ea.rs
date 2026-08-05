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

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlock, BasicBlockId, Constant, ConstantId, FunctionId, Instruction, Module, Terminator,
    ValueId,
};

use super::cfg_fold::{self, terminator_successors};
use crate::passes::direct_call::{instr_uses, instruction_dest, terminator_uses};

/// 计算函数内最大的 ValueId。
fn max_value_id_in_function(function: &wjsm_ir::Function) -> u32 {
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
        CallBuiltin { dest, args, builtin: _ } => {
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
        SetProp { object, key, value } => {
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
        GetElem { dest, object, index } => {
            add(dest);
            add(object);
            add(index);
        }
        SetElem { object, index, value } => {
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
        ObjectSpread { dest, source } => {
            add(dest);
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
        CallBuiltin { dest, args, builtin: _ } => {
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
        SetProp { object, key, value } => {
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
        GetElem { dest, object, index } => {
            rep(dest);
            rep(object);
            rep(index);
        }
        SetElem { object, index, value } => {
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
        ObjectSpread { dest, source } => {
            rep(dest);
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

/// 收集函数内全部 `$this` 族 LoadVar 的 dest。
fn collect_this_dests(function: &wjsm_ir::Function) -> Vec<ValueId> {
    let mut dests = Vec::new();
    for block in function.blocks() {
        for ins in block.instructions() {
            if let Instruction::LoadVar { dest, name } = ins {
                if is_this_name(name) {
                    dests.push(*dest);
                }
            }
        }
    }
    dests
}

/// 查找或追加 `Constant::Undefined` 常量。
fn undefined_const_id(module: &mut Module) -> ConstantId {
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
}

/// 阶段 A：静态多块内联的一轮。返回是否发生了任何内联。
fn static_inline_round(module: &mut Module) -> bool {
    // ── 预收集（不可变借用阶段）──
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
                let callee_id = match per_func_defs[func_idx].get(callee) {
                    Some(Instruction::Const { constant, .. }) => {
                        match constants_snapshot.get(constant.0 as usize) {
                            Some(Constant::FunctionRef(f)) => *f,
                            _ => continue,
                        }
                    }
                    _ => continue,
                };
                let callee_idx = callee_id.0 as usize;
                if callee_idx >= per_func_info.len() {
                    continue;
                }
                let (direct_callable, has_eval, num_blocks) = per_func_info[callee_idx];
                if !direct_callable || has_eval || num_blocks == 0 || callee_idx == func_idx {
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
                if is_construct {
                    // 构造器附加条件：所有 Return 必须返回 $this（或空 return）。
                    // 显式返回其他对象/原语具有特殊 `new` 语义，保守跳过。
                    let this_dests = collect_this_dests(callee_func);
                    let returns_this = callee_func.blocks().iter().all(|b| match b.terminator() {
                        Terminator::Return { value: None } => true,
                        Terminator::Return { value: Some(v) } => this_dests.contains(v),
                        _ => true,
                    });
                    if !returns_this {
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
        candidate
            .dest
            .and_then(|d| find_exception_path(&module.functions()[func_idx], block_idx, instr_idx, d))
    } else {
        None
    };
    let mut return_records: Vec<(BasicBlockId, ValueId)> = Vec::new();
    for clone in cloned_blocks.iter_mut() {
        let term = clone.terminator().clone();
        match term {
            Terminator::Return { value } => {
                if !candidate.is_construct {
                    let mapped = match value {
                        Some(v) => v,
                        None => undefined_dest,
                    };
                    return_records.push((clone.id(), mapped));
                }
                clone.set_terminator(Terminator::Jump { target: b_post_id });
            }
            Terminator::Throw { value } => {
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
        b_pre.set_terminator(Terminator::Jump { target: clone_entry });
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
        if candidate.is_construct {
            // 构造结果 == this（候选条件保证）。
            if let Some(call_dest) = candidate.dest {
                replace_all_uses_of(caller, call_dest, candidate.this_val);
            }
        } else if let Some(call_dest) = candidate.dest {
            match return_records.len() {
                1 => {
                    replace_all_uses_of(caller, call_dest, resolve_param(return_records[0].1));
                }
                n if n >= 2 => {
                    // 多返回点：B_post 头部插入 phi。
                    let b_post = &mut caller.blocks_mut()[b_post_id.0 as usize];
                    let mut sources = Vec::with_capacity(n);
                    for (pred, value) in &return_records {
                        sources.push(wjsm_ir::PhiSource {
                            predecessor: *pred,
                            value: resolve_param(*value),
                        });
                    }
                    b_post
                        .instructions_mut()
                        .insert(0, Instruction::Phi { dest: call_dest, sources });
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
fn find_exception_path(
    function: &wjsm_ir::Function,
    block_idx: usize,
    instr_idx: usize,
    call_dest: ValueId,
) -> Option<(String, BasicBlockId)> {
    let block = &function.blocks()[block_idx];
    // 调用点之后的指令中找 `is_exception(call_dest)`。
    let mut exc_dest = None;
    for ins in block.instructions().iter().skip(instr_idx + 1) {
        if let Instruction::IsException { dest, value } = ins {
            if *value == call_dest {
                exc_dest = Some(*dest);
                break;
            }
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
    function.blocks().iter().any(|b| {
        b.id().0 > t.0 && terminator_successors(b.terminator()).contains(&t)
    })
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
                if uses_closure {
                    if let Some(d) = instruction_dest(ins) {
                        if closure.insert(d) {
                            changed = true;
                        }
                    }
                }
                if let Instruction::StoreVar { name, value } = ins {
                    if closure.contains(value) {
                        for block2 in function.blocks() {
                            for ins2 in block2.instructions() {
                                if let Instruction::LoadVar { dest: d, name: n } = ins2 {
                                    if n == name && closure.insert(*d) {
                                        changed = true;
                                    }
                                }
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
        let contains_region_instr = block.instructions().iter().any(|ins| {
            instruction_dest(ins).is_some_and(|d| closure.contains(&d))
        });
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

    // ── 出口检查：S 内终止器指向 S 外目标时，目标块指令/终止器不得使用 T 值 ──
    let s_set: HashSet<BasicBlockId> = s_blocks.iter().copied().collect();
    for &bid in &s_blocks {
        let block = match function.block_by_id(bid) {
            Some(b) => b,
            None => continue,
        };
        for succ in terminator_successors(block.terminator()) {
            if !s_set.contains(&succ) {
                let target = match function.block_by_id(succ) {
                    Some(b) => b,
                    None => continue,
                };
                let uses_t = target.instructions().iter().any(|ins| {
                    instr_uses(ins).iter().any(|u| closure.contains(u))
                }) || terminator_uses(target.terminator())
                    .iter()
                    .any(|u| closure.contains(u));
                if uses_t {
                    return None;
                }
            }
        }
    }

    Some((closure, s_blocks))
}

/// 在指定块集合内重命名变量（LoadVar/StoreVar 的 name 字段）。
fn rename_var_in_blocks(blocks: &mut [BasicBlock], old_name: &str, new_name: &str) {
    for block in blocks {
        for ins in block.instructions_mut() {
            match ins {
                Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => {
                    if name == old_name {
                        *name = new_name.to_string();
                    }
                }
                _ => {}
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
                // 全函数恰 1 个 Return。
                let return_count = target_func
                    .blocks()
                    .iter()
                    .filter(|b| matches!(b.terminator(), Terminator::Return { .. }))
                    .count();
                if return_count != 1 {
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
                    && cand
                        .region_blocks
                        .contains(&BasicBlockId(other.block_idx))
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
    //      cfg 状态机，状态机编译部分形态有栈不平衡缺陷（wasm 校验 values
    //      remaining）→ 保守跳过该调用点。
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
    let region_entry =
        BasicBlockId(module.functions()[func_idx].blocks().len() as u32 + target_func.blocks().len() as u32);

    // 快路径 callee 的 Throw（语句级异常传播）改写为「存异常到调用点的 catch
    // 变量 + 跳转 catch 处理块」，否则 throw 编译为 CreateException+Return
    // 直接从调用者函数返回，绕过语句级 is_exception 分叉（try/catch 失效）。
    let exception_path =
        find_exception_path(&module.functions()[func_idx], block_idx, instr_idx, candidate.dest);

    // 参数替换映射（callee params 约定 [$env, $this, ...args]）：
    // 参数 LoadVar 是纯参数读取，克隆时跳过并记录 use 替换（同阶段 A，见
    // inline_static_candidate——保留后替换 dest 会破坏 SSA）。
    let callee_params = target_func.params().to_vec();
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
    let mut ret_mapped = undefined_dest;
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
                // 唯一 Return → Jump(区域入口)；记录返回值（None → undefined）。
                if let Some(v) = value {
                    ret_mapped = v;
                }
                clone.set_terminator(Terminator::Jump { target: region_entry });
            }
            Terminator::Throw { value } => {
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
    let mut remap_block = |b: BasicBlockId| block_map.get(&b).copied().unwrap_or(b);
    let mapped_dest = ValueId(candidate.dest.0 + value_offset);
    // 返回值可能来自参数 LoadVar 的 dest（如 `return x`），经 param_subst 解析
    // 为实参值，否则区域克隆中 R 的 use 会指向悬空值。
    if let Some((_, resolved)) = param_subst
        .iter()
        .find(|(old, _)| *old == ret_mapped)
    {
        ret_mapped = *resolved;
    }

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
                    Instruction::LoadVar { name: n, .. } | Instruction::StoreVar { name: n, .. } => {
                        n != name || s_set.contains(&b.id())
                    }
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
    for &orig in &region_blocks {
        let clone_id = block_map[&orig];
        let mut clone = BasicBlock::new(clone_id);
        let clone_instruction = |ins: &Instruction, clone: &mut BasicBlock| {
            // 消费区中的其他调用点若已被推测内联（克隆自含 GuardSameFunction
            // 的块），其快路径克隆绑定原始调用点的 SSA 参数，克隆体路径上
            // 未定义——降级为恒 false（走原始慢路径块，参数有效）。
            if let Instruction::GuardSameFunction { dest, .. } = ins {
                clone.push_instruction(Instruction::Const {
                    dest: ValueId(dest.0 + value_offset),
                    constant: bool_false_id,
                });
                return;
            }
            let mut ins = ins.clone();
            add_offset_to_value_id(&mut ins, value_offset);
            replace_value_id(&mut ins, mapped_dest, ret_mapped);
            clone.push_instruction(ins);
        };
        if orig == call_block_id {
            // 调用块尾部：调用后指令 + 原终止器。
            for ins in &post_instructions {
                clone_instruction(ins, &mut clone);
            }
            let mut term = orig_terminator.clone();
            term.remap_values(&mut |v| ValueId(v.0 + value_offset));
            term.remap_blocks(&mut remap_block);
            replace_value_id_in_terminator(&mut term, mapped_dest, ret_mapped);
            clone.set_terminator(term);
        } else {
            let src = &module.functions()[func_idx].blocks()[orig.0 as usize];
            for ins in src.instructions() {
                clone_instruction(ins, &mut clone);
            }
            let mut term = src.terminator().clone();
            term.remap_values(&mut |v| ValueId(v.0 + value_offset));
            term.remap_blocks(&mut remap_block);
            replace_value_id_in_terminator(&mut term, mapped_dest, ret_mapped);
            clone.set_terminator(term);
        }
        region_clones.push(clone);
    }
    // 变量改名（克隆体独立于慢路径变量）。
    for (old_name, new_name) in &rename_map {
        rename_var_in_blocks(&mut region_clones, old_name, new_name);
    }

    // 克隆体 max ValueId（记账）。
    let mut clone_max = 0u32;
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
/// 方法内联）→ 终轮 cfg_fold。
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

    // 阶段 C：守卫式推测方法内联（单次执行，Step 4 实现）。
    speculative_inline_round(module);
    cfg_fold::run(module);
}
