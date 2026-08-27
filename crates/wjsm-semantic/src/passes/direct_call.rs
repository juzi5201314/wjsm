//! direct_call pass：标记可直接调用的函数并原地替换绑定读取为 `Const(FunctionRef)`。
//!
//! 背景：每次 JS 函数调用默认被编译为全动态语义（callee 求值、类型分派、闭包解包、
//! new.target 保存等，伴随多次 native host 往返）。本 pass 识别「不可变函数声明绑定」
//! 与「函数体不依赖 env/this/new.target」的函数：
//!
//! - 把 `LoadVar("$N.fib")` 与 `GetProp(env, "$N.fib")` 原地替换为 `Const(FunctionRef)`，
//!   使后端 callee 静态解析直接命中；
//! - 给函数写回 `direct_callable` 标记，后端据此对调用点发射直接 `call`。
//!
//! 安全性：函数声明 hoisted 且语义不可重赋（`fn_decls` 中唯一一次 `StoreVar` + 无 eval），
//! 其绑定值恒等于初始 `FunctionRef`；替换后动态调用路径（TAG_FUNCTION，env=undefined）
//! 对 env 全可解析的函数同样安全。
//!
//! 注意：ValueId 在每个函数内从 0 重新编号（`push_function_context` 重置 `next_value`），
//! 因此 def 表、const 字符串表与 env GetProp 记录都必须按函数维度隔离。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    Constant, ConstantId, Function, FunctionId, Instruction, Module, Terminator, ValueId,
};

/// 是否为 env 变量 IR 名（`$env` 或 `${scope}.$env`）。
pub(crate) fn is_env_name(name: &str) -> bool {
    name == "$env" || name.ends_with(".$env")
}

/// 指令的 ValueId 操作数（uses）。与后端 `analysis_liveness` 的收集规则一致。
pub(crate) fn instr_uses(ins: &Instruction) -> Vec<ValueId> {
    use Instruction::*;
    match ins {
        Binary { lhs, rhs, .. } | Compare { lhs, rhs, .. } => vec![*lhs, *rhs],
        Unary { value, .. } => vec![*value],
        StringConcatVa { parts, .. } => parts.clone(),
        GetProp { object, key, .. } => vec![*object, *key],
        SetProp {
            object, key, value, ..
        }
        | CreateDataProperty {
            object, key, value, ..
        } => vec![*object, *key, *value],
        SetProto { object, value } => vec![*object, *value],
        GetElem { object, index, .. } => vec![*object, *index],
        ElemShapeGuard { array, .. } => vec![*array],
        GetElemGuarded {
            object,
            index,
            guard,
            ..
        } => vec![*object, *index, *guard],
        GetPropGuarded {
            object, key, guard, ..
        } => vec![*object, *key, *guard],
        SetElem {
            object,
            index,
            value,
            ..
        } => vec![*object, *index, *value],
        OptionalGetProp { object, key, .. } | OptionalGetElem { object, key, .. } => {
            vec![*object, *key]
        }
        OptionalCall {
            callee,
            this_val,
            args,
            ..
        }
        | Call {
            callee,
            this_val,
            args,
            ..
        }
        | SuperCall {
            callee,
            this_val,
            args,
            ..
        } => {
            let mut v = vec![*callee, *this_val];
            v.extend(args.iter().copied());
            v
        }
        ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => {
            let mut v = vec![*callee, *this_val];
            v.extend(args.iter().copied());
            v
        }
        CallBuiltin { args, .. } => args.clone(),
        DeleteProp { object, key, .. } => vec![*object, *key],
        PromiseResolve { promise, value }
        | PromiseReject {
            promise,
            reason: value,
        } => vec![*promise, *value],
        Suspend { promise, .. } => vec![*promise],
        GeneratorSuspend { result, .. } => vec![*result],
        IsException { value, .. }
        | EncodeException { value, .. }
        | ExceptionToObject { value, .. } => vec![*value],
        GuardSameFunction { callee, .. } => vec![*callee],
        ObjectSpread { object, source, .. } => vec![*object, *source],
        StoreVar { value, .. } => vec![*value],
        InitObjectLiteral { values, .. } => values.clone(),
        // 无操作数
        Const { .. }
        | LoadVar { .. }
        | NewObject { .. }
        | NewArray { .. }
        | CloneArrayTemplate { .. }
        | GetSuperBase { .. }
        | GetSuperConstructor { .. }
        | NewPromise { .. }
        | CollectRestArgs { .. }
        | DebugCheck { .. }
        | Phi { .. } => vec![],
    }
}

