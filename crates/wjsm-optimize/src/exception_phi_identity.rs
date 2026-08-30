//! 异常混合 Phi 在成功路径上的对象恒等折叠。
//!
//! 内联 `ConstructCall` 后常见：
//! `%p = phi [exception_value(set_prop), new_object, …]`，随后
//! `is_exception %p` 分出 throw / 成功。成功块内 `%p` 必等于那个
//! `NewObject`（或同一分配的 SetProp 成功 dest）。把成功侧支配区域内
//! 的 `%p` 换成该对象 SSA，让后续 `is_js_object` / `is_exception` /
//! 逃逸分析看到普通 `NewObject`，而不是「对象|异常」混合值。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Builtin, Dominators, Function, FunctionId, Instruction, Module, Terminator,
    ValueId,
};

use super::inline_for_ea::{replace_value_id, replace_value_id_in_terminator};
use crate::ir_walk::instruction_dest;

struct Collapse {
    phi: ValueId,
    object: ValueId,
    success: BasicBlockId,
}

pub(crate) fn run(module: &mut Module) {
    for index in 0..module.functions().len() {
        let function_id = FunctionId(index as u32);
        let function = module
            .function_mut(function_id)
            .expect("function id must be valid");
        fold_function(function);
    }
}

pub(crate) fn fold_function(function: &mut Function) -> bool {
    let defs = collect_defs(function);
    let collapses = collect_collapses(function, &defs);
    if collapses.is_empty() {
        return false;
    }
    let dom = Dominators::compute(function);
    let mut changed = false;
    for collapse in collapses {
        if apply_collapse(function, &dom, &collapse) {
            changed = true;
        }
    }
    changed
}

fn collect_defs(function: &Function) -> HashMap<ValueId, Instruction> {
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction.clone());
            }
        }
    }
    defs
}

fn collect_collapses(function: &Function, defs: &HashMap<ValueId, Instruction>) -> Vec<Collapse> {
    let mut out = Vec::new();
    for block in function.blocks() {
        let Terminator::Branch {
            condition,
            true_block: _,
            false_block,
        } = block.terminator()
        else {
            continue;
        };
        for instruction in block.instructions() {
            let Instruction::IsException { dest, value } = instruction else {
                continue;
            };
            if dest != condition {
                continue;
            }
            let Some(Instruction::Phi { .. }) = defs.get(value) else {
                continue;
            };
            let Some(object) = non_exception_identity(defs, *value, &mut HashSet::new()) else {
                continue;
            };
            if object == *value {
                continue;
            }
            out.push(Collapse {
                phi: *value,
                object,
                success: *false_block,
            });
        }
    }
    out
}

/// 非异常路径上 `value` 必等于的对象 SSA。纯 `exception_value` 源视为无身份。
fn non_exception_identity(
    defs: &HashMap<ValueId, Instruction>,
    value: ValueId,
    visiting: &mut HashSet<ValueId>,
) -> Option<ValueId> {
    if !visiting.insert(value) {
        return None;
    }
    let instruction = defs.get(&value)?;
    let result = match instruction {
        Instruction::Const { .. }
        | Instruction::NewObject { .. }
        | Instruction::InitObjectLiteral { .. }
        | Instruction::Compare { .. }
        | Instruction::NewArray { .. } => Some(value),
        Instruction::CallBuiltin {
            builtin: Builtin::CreateClosure,
            ..
        } => Some(value),
        Instruction::CallBuiltin {
            builtin: Builtin::ExceptionValue,
            ..
        } => None,
        Instruction::SetProp { object, .. } | Instruction::CreateDataProperty { object, .. } => {
            non_exception_identity(defs, *object, visiting)
        }
        Instruction::Phi { sources, .. } => merge_phi_identities(defs, sources, visiting),
        _ => None,
    };
    visiting.remove(&value);
    result
}

fn merge_phi_identities(
    defs: &HashMap<ValueId, Instruction>,
    sources: &[wjsm_ir::PhiSource],
    visiting: &mut HashSet<ValueId>,
) -> Option<ValueId> {
    let mut found = None;
    for source in sources {
        let Some(identity) = non_exception_identity(defs, source.value, visiting) else {
            continue;
        };
        match found {
            None => found = Some(identity),
            Some(existing) if existing == identity => {}
            Some(_) => return None,
        }
    }
    found
}

fn apply_collapse(function: &mut Function, dom: &Dominators, collapse: &Collapse) -> bool {
    let mut changed = false;
    for block in function.blocks_mut() {
        if !dom.dominates(collapse.success, block.id()) {
            continue;
        }
        for instruction in block.instructions_mut() {
            if instruction.uses().contains(&collapse.phi) {
                replace_value_id(instruction, collapse.phi, collapse.object);
                changed = true;
            }
        }
        if crate::ir_walk::terminator_uses(block.terminator()).contains(&collapse.phi) {
            replace_value_id_in_terminator(block.terminator_mut(), collapse.phi, collapse.object);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use wjsm_ir::{BasicBlock, Constant, Function, Program, Terminator};

    use super::*;

    #[test]
    fn success_path_phi_becomes_new_object() {
        let mut program = Program::new();
        let undef = program.add_constant(Constant::Undefined);
        let key = program.add_constant(Constant::String("x".into()));
        let mut function = Function::new("f", BasicBlockId(0));

        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::NewObject {
            dest: ValueId(0),
            capacity: 4,
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: key,
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: undef,
        });
        bb0.push_instruction(Instruction::SetProp {
            dest: ValueId(3),
            object: ValueId(0),
            key: ValueId(1),
            value: ValueId(2),
            strict: true,
        });
        bb0.push_instruction(Instruction::IsException {
            dest: ValueId(4),
            value: ValueId(3),
        });
        bb0.set_terminator(Terminator::Branch {
            condition: ValueId(4),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });

        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(5)),
            builtin: Builtin::ExceptionValue,
            args: vec![ValueId(3)],
        });
        bb1.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });

        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });

        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.push_instruction(Instruction::Phi {
            dest: ValueId(6),
            sources: vec![
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(1),
                    value: ValueId(5),
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: ValueId(0),
                },
            ],
        });
        bb3.push_instruction(Instruction::IsException {
            dest: ValueId(7),
            value: ValueId(6),
        });
        bb3.set_terminator(Terminator::Branch {
            condition: ValueId(7),
            true_block: BasicBlockId(4),
            false_block: BasicBlockId(5),
        });

        let mut bb4 = BasicBlock::new(BasicBlockId(4));
        bb4.set_terminator(Terminator::Throw { value: ValueId(6) });

        let mut bb5 = BasicBlock::new(BasicBlockId(5));
        bb5.push_instruction(Instruction::StoreVar {
            name: "$scaled".into(),
            value: ValueId(6),
        });
        bb5.set_terminator(Terminator::Return {
            value: Some(ValueId(6)),
        });

        function.push_block(bb0);
        function.push_block(bb1);
        function.push_block(bb2);
        function.push_block(bb3);
        function.push_block(bb4);
        function.push_block(bb5);
        program.push_function(function);

        let function = program.function_mut(FunctionId(0)).expect("function 0");
        assert!(fold_function(function));
        let text = function.dump_text();
        assert!(
            text.contains("store var $scaled, %0"),
            "成功路径应将 phi 换成 NewObject：{text}"
        );
        assert!(
            !text.contains("store var $scaled, %6"),
            "成功路径不应再存储混合 phi：{text}"
        );
    }
}
