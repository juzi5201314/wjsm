//! inline_for_ea pass：构造器体 + 单点直调函数内联 + IsJsObject 常量折叠。
//!
//! 本 pass 在 direct_call pass 之后运行，利用已标记的 `direct_callable` 函数：
//!
//! - **构造器体内联**：遇到 `FunctionRef(ctor_id)` 的 `ConstructCall` 且构造器
//!   是 direct_callable 且为单 block 时，将构造器指令序列原地展开到调用处，替换
//!   `LoadVar` 参数名为实际参数值，消除构造器调用开销。
//!
//! - **方法内联**：遇到 `Call` 指令且 callee 是 `GetProp { object, key }` 的结果，
//!   其中 object 等于 this_val、key 为常量字符串、且该名称对应一个 direct_callable
//!   函数时，将方法体内联到调用点。
//!
//! - **IsJsObject 常量折叠**：`CallBuiltin { builtin: IsJsObject, args: [v] }`
//!   当 v 是 `NewObject` 指令的 dest 时，替换为 `Const { Bool(true) }`。

use std::collections::HashMap;

use wjsm_ir::{
    Builtin, Constant, ConstantId, FunctionId, Instruction, Module, Terminator, ValueId,
};

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

/// 计算一组指令中的最大 ValueId（克隆体偏移递增用）。
fn max_value_id_in_instructions(instructions: &[Instruction]) -> u32 {
    let mut max = 0u32;
    for instruction in instructions {
        if let Some(dest) = instruction_dest(instruction) {
            max = max.max(dest.0);
        }
        for used in instr_uses(instruction) {
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
        Const { .. } => {}
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
fn replace_value_id(ins: &mut Instruction, old_val: ValueId, new_val: ValueId) {
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
fn replace_value_id_in_terminator(terminator: &mut Terminator, old_val: ValueId, new_val: ValueId) {
    match terminator {
        Terminator::Return { value: Some(v) } if *v == old_val => *v = new_val,
        Terminator::Branch { condition, .. } if *condition == old_val => *condition = new_val,
        Terminator::Switch { value, .. } if *value == old_val => *value = new_val,
        Terminator::Throw { value } if *value == old_val => *value = new_val,
        _ => {}
    }
}

/// 在函数中，将 `old_val` 的所有引用替换为 `new_val`。
fn replace_all_uses_of(
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

/// 收集并替换所有 CallBuiltin IsJsObject 指令，其参数是 NewObject 的 dest。
fn fold_is_js_object_in_block(
    block: &mut wjsm_ir::BasicBlock,
    new_object_dests: &[ValueId],
    const_true_id: ConstantId,
) {
    let mut fold_sites: Vec<usize> = Vec::new();
    for (i, instr) in block.instructions().iter().enumerate() {
        if let Instruction::CallBuiltin {
            dest: Some(_),
            builtin: Builtin::IsJsObject,
            args,
        } = instr
        {
            if args.len() == 1 && new_object_dests.contains(&args[0]) {
                fold_sites.push(i);
            }
        }
    }
    // 从后往前替换，避免索引偏移
    for &i in fold_sites.iter().rev() {
        let instr = &mut block.instructions_mut()[i];
        if let Instruction::CallBuiltin {
            dest: Some(dest), ..
        } = instr
        {
            *instr = Instruction::Const {
                dest: *dest,
                constant: const_true_id,
            };
        }
    }
}

/// 运行 inline_for_ea pass。
///
/// 顺序：构造器内联 → IsJsObject 常量折叠。
pub(crate) fn run(module: &mut Module) {
    // 1. 全局守卫：eval 可动态变动绑定，禁用整个 pass。
    if module.functions().iter().any(|f| f.has_eval()) {
        return;
    }

    // 2. 预先计算每个函数的 max_value_id（不能在可变借用中使用）
    let per_func_max_value: Vec<u32> = module
        .functions()
        .iter()
        .map(|f| max_value_id_in_function(f))
        .collect();

    // 预收集每个函数中的常量字符串表（key 的 ValueId → 字符串值）
    // 用于方法内联时判断 GetProp key 是否为常量字符串及匹配方法名
    let _per_func_const_strings: Vec<HashMap<ValueId, String>> = module
        .functions()
        .iter()
        .map(|f| {
            let mut strings = HashMap::new();
            for block in f.blocks() {
                for instr in block.instructions() {
                    if let Instruction::Const { dest, constant } = instr
                        && let Some(Constant::String(s)) =
                            module.constants().get(constant.0 as usize)
                    {
                        strings.insert(*dest, s.clone());
                    }
                }
            }
            strings
        })
        .collect();

    // 收集所有函数名 → FunctionId 映射（用于方法名查找）
    let _fn_name_to_id: HashMap<&str, FunctionId> = module
        .functions()
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name(), FunctionId(i as u32)))
        .collect();

    // 预收集每个函数中 ValueId → 产生该 ValueId 的指令（def 表）
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

    // 预收集函数信息（避免在迭代中同时借用 immutable 和 mutable）
    let per_func_info: Vec<(bool, bool, usize)> = module
        .functions()
        .iter()
        .map(|f| (f.direct_callable(), f.has_eval(), f.blocks().len()))
        .collect();

    // ================================================================
    // 阶段 A：构造器体内联
    // ================================================================
    // 收集构造器内联候选：(func_idx, block_idx, instr_idx, ctor_id, this_val, args, dest)
    let mut ctor_candidates: Vec<(
        u32,
        u32,
        usize,
        FunctionId,
        ValueId,
        Vec<ValueId>,
        Option<ValueId>,
    )> = Vec::new();

    for (func_idx, function) in module.functions().iter().enumerate() {
        for (block_idx, block) in function.blocks().iter().enumerate() {
            for (instr_idx, instr) in block.instructions().iter().enumerate() {
                if let Instruction::ConstructCall {
                    dest,
                    callee,
                    this_val,
                    args,
                } = instr
                {
                    let defs = &per_func_defs[func_idx];
                    if let Some(Instruction::Const { constant, .. }) = defs.get(callee) {
                        if let Some(Constant::FunctionRef(ctor_id)) =
                            module.constants().get(constant.0 as usize)
                        {
                            let ctor_idx = ctor_id.0 as usize;
                            if ctor_idx < per_func_info.len() {
                                let (direct_callable, has_eval, num_blocks) =
                                    per_func_info[ctor_idx];
                                if direct_callable && num_blocks == 1 && !has_eval {
                                    ctor_candidates.push((
                                        func_idx as u32,
                                        block_idx as u32,
                                        instr_idx,
                                        *ctor_id,
                                        *this_val,
                                        args.clone(),
                                        *dest,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 执行构造器内联：按 (func, block, instr) 逆序处理，保证后续插入
    // 不会使先前候选的插入位置偏移；每个调用者函数跟踪当前最大 ValueId，
    // 每次内联后递增，避免多个构造器内联进同一函数时 ValueId 冲突。
    let mut current_max_value: Vec<u32> = per_func_max_value.clone();
    let mut ctor_candidates = ctor_candidates;
    ctor_candidates.sort_by_key(|(f, b, i, ..)| (*f, *b, *i));
    ctor_candidates.reverse();

    for (func_idx, block_idx, instr_idx, ctor_id, this_val, args, dest) in ctor_candidates {
        let ctor_func = match module.function_mut(ctor_id) {
            Some(f) => f.clone(),
            None => continue,
        };

        let ctor_block = match ctor_func.blocks().first() {
            Some(b) => b.clone(),
            None => continue,
        };

        let return_value = match &ctor_block.terminator() {
            Terminator::Return { value: Some(v) } => *v,
            _ => continue,
        };

        // 仅内联「返回 $this」的构造器。构造器显式返回其他对象/原语时具有
        // 特殊 `new` 语义（返回值替换 this），内联后调用者的 phi 合并来源
        // （返回值 vs 原始 this）不一致，会得到错误结果——保守跳过。
        let this_dest = ctor_block
            .instructions()
            .iter()
            .find_map(|ins| match ins {
                Instruction::LoadVar { dest, name } if name == "$this" || name.ends_with(".$this") => {
                    Some(*dest)
                }
                _ => None,
            });
        let returns_this = match this_dest {
            Some(this_dest) => return_value == this_dest,
            // 无 $this LoadVar 的构造器（不可能出现）保守跳过
            None => false,
        };
        if !returns_this {
            continue;
        }

        let value_offset = current_max_value[func_idx as usize] + 1;

        // 克隆构造器的指令（不含 terminator），并添加 ValueId 偏移
        let mut cloned_instructions: Vec<Instruction> = ctor_block
            .instructions()
            .iter()
            .cloned()
            .collect();

        // 应用 ValueId 偏移
        for ins in cloned_instructions.iter_mut() {
            add_offset_to_value_id(ins, value_offset);
        }

        // 内联后调用者函数的最大 ValueId 增大：偏移量必须覆盖克隆指令
        // 中的所有 ValueId，确保下一次内联使用更大的偏移。
        current_max_value[func_idx as usize] =
            value_offset + max_value_id_in_instructions(&cloned_instructions);

        // 构建参数替换映射：(cloned LoadVar dest → 实际参数值)
        // 构造器 params 约定为: [$env, $this, ...args]
        let ctor_params = ctor_func.params().to_vec();
        let mut param_subst: Vec<(ValueId, ValueId)> = Vec::new();

        for (_ci, ctor_instr) in ctor_block.instructions().iter().enumerate() {
            if let Instruction::LoadVar { dest, name } = ctor_instr {
                let mapped_dest = ValueId(dest.0 + value_offset);
                if name == "$this" || name.ends_with(".$this") {
                    param_subst.push((mapped_dest, this_val));
                } else if !(name == "$env" || name.ends_with(".$env")) {
                    // 普通参数
                    if let Some((param_idx, _)) = ctor_params
                        .iter()
                        .enumerate()
                        .find(|(_, p)| p.as_str() == name)
                    {
                        if param_idx >= 2 {
                            let arg_idx = param_idx - 2;
                            if arg_idx < args.len() {
                                param_subst.push((mapped_dest, args[arg_idx]));
                            }
                        }
                    }
                }
            }
        }

        // 插入所有克隆指令到调用者 block（在 ConstructCall 之后）
        // 包括 LoadVar 指令（它们会被参数替换覆盖，留下死代码但不影响正确性）
        let insert_pos = instr_idx + 1;
        for (i, ins) in cloned_instructions.into_iter().enumerate() {
            let func = module
                .function_mut(FunctionId(func_idx))
                .expect("caller function must exist");
            let block = func
                .blocks_mut()
                .get_mut(block_idx as usize)
                .expect("caller block must exist");
            block.instructions_mut().insert(insert_pos + i, ins);
        }

        // 返回值的偏移后 ValueId
        let ret_value_mapped = ValueId(return_value.0 + value_offset);

        let caller_func = module
            .function_mut(FunctionId(func_idx))
            .expect("caller function must exist after inlining");

        // 应用参数替换：将 LoadVar 的 dest 的所有 use 替换为实际参数值
        for (old_val, new_val) in &param_subst {
            replace_all_uses_of(caller_func, *old_val, *new_val);
        }

        // 替换所有对 ConstructCall dest 的引用为返回值的偏移后 ValueId
        if let Some(call_dest) = dest {
            replace_all_uses_of(caller_func, call_dest, ret_value_mapped);
        }

        // 删除 ConstructCall 指令
        let caller_block = caller_func
            .blocks_mut()
            .get_mut(block_idx as usize)
            .expect("caller block must exist");
        caller_block.instructions_mut().remove(instr_idx);
    }

    // ================================================================
    // 阶段 B：IsJsObject 常量折叠
    // ================================================================
    // 先检查是否有需要折叠的 IsJsObject，避免不必要的常量添加
    let mut has_foldable = false;
    'find_foldable: for func in module.functions() {
        for block in func.blocks() {
            for instr in block.instructions() {
                if let Instruction::CallBuiltin {
                    dest: Some(_),
                    builtin: Builtin::IsJsObject,
                    args,
                } = instr
                {
                    // 检查 args[0] 是否在 NewObject dest 中
                    if args.len() == 1 {
                        for block2 in func.blocks() {
                            for instr2 in block2.instructions() {
                                if let Instruction::NewObject { dest, .. } = instr2 {
                                    if *dest == args[0] {
                                        has_foldable = true;
                                        break 'find_foldable;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if has_foldable {
        let const_true_id = {
            let mut found = None;
            for (i, c) in module.constants().iter().enumerate() {
                if matches!(c, Constant::Bool(true)) {
                    found = Some(ConstantId(i as u32));
                    break;
                }
            }
            found.unwrap_or_else(|| module.add_constant(Constant::Bool(true)))
        };

        for func_idx in 0..module.functions().len() {
            let new_object_dests = {
                let func = &module.functions()[func_idx];
                let mut dests = Vec::new();
                for block in func.blocks() {
                    for instr in block.instructions() {
                        if let Instruction::NewObject { dest, .. } = instr {
                            dests.push(*dest);
                        }
                    }
                }
                dests
            };
            if new_object_dests.is_empty() {
                continue;
            }

            let func = module
                .function_mut(FunctionId(func_idx as u32))
                .expect("function must exist");
            for block in func.blocks_mut() {
                fold_is_js_object_in_block(block, &new_object_dests, const_true_id);
            }
        }
    }
}