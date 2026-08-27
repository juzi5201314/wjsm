use std::collections::HashMap;

use wjsm_ir::{Constant, FunctionId, Instruction, Program, Terminator, ValueId};

#[derive(Clone)]
pub(crate) struct DirectCallSite {
    pub(crate) caller: usize,
    pub(crate) args: Vec<ValueId>,
}

pub(crate) struct DirectCallInfo {
    pub(crate) calls_by_target: Vec<Vec<DirectCallSite>>,
    pub(crate) targets_by_callee_value: Vec<HashMap<ValueId, FunctionId>>,
    pub(crate) escaped_target: Vec<bool>,
}

/// 收集模块内可静态解析的直接调用边。
///
/// callee 来源：`Const FunctionRef` 与 `LoadVar` + `known_callee_vars`。
pub(crate) fn collect_direct_calls(program: &Program) -> DirectCallInfo {
    let function_count = program.functions().len();
    let mut calls_by_target = vec![Vec::new(); function_count];
    let mut targets_by_callee_value = Vec::with_capacity(function_count);
    let mut escaped_target = vec![false; function_count];

    for (caller, function) in program.functions().iter().enumerate() {
        let mut targets = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                match instruction {
                    Instruction::Const {
                        dest,
                        constant: wjsm_ir::ConstantId(constant),
                    } => {
                        if let Some(Constant::FunctionRef(target)) =
                            program.constants().get(*constant as usize)
                        {
                            targets.insert(*dest, *target);
                        }
                    }
                    Instruction::LoadVar { dest, name } => {
                        if let Some(target) = function.known_callee_vars().get(name) {
                            targets.insert(*dest, *target);
                        }
                    }
                    _ => {}
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
                if let Some((target, args)) = direct_callee
                    && let Some(calls) = calls_by_target.get_mut(target.0 as usize)
                {
                    calls.push(DirectCallSite {
                        caller,
                        args: args.clone(),
                    });
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
                    if !used_as_callee || instruction_uses_other_than_callee(instruction, *value) {
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

/// 函数体内所有可解析 `Call` 的目标。
pub(crate) fn direct_call_targets(
    function: &wjsm_ir::Function,
    targets: &HashMap<ValueId, FunctionId>,
) -> Vec<FunctionId> {
    let mut callees = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Call { callee, .. } = instruction
                && let Some(target) = targets.get(callee)
            {
                callees.push(*target);
            }
        }
    }
    callees
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
        | Instruction::CloneArrayTemplate { .. }
        | Instruction::InitObjectLiteral { .. }
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
