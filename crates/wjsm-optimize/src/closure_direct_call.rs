//! 存储闭包直调解析：将 `const inc = factory()` 后反复 `inc()` 解析为
//! `(FunctionRef, lex_env)`，供后端跳过 PrepareCall 闭包解包。

use std::collections::HashMap;

use wjsm_ir::{Builtin, Constant, Function, FunctionId, Instruction, Module, ValueId};

use crate::inline_for_ea::compute_load_var_reaching;
use crate::ir_walk::instruction_dest;

/// 静态可解析的闭包调用目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedClosureCall {
    pub function_id: FunctionId,
    pub env: ValueId,
}

/// 模块级不可变函数声明绑定（与 semantic `direct_call` 同口径）。
pub fn module_immutable_function_bindings(module: &Module) -> HashMap<String, FunctionId> {
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

fn trace_value(mut current: ValueId, load_reaching: &HashMap<ValueId, ValueId>) -> ValueId {
    while let Some(reaching) = load_reaching.get(&current) {
        if *reaching == current {
            break;
        }
        current = *reaching;
    }
    current
}

fn resolve_create_closure(
    defs: &HashMap<ValueId, &Instruction>,
    constants: &[Constant],
    load_reaching: &HashMap<ValueId, ValueId>,
    value: ValueId,
) -> Option<ResolvedClosureCall> {
    let current = trace_value(value, load_reaching);
    let Instruction::CallBuiltin {
        builtin: Builtin::CreateClosure,
        args,
        ..
    } = defs.get(&current)?
    else {
        return None;
    };
    if args.len() < 2 {
        return None;
    }
    let mut fn_val = args[0];
    fn_val = trace_value(fn_val, load_reaching);
    let Instruction::Const { constant, .. } = defs.get(&fn_val)? else {
        return None;
    };
    let Constant::FunctionRef(function_id) = constants.get(constant.0 as usize)? else {
        return None;
    };
    Some(ResolvedClosureCall {
        function_id: *function_id,
        env: args[1],
    })
}

fn single_store_value(function: &Function, name: &str) -> Option<ValueId> {
    let mut values = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name: slot, value } = instruction
                && slot == name
            {
                values.push(*value);
            }
        }
    }
    if values.len() == 1 {
        Some(values[0])
    } else {
        None
    }
}

fn store_count(function: &Function, name: &str) -> u32 {
    function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter(|instruction| {
            matches!(
                instruction,
                Instruction::StoreVar { name: slot, .. } if slot == name
            )
        })
        .count() as u32
}

/// 解析 `Call` 的 callee 是否为存储闭包直调目标。
pub fn resolve_stored_closure_call(
    module: &Module,
    function: &Function,
    callee: ValueId,
) -> Option<ResolvedClosureCall> {
    if module.functions().iter().any(|f| f.has_eval()) {
        return None;
    }
    let constants = module.constants();
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction);
            }
        }
    }
    let load_reaching = compute_load_var_reaching(function);
    let current = trace_value(callee, &load_reaching);

    if let Some(resolved) = resolve_create_closure(&defs, constants, &load_reaching, current) {
        return Some(resolved);
    }

    let Instruction::LoadVar { name, .. } = defs.get(&current)? else {
        return None;
    };
    if store_count(function, name) != 1 {
        return None;
    }
    let stored = single_store_value(function, name)?;
    resolve_create_closure(&defs, constants, &load_reaching, stored)
}

/// 内联阶段 A 是否应跳过该闭包 callee 的体展开（改由后端直调）。
pub fn should_backend_direct_closure_call(module: &Module, function_id: FunctionId) -> bool {
    module
        .functions()
        .get(function_id.0 as usize)
        .is_some_and(|function| !function.captured_names().is_empty())
}

/// 解析 callee ValueId 为 FunctionRef（含不可变模块绑定与 env GetProp）。
pub fn resolve_function_ref_callee(
    module: &Module,
    _function: &Function,
    defs: &HashMap<ValueId, &Instruction>,
    load_reaching: &HashMap<ValueId, ValueId>,
    callee: &ValueId,
) -> Option<FunctionId> {
    let constants = module.constants();
    let immutable = module_immutable_function_bindings(module);
    let current = trace_value(*callee, load_reaching);
    match defs.get(&current)? {
        Instruction::Const { constant, .. } => match constants.get(constant.0 as usize) {
            Some(Constant::FunctionRef(function_id)) => Some(*function_id),
            _ => None,
        },
        Instruction::LoadVar { name, .. } => immutable.get(name).copied(),
        Instruction::GetProp { object: _, key, .. } => {
            let Instruction::Const { constant, .. } = defs.get(key)? else {
                return None;
            };
            let Constant::String(key_str) = constants.get(constant.0 as usize)? else {
                return None;
            };
            immutable.get(key_str).copied()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Terminator};

    #[test]
    fn resolves_stored_create_closure_binding() {
        let mut module = Module::new();
        let fn_id = FunctionId(1);
        let fn_const = module.add_constant(Constant::FunctionRef(fn_id));
        let mut work = Function::new("work", BasicBlockId(0));
        work.set_params(vec!["$4.$env".to_string(), "$4.$this".to_string()]);
        let mut bb = BasicBlock::new(BasicBlockId(0));
        bb.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: fn_const,
        });
        bb.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: module.add_constant(Constant::Undefined),
        });
        bb.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(2)),
            builtin: Builtin::CreateClosure,
            args: vec![ValueId(0), ValueId(1)],
        });
        bb.push_instruction(Instruction::StoreVar {
            name: "$4.inc".to_string(),
            value: ValueId(2),
        });
        bb.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$4.inc".to_string(),
        });
        bb.set_terminator(Terminator::Return { value: Some(ValueId(3)) });
        work.push_block(bb);
        module.push_function(Function::new("dummy", BasicBlockId(0)));
        module.push_function(work);

        let work = &module.functions()[1];
        let resolved = resolve_stored_closure_call(&module, work, ValueId(3)).expect("应解析");
        assert_eq!(resolved.function_id, fn_id);
        assert_eq!(resolved.env, ValueId(1));
    }
}