/// 终止器的 ValueId 操作数（uses）。
pub(crate) fn terminator_uses(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Return { value: Some(v) } => vec![*v],
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Switch { value, .. } => vec![*value],
        Terminator::Throw { value } => vec![*value],
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {
            vec![]
        }
    }
}

/// 收集 `target` 在本函数中的全部 use 指令（含 Phi source）。
pub(crate) fn collect_uses(function: &Function, target: ValueId) -> Vec<&Instruction> {
    let mut uses = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            let mut used = instr_uses(instruction);
            if let Instruction::Phi { sources, .. } = instruction {
                used.extend(sources.iter().map(|s| s.value));
            }
            if used.contains(&target) {
                uses.push(instruction);
            }
        }
    }
    uses
}

/// 取 producing 指令的 dest（def）。非 producing 返回 None。
pub(crate) fn instruction_dest(ins: &Instruction) -> Option<ValueId> {
    use Instruction::*;
    Some(match ins {
        Const { dest, .. }
        | Binary { dest, .. }
        | Unary { dest, .. }
        | Compare { dest, .. }
        | Phi { dest, .. }
        | StringConcatVa { dest, .. }
        | LoadVar { dest, .. }
        | NewObject { dest, .. }
        | GetProp { dest, .. }
        | SetProp { dest, .. }
        | CreateDataProperty { dest, .. }
        | DeleteProp { dest, .. }
        | NewArray { dest, .. }
        | CloneArrayTemplate { dest, .. }
        | InitObjectLiteral { dest, .. }
        | GetElem { dest, .. }
        | SetElem { dest, .. }
        | OptionalGetProp { dest, .. }
        | OptionalGetElem { dest, .. }
        | ElemShapeGuard { dest, .. }
        | GetElemGuarded { dest, .. }
        | GetPropGuarded { dest, .. }
        | OptionalCall { dest, .. }
        | GetSuperBase { dest }
        | GetSuperConstructor { dest }
        | NewPromise { dest }
        | CollectRestArgs { dest, .. }
        | IsException { dest, .. }
        | GuardSameFunction { dest, .. }
        | EncodeException { dest, .. }
        | ExceptionToObject { dest, .. }
        | ObjectSpread { dest, .. } => *dest,
        Call { dest, .. }
        | CallBuiltin { dest, .. }
        | SuperCall { dest, .. }
        | ConstructCall { dest, .. } => (*dest)?,
        // 非 producing
        StoreVar { .. }
        | SetProto { .. }
        | PromiseResolve { .. }
        | PromiseReject { .. }
        | Suspend { .. }
        | GeneratorSuspend { .. }
        | DebugCheck { .. } => return None,
    })
}

/// 收集本函数中作为 `TdzCheck` 首参的 ValueId 集合。
///
/// 这些读取是跨函数前向引用的运行时 TDZ 受检读取：声明执行前 env 槽持有
/// 未初始化哨兵，必须保留真实的 GetProp/LoadVar 读取，不得替换为
/// `Const(FunctionRef)`（否则类声明的 TDZ 语义被优化抹掉）。
fn tdz_checked_reads(function: &Function) -> HashSet<ValueId> {
    let mut checked = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::CallBuiltin {
                builtin: wjsm_ir::Builtin::TdzCheck,
                args,
                ..
            } = instruction
                && let Some(value) = args.first()
            {
                checked.insert(*value);
            }
        }
    }
    checked
}

