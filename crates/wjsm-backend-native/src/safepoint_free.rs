use std::collections::{HashMap, HashSet};

use wjsm_ir::{Constant, FunctionId, Instruction, Program, UnaryOp, ValueId};
use wjsm_native_abi::NativeHostSymbol;

use crate::call_graph::{collect_direct_calls, direct_call_targets};
use crate::f64_analysis::infer_f64_values;
use crate::root_plan::RootPlan;
pub(crate) fn infer_safepoint_free_functions(program: &Program) -> HashSet<FunctionId> {
    let call_info = collect_direct_calls(program);
    let inferred_f64 = infer_f64_values(program);
    let frame_locals = program.frame_local_variable_names_by_function();
    let function_count = program.functions().len();
    let constants = program.constants();

    let locally_eligible: Vec<bool> = program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
            let f64_values = inferred_f64
                .get(&function_id)
                .expect("f64 analysis covers every function");
            if has_bigint_constant(function, constants) {
                return false;
            }
            let call_only_refs = call_only_function_ref_values(function, constants);
            let root_plan = RootPlan::build(function, f64_values);
            if root_plan.max_roots_excluding(&call_only_refs) != 0 {
                return false;
            }
            function_instructions_allowed(
                function,
                constants,
                f64_values,
                &frame_locals[index],
                &call_info.targets_by_callee_value[index],
            )
        })
        .collect();

    let mut free: HashSet<usize> = (0..function_count)
        .filter(|index| locally_eligible[*index])
        .collect();
    loop {
        let before = free.len();
        let retained: HashSet<usize> = free
            .iter()
            .copied()
            .filter(|index| {
                direct_call_targets(
                    &program.functions()[*index],
                    &call_info.targets_by_callee_value[*index],
                )
                .iter()
                .all(|target| free.contains(&(target.0 as usize)))
            })
            .collect();
        free = retained;
        if free.len() == before {
            break;
        }
    }

    free.into_iter()
        .map(|index| FunctionId(index as u32))
        .collect()
}

fn has_bigint_constant(function: &wjsm_ir::Function, constants: &[Constant]) -> bool {
    function.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            let Instruction::Const { constant, .. } = instruction else {
                return false;
            };
            matches!(
                constants.get(constant.0 as usize),
                Some(Constant::BigInt(_))
            )
        })
    })
}

fn function_instructions_allowed(
    function: &wjsm_ir::Function,
    constants: &[Constant],
    f64_values: &HashSet<ValueId>,
    frame_locals: &std::collections::BTreeSet<&str>,
    callee_targets: &HashMap<ValueId, FunctionId>,
) -> bool {
    function.blocks().iter().all(|block| {
        block.instructions().iter().all(|instruction| {
            instruction_is_allowed(
                instruction,
                constants,
                f64_values,
                frame_locals,
                callee_targets,
            )
        })
    })
}

fn instruction_is_allowed(
    instruction: &Instruction,
    constants: &[Constant],
    f64_values: &HashSet<ValueId>,
    frame_locals: &std::collections::BTreeSet<&str>,
    callee_targets: &HashMap<ValueId, FunctionId>,
) -> bool {
    match instruction {
        Instruction::Const { constant, .. } => !matches!(
            constants.get(constant.0 as usize),
            Some(Constant::BigInt(_))
        ),
        Instruction::LoadVar { dest, name } => {
            frame_locals.contains(name.as_str()) && f64_values.contains(dest)
        }
        Instruction::StoreVar { name, value } => {
            frame_locals.contains(name.as_str()) && f64_values.contains(value)
        }
        Instruction::Binary { dest, lhs, rhs, .. } => every_f64([*dest, *lhs, *rhs], f64_values),
        Instruction::Unary { dest, op, value } => {
            matches!(
                op,
                UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot | UnaryOp::Not
            ) && every_f64([*dest, *value], f64_values)
        }
        Instruction::Compare { lhs, rhs, .. } => every_f64([*lhs, *rhs], f64_values),
        Instruction::Phi { sources, .. } => {
            !sources.is_empty()
                && sources
                    .iter()
                    .all(|source| f64_values.contains(&source.value))
        }
        Instruction::Call { callee, .. } => callee_targets.contains_key(callee),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } => {
            f64_values.contains(dest)
                && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                    args.len() == usize::from(symbol.signature().argument_count())
                })
                && every_f64(args.iter().copied(), f64_values)
        }
        _ => false,
    }
}

fn every_f64(values: impl IntoIterator<Item = ValueId>, f64_values: &HashSet<ValueId>) -> bool {
    values.into_iter().all(|value| f64_values.contains(&value))
}

/// 仅作为 `Call` callee 使用的 `FunctionRef` 常量：direct call 不需要把它们发布进 root frame。
fn call_only_function_ref_values(
    function: &wjsm_ir::Function,
    constants: &[Constant],
) -> HashSet<ValueId> {
    let mut defs = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::Const { dest, constant } = instruction else {
                continue;
            };
            if matches!(
                constants.get(constant.0 as usize),
                Some(Constant::FunctionRef(_))
            ) {
                defs.insert(*dest);
            }
        }
    }
    let mut call_only = defs.clone();
    for block in function.blocks() {
        for instruction in block.instructions() {
            for value in instruction_value_uses(instruction) {
                if !defs.contains(&value) {
                    continue;
                }
                let callee_only = matches!(
                    instruction,
                    Instruction::Call { callee, .. } if *callee == value
                ) && !instruction_uses_other_than_callee(instruction, value);
                if !callee_only {
                    call_only.remove(&value);
                }
            }
        }
    }
    call_only
}

fn instruction_value_uses(instruction: &Instruction) -> HashSet<ValueId> {
    let mut uses = HashSet::new();
    let mut remapped = instruction.clone();
    remapped.remap_values(&mut |value| {
        uses.insert(value);
        value
    });
    let _ = remapped;
    uses
}

fn instruction_uses_other_than_callee(instruction: &Instruction, target: ValueId) -> bool {
    match instruction {
        Instruction::Call { this_val, args, .. } => *this_val == target || args.contains(&target),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Builtin, Constant, Function, Terminator};

    #[test]
    fn numeric_leaf_add_is_safepoint_free() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let add_ref = program.add_constant(Constant::FunctionRef(FunctionId(1)));

        let mut caller = Function::new("main", BasicBlockId(0));
        let mut caller_block = BasicBlock::new(BasicBlockId(0));
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: add_ref,
        });
        caller_block.push_instruction(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(0),
            args: vec![ValueId(0), ValueId(0)],
        });
        caller_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee = Function::new("add", BasicBlockId(0));
        callee.set_direct_callable(true);
        callee.set_params(vec!["$env".into(), "$this".into(), "x".into(), "y".into()]);
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "y".into(),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: wjsm_ir::BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        callee.push_block(block);
        program.push_function(callee);

        let free = infer_safepoint_free_functions(&program);
        assert_eq!(free, HashSet::from([FunctionId(1)]));
    }

    #[test]
    fn console_log_disqualifies_function() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ConsoleLog,
            args: vec![ValueId(0)],
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        program.push_function(function);

        let free = infer_safepoint_free_functions(&program);
        assert!(free.is_empty());
    }

    #[test]
    fn indirect_call_disqualifies_function() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        block.push_instruction(Instruction::Call {
            dest: Some(ValueId(1)),
            callee: ValueId(0),
            this_val: ValueId(0),
            args: vec![ValueId(0)],
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(block);
        program.push_function(function);

        let free = infer_safepoint_free_functions(&program);
        assert!(free.is_empty());
    }
}
