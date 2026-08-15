use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    Builtin, Constant, Function, FunctionId, Instruction, Program, Terminator, UnaryOp, ValueId,
};
use wjsm_native_abi::NativeHostSymbol;

#[derive(Clone)]
struct DirectCallSite {
    caller: usize,
    args: Vec<ValueId>,
}

struct DirectCallInfo {
    calls_by_target: Vec<Vec<DirectCallSite>>,
    targets_by_callee_value: Vec<HashMap<ValueId, FunctionId>>,
    escaped_target: Vec<bool>,
}

pub(crate) fn infer_f64_values(program: &Program) -> HashMap<FunctionId, HashSet<ValueId>> {
    infer_f64_values_with_param_seeds(program, &HashMap::new())
}

/// 以「调用方保证第 i 个 JS 参数是 number」的种子运行同一套 fixpoint 分析。
///
/// 种子只把更多参数标记为 f64 起点，不改变传播规则；用于运行时特化编译时，
/// 让 wrapper 的入口 tag 守卫背书下的参数衍生值获得 f64 证明。base 编译
/// （无种子）与 overlay 编译（有种子）共享同一分析实现。
pub(crate) fn infer_f64_values_with_param_seeds(
    program: &Program,
    seeds: &HashMap<FunctionId, Vec<bool>>,
) -> HashMap<FunctionId, HashSet<ValueId>> {
    let call_info = collect_direct_calls(program);
    let function_count = program.functions().len();
    let frame_locals = program.frame_local_variable_names_by_function();
    let mut f64_params: Vec<Vec<bool>> = program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let empty = vec![false; function.params().len().saturating_sub(2)];
            let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
            match seeds.get(&function_id) {
                Some(seed) => seed.clone(),
                None => empty,
            }
        })
        .collect();
    let mut return_f64 = vec![false; function_count];
    let mut inferred = vec![HashSet::new(); function_count];

    loop {
        let next_inferred: Vec<HashSet<ValueId>> = program
            .functions()
            .iter()
            .enumerate()
            .map(|(index, function)| {
                analyze_function(
                    program,
                    function,
                    &frame_locals[index],
                    &f64_params[index],
                    &call_info.targets_by_callee_value[index],
                    &return_f64,
                )
            })
            .collect();
        let next_return_f64: Vec<bool> = program
            .functions()
            .iter()
            .enumerate()
            .map(|(index, function)| function_returns_f64(function, &next_inferred[index]))
            .collect();
        let mut next_params = f64_params.clone();
        for (target, calls) in call_info.calls_by_target.iter().enumerate() {
            if call_info.escaped_target[target] || calls.is_empty() {
                continue;
            }
            for parameter in 0..next_params[target].len() {
                if next_params[target][parameter]
                    || calls.iter().all(|call| {
                        call.args
                            .get(parameter)
                            .is_some_and(|value| next_inferred[call.caller].contains(value))
                    })
                {
                    next_params[target][parameter] = true;
                }
            }
        }

        let stable =
            next_inferred == inferred && next_return_f64 == return_f64 && next_params == f64_params;
        inferred = next_inferred;
        return_f64 = next_return_f64;
        f64_params = next_params;
        if stable {
            break;
        }
    }

    inferred
        .into_iter()
        .enumerate()
        .map(|(index, values)| (FunctionId(index as u32), values))
        .collect()
}