/// 不可变函数/类声明绑定：`known_callee_vars` 中全模块只被 `StoreVar` 写过一次的名字。
///
/// 唯一一次 store 即声明的初始 store（`record_known_callee` + `StoreVar` 路径），
/// 因此该名字的读取恒等于初始函数值。调用方必须先确认全模块无 eval。
pub(crate) fn immutable_function_bindings(module: &Module) -> HashMap<String, FunctionId> {
    let mut store_count: HashMap<&str, u32> = HashMap::new();
    for function in module.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, .. } = instruction {
                    *store_count.entry(name.as_str()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut immutable = HashMap::new();
    for function in module.functions() {
        for (name, function_id) in function.known_callee_vars() {
            if store_count.get(name.as_str()) == Some(&1) {
                immutable.insert(name.clone(), *function_id);
            }
        }
    }
    immutable
}

/// 运行 direct_call pass。任一函数含 eval 时全局禁用（eval 可动态改写绑定）。
pub fn run(module: &mut Module) {
    // 1. 全局守卫：eval 可动态改写绑定，保守禁用整个 pass。
    if module.functions().iter().any(|f| f.has_eval()) {
        return;
    }

    // 2. 不可变绑定集合。
    let immutable = immutable_function_bindings(module);

    // 3. per 函数判定：env_required / has_new_target / resolvable_env_gets。
    let mut env_required: HashSet<FunctionId> = HashSet::new();
    let mut has_new_target: HashSet<FunctionId> = HashSet::new();
    // env_deps：函数 → 其 env 读取（`GetProp(env, immutable 函数声明)`）依赖的目标
    // 函数。目标函数 env_required 时该读取不会被替换为 FunctionRef，env 仍然必需，
    // 经不动点传播后本函数同样 env_required。
    let mut env_deps: HashMap<FunctionId, Vec<FunctionId>> = HashMap::new();
    // resolvable_env_gets：函数 → {(env LoadVar dest, 属性名)}，该 env 读取的 GetProp 可解析。
    let mut resolvable_env_gets: HashMap<FunctionId, HashSet<(ValueId, String)>> = HashMap::new();

    for (func_idx, function) in module.functions().iter().enumerate() {
        let func_id = FunctionId(func_idx as u32);
        let tdz_checked = tdz_checked_reads(function);

        // 本函数 def 表与 Const(String) 常量名映射（ValueId 每函数独立编号）。
        let mut defs: HashMap<ValueId, &Instruction> = HashMap::new();
        let mut const_strings: HashMap<ValueId, String> = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                match instruction {
                    Instruction::Const { dest, constant } => {
                        defs.insert(*dest, instruction);
                        if let Some(Constant::String(s)) =
                            module.constants().get(constant.0 as usize)
                        {
                            const_strings.insert(*dest, s.clone());
                        }
                    }
                    _ => {
                        if let Some(dest) = instruction_dest(instruction) {
                            defs.insert(dest, instruction);
                        }
                    }
                }
            }
        }

        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::CallBuiltin {
                    builtin: wjsm_ir::Builtin::NewTarget,
                    ..
                } = instruction
                {
                    has_new_target.insert(func_id);
                }
                // SuperCall 也在后端调用 new.target，阻止 direct_callable。
                if matches!(instruction, Instruction::SuperCall { .. }) {
                    has_new_target.insert(func_id);
                }
                // CollectRestArgs 依赖宿主运行时 activation，阻止 direct_callable。
                if matches!(instruction, Instruction::CollectRestArgs { .. }) {
                    has_new_target.insert(func_id);
                }
                // GetSuperBase / GetSuperConstructor 读取当前 activation 的
                // home_object；直接调用不压 activation，会错读外层帧，阻止
                // direct_callable（静态字段初始化器/static block/方法的 super）。
                if matches!(
                    instruction,
                    Instruction::GetSuperBase { .. } | Instruction::GetSuperConstructor { .. }
                ) {
                    has_new_target.insert(func_id);
                }
            }
        }

        // env LoadVar dest 收集。
        let mut env_load_dests: Vec<ValueId> = Vec::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::LoadVar { dest, name } = instruction
                    && is_env_name(name)
                {
                    env_load_dests.push(*dest);
                }
            }
        }

        // env dest 的 use 分析：全部是 `GetProp(env, immutable 常量)` 才可解析。
        // 可解析仅表示 key 是函数声明名；若目标函数自身 env_required（其 env 读取
        // 不可全解析），该 GetProp 不会在替换阶段变成 FunctionRef，env 仍然必需，
        // 经不动点（见下）传播为本函数 env_required。
        for env_dest in env_load_dests {
            let mut all_resolvable = true;
            // terminator 使用 env dest（如 return env）→ env 必需。
            for block in function.blocks() {
                if terminator_uses(block.terminator()).contains(&env_dest) {
                    all_resolvable = false;
                    break;
                }
            }
            for use_instr in collect_uses(function, env_dest) {
                match use_instr {
                    Instruction::GetProp { dest, object, key } if *object == env_dest => {
                        // TDZ 受检读取必须保留真实 env 读取，视为不可解析。
                        if tdz_checked.contains(dest) {
                            all_resolvable = false;
                            continue;
                        }
                        match defs.get(key) {
                            Some(Instruction::Const { constant, .. }) => {
                                if let Some(Constant::String(s)) =
                                    module.constants().get(constant.0 as usize)
                                    && let Some(target) = immutable.get(s)
                                {
                                    resolvable_env_gets
                                        .entry(func_id)
                                        .or_default()
                                        .insert((env_dest, s.clone()));
                                    env_deps.entry(func_id).or_default().push(*target);
                                    continue;
                                }
                                all_resolvable = false;
                            }
                            _ => all_resolvable = false,
                        }
                    }
                    _ => all_resolvable = false,
                }
            }
            if !all_resolvable {
                env_required.insert(func_id);
            }
        }
    }

    // env_required 不动点：函数直接不可解析（非 `GetProp(env, immutable)` use）或经
    // `GetProp(env, immutable)` 依赖某个 env_required 的目标函数（该读取替换为
    // FunctionRef 后如果目标函数自身仍需 env，则当前函数如果需要直接调用它，自身需知晓）。
    loop {
        let mut changed = false;
        for (f, targets) in &env_deps {
            if !env_required.contains(f) && targets.iter().any(|t| env_required.contains(t)) {
                env_required.insert(*f);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 4. 写回 direct_callable（函数体不依赖 env/new.target，且无 eval）。
    // direct ABI 显式传递 this，因此读取 this 不影响静态直调。
    let mut direct_callables: Vec<(FunctionId, bool)> = Vec::new();
    for (func_idx, function) in module.functions().iter().enumerate() {
        let func_id = FunctionId(func_idx as u32);
        let direct_callable = !env_required.contains(&func_id)
            && !has_new_target.contains(&func_id)
            && !function.has_eval()
            && function.captured_names().is_empty();
        direct_callables.push((func_id, direct_callable));
    }
    for (func_id, direct_callable) in direct_callables {
        if direct_callable && let Some(f) = module.function_mut(func_id) {
            f.set_direct_callable(true);
        }
    }

    // 5. 替换集合：不可变绑定且自身不依赖 env 且无捕获。
    let replaceable: HashMap<String, FunctionId> = immutable
        .into_iter()
        .filter(|(_, f)| {
            !env_required.contains(f)
                && module
                    .functions()
                    .get(f.0 as usize)
                    .is_some_and(|func| func.direct_callable() && func.captured_names().is_empty())
        })
        .collect();
    if replaceable.is_empty() {
        return;
    }

    // 6. 执行替换：LoadVar(name ∈ replaceable) / GetProp(env, key ∈ replaceable) → Const(FunctionRef)。
    //    先确保所有需要的 FunctionRef 常量存在，避免替换循环内借用冲突。
    let mut functionref_consts: HashMap<FunctionId, ConstantId> = HashMap::new();
    for (idx, constant) in module.constants().iter().enumerate() {
        if let Constant::FunctionRef(id) = constant {
            functionref_consts
                .entry(*id)
                .or_insert(ConstantId(idx as u32));
        }
    }
    for fn_id in replaceable.values() {
        if !functionref_consts.contains_key(fn_id) {
            let cid = module.add_constant(Constant::FunctionRef(*fn_id));
            functionref_consts.insert(*fn_id, cid);
        }
    }

    // per 函数 Const(String) 表（替换阶段查 key 的常量字符串）。
    let mut per_fn_const_strings: Vec<HashMap<ValueId, String>> = Vec::new();
    for function in module.functions() {
        let mut strings = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Const { dest, constant } = instruction
                    && let Some(Constant::String(s)) = module.constants().get(constant.0 as usize)
                {
                    strings.insert(*dest, s.clone());
                }
            }
        }
        per_fn_const_strings.push(strings);
    }

    // 替换循环必须按索引访问（function_mut 需可变借用，不能与 iter() 共存）。
    #[allow(clippy::needless_range_loop)]
    for func_idx in 0..module.functions().len() {
        let func_id = FunctionId(func_idx as u32);
        let resolvable = resolvable_env_gets.get(&func_id);
        let const_strings = &per_fn_const_strings[func_idx];
        let function = module.function_mut(func_id).unwrap();
        let tdz_checked = tdz_checked_reads(function);
        for block in function.blocks_mut() {
            for instruction in block.instructions_mut() {
                match instruction {
                    Instruction::LoadVar { dest, name } => {
                        if !tdz_checked.contains(dest)
                            && let Some(fn_id) = replaceable.get(name)
                        {
                            let constant = functionref_consts[fn_id];
                            *instruction = Instruction::Const {
                                dest: *dest,
                                constant,
                            };
                        }
                    }
                    Instruction::GetProp { dest, object, key } => {
                        // key 的 def 是 Const(String) 且 (object, key_str) 可解析、key_str 可替换；
                        // TDZ 受检读取除外（必须在运行时观察 env 槽的哨兵状态）。
                        if !tdz_checked.contains(dest)
                            && let (Some(resolvable), Some(key_str)) =
                                (resolvable, const_strings.get(key))
                            && resolvable.contains(&(*object, key_str.clone()))
                            && let Some(fn_id) = replaceable.get(key_str)
                        {
                            let constant = functionref_consts[fn_id];
                            *instruction = Instruction::Const {
                                dest: *dest,
                                constant,
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