fn collect_direct_calls(program: &Program) -> DirectCallInfo {
    let function_count = program.functions().len();
    let mut calls_by_target = vec![Vec::new(); function_count];
    let mut targets_by_callee_value = Vec::with_capacity(function_count);
    let mut escaped_target = vec![false; function_count];

    for (caller, function) in program.functions().iter().enumerate() {
        let mut targets = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Const {
                    dest,
                    constant: wjsm_ir::ConstantId(constant),
                } = instruction
                    && let Some(Constant::FunctionRef(target)) =
                        program.constants().get(*constant as usize)
                {
                    targets.insert(*dest, *target);
                }
            }
        }

        for block in function.blocks() {
            for instruction in block.instructions() {
                let direct_callee = match instruction {
                    Instruction::Call { callee, args, .. }
                    | Instruction::ConstructCall { callee, args, .. } => {
                        targets.get(callee).copied().map(|target| (target, args))
                    }
                    _ => None,
                };
                if let Some((target, args)) = direct_callee {
                    if let Some(calls) = calls_by_target.get_mut(target.0 as usize) {
                        calls.push(DirectCallSite {
                            caller,
                            args: args.clone(),
                        });
                    }
                }
                for (value, target) in &targets {
                    if !instruction_uses_value(instruction, *value) {
                        continue;
                    }
                    let used_as_callee = matches!(
                        instruction,
                        Instruction::Call { callee, .. }
                            | Instruction::ConstructCall { callee, .. }
                            if callee == value
                    );
                    if !used_as_callee {
                        escaped_target[target.0 as usize] = true;
                    } else if instruction_uses_other_than_callee(instruction, *value) {
                        escaped_target[target.0 as usize] = true;
                    }
                }
            }
            let terminator = block.terminator();
            for (value, target) in &targets {
                if terminator_uses_value(terminator, *value) {
                    escaped_target[target.0 as usize] = true;
                }
            }
        }
        targets_by_callee_value.push(targets);
    }

    DirectCallInfo {
        calls_by_target,
        targets_by_callee_value,
        escaped_target,
    }
}

fn analyze_function(
    program: &Program,
    function: &Function,
    frame_locals: &std::collections::BTreeSet<&str>,
    f64_params: &[bool],
    direct_targets: &HashMap<ValueId, FunctionId>,
    return_f64: &[bool],
) -> HashSet<ValueId> {
    let parameter_names: HashSet<&str> = function
        .params()
        .iter()
        .skip(2)
        .zip(f64_params)
        .filter_map(|(name, is_f64)| is_f64.then_some(name.as_str()))
        .collect();
    let modified_parameters: HashSet<&str> = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::StoreVar { name, .. } if parameter_names.contains(name.as_str()) => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    let mut f64_values = HashSet::new();
    let mut f64_locals: HashSet<&str> = parameter_names
        .iter()
        .copied()
        .filter(|name| frame_locals.contains(name))
        .collect();
    let mut mixed_locals: HashSet<&str> = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let destination = match instruction {
                    Instruction::Const { dest, constant }
                        if matches!(
                            program.constants().get(constant.0 as usize),
                            Some(Constant::Number(_))
                        ) =>
                    {
                        Some(*dest)
                    }
                    Instruction::StoreVar { name, value }
                        if frame_locals.contains(name.as_str())
                            && !mixed_locals.contains(name.as_str()) =>
                    {
                        if f64_values.contains(value) {
                            if f64_locals.insert(name.as_str()) {
                                changed = true;
                            }
                        } else if mixed_locals.insert(name.as_str()) {
                            f64_locals.remove(name.as_str());
                            changed = true;
                        }
                        None
                    }
                    Instruction::LoadVar { dest, name }
                        if f64_locals.contains(name.as_str())
                            || (parameter_names.contains(name.as_str())
                                && !modified_parameters.contains(name.as_str())) =>
                    {
                        Some(*dest)
                    }
                    Instruction::Binary { dest, lhs, rhs, .. }
                        if f64_values.contains(lhs) && f64_values.contains(rhs) =>
                    {
                        Some(*dest)
                    }
                    Instruction::Unary {
                        dest,
                        value,
                        op: UnaryOp::Neg | UnaryOp::Pos,
                    } if f64_values.contains(value) => Some(*dest),
                    Instruction::Call {
                        dest: Some(dest),
                        callee,
                        ..
                    } if direct_targets.get(callee).is_some_and(|target| {
                        return_f64.get(target.0 as usize).copied().unwrap_or(false)
                    }) =>
                    {
                        Some(*dest)
                    }
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin:
                            Builtin::MathAbs
                            | Builtin::MathSqrt
                            | Builtin::MathCeil
                            | Builtin::MathFloor
                            | Builtin::MathTrunc
                            | Builtin::MathFround,
                        args,
                    } if matches!(args.as_slice(), [arg] if f64_values.contains(arg)) => {
                        Some(*dest)
                    }
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin,
                        args,
                    } if NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                        let arity = usize::from(symbol.signature().argument_count());
                        args.len() == arity && args.iter().all(|arg| f64_values.contains(arg))
                    }) =>
                    {
                        Some(*dest)
                    }
                    Instruction::Phi { dest, sources }
                        if !sources.is_empty()
                            && sources
                                .iter()
                                .all(|source| f64_values.contains(&source.value)) =>
                    {
                        Some(*dest)
                    }
                    _ => None,
                };
                if destination.is_some_and(|value| f64_values.insert(value)) {
                    changed = true;
                }
            }
        }
    }
    f64_values
}

fn function_returns_f64(function: &Function, values: &HashSet<ValueId>) -> bool {
    let mut saw_return = false;
    for block in function.blocks() {
        match block.terminator() {
            Terminator::Return { value: Some(value) } => {
                saw_return = true;
                if !values.contains(value) {
                    return false;
                }
            }
            Terminator::Return { value: None } | Terminator::Throw { .. } => return false,
            _ => {}
        }
    }
    saw_return
}

fn instruction_uses_value(instruction: &Instruction, target: ValueId) -> bool {
    match instruction {
        Instruction::Binary { lhs, rhs, .. } | Instruction::Compare { lhs, rhs, .. } => {
            *lhs == target || *rhs == target
        }
        Instruction::Unary { value, .. }
        | Instruction::IsException { value, .. }
        | Instruction::EncodeException { value, .. }
        | Instruction::ExceptionToObject { value, .. } => *value == target,
        Instruction::StringConcatVa { parts, .. }
        | Instruction::CallBuiltin { args: parts, .. } => parts.contains(&target),
        Instruction::GetProp { object, key, .. }
        | Instruction::OptionalGetProp { object, key, .. }
        | Instruction::OptionalGetElem { object, key, .. } => *object == target || *key == target,
        Instruction::SetProp {
            object, key, value, ..
        }
        | Instruction::CreateDataProperty {
            object, key, value, ..
        } => *object == target || *key == target || *value == target,
        Instruction::SetProto { object, value } => *object == target || *value == target,
        Instruction::GetElem { object, index, .. } => *object == target || *index == target,
        Instruction::SetElem {
            object,
            index,
            value,
            ..
        } => *object == target || *index == target || *value == target,
        Instruction::Call {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::OptionalCall {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::SuperCall {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => *callee == target || *this_val == target || args.contains(&target),
        Instruction::DeleteProp { object, key, .. } => *object == target || *key == target,
        Instruction::PromiseResolve { promise, value }
        | Instruction::PromiseReject {
            promise,
            reason: value,
        } => *promise == target || *value == target,
        Instruction::Suspend { promise, .. } => *promise == target,
        Instruction::GeneratorSuspend { result, .. } => *result == target,
        Instruction::GuardSameFunction { callee, .. } => *callee == target,
        Instruction::ObjectSpread { dest, source } => *dest == target || *source == target,
        Instruction::StoreVar { value, .. } => *value == target,
        Instruction::Phi { sources, .. } => sources.iter().any(|source| source.value == target),
        Instruction::Const { .. }
        | Instruction::LoadVar { .. }
        | Instruction::NewObject { .. }
        | Instruction::NewArray { .. }
        | Instruction::GetSuperBase { .. }
        | Instruction::GetSuperConstructor { .. }
        | Instruction::NewPromise { .. }
        | Instruction::CollectRestArgs { .. }
        | Instruction::DebugCheck { .. } => false,
    }
}

fn instruction_uses_other_than_callee(instruction: &Instruction, target: ValueId) -> bool {
    match instruction {
        Instruction::Call { this_val, args, .. }
        | Instruction::OptionalCall { this_val, args, .. }
        | Instruction::SuperCall { this_val, args, .. }
        | Instruction::ConstructCall { this_val, args, .. } => {
            *this_val == target || args.contains(&target)
        }
        _ => false,
    }
}

fn terminator_uses_value(terminator: &Terminator, target: ValueId) -> bool {
    match terminator {
        Terminator::Return { value } => value.is_some_and(|value| value == target),
        Terminator::Throw { value } => *value == target,
        Terminator::Branch { condition, .. }
        | Terminator::Switch {
            value: condition, ..
        } => *condition == target,
        Terminator::Jump { .. } | Terminator::Unreachable => false,
    }
}
