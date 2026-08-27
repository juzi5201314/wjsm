//! 同基本块内 `InitObjectLiteral` 后的常量键 `GetProp` 折叠为 SSA 值。
//!
//! `tlab-object` 等热路径在字面量初始化后立即读取 `name`/`value`/`length`；
//! 若对象尚未经写入或调用变异，可直接使用 `InitObjectLiteral.values` 而无需
//! 走属性访问。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Constant, ConstantId, FunctionId, Instruction, Module, ValueId, value,
};

use super::inline_for_ea::replace_all_uses_of;

#[derive(Clone, Debug)]
struct LiteralSite {
    template: ConstantId,
    values: Vec<ValueId>,
}

/// 将常量字符串键解析为 `ObjectTemplate.keys` 中的下标。
fn template_property_index(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    template: ConstantId,
    key: ValueId,
) -> Option<usize> {
    let constant_id = const_defs.get(&key)?;
    let key_index = usize::try_from(constant_id.0).ok()?;
    let Constant::String(key_text) = constants.get(key_index)? else {
        return None;
    };
    let template_index = usize::try_from(template.0).ok()?;
    let Constant::ObjectTemplate { keys } = constants.get(template_index)? else {
        return None;
    };
    if let Some(encoded) = value::encode_inline_ascii(key_text.as_bytes()) {
        if let Some(key_raw) = value::inline_property_key_raw(encoded) {
            if let Some(index) = keys.iter().position(|candidate| *candidate == key_raw) {
                return Some(index);
            }
        }
    }
    keys.iter().position(|candidate| {
        value::template_key_name_ref(*candidate).is_some_and(|idx| {
            matches!(
                constants.get(idx as usize),
                Some(Constant::String(text)) if text == key_text
            )
        })
    })
}

fn resolve_canonical(object: ValueId, aliases: &HashMap<ValueId, ValueId>) -> ValueId {
    aliases.get(&object).copied().unwrap_or(object)
}

fn is_tracked_object(
    object: ValueId,
    aliases: &HashMap<ValueId, ValueId>,
    sites: &HashMap<ValueId, LiteralSite>,
) -> Option<ValueId> {
    let canonical = resolve_canonical(object, aliases);
    sites.contains_key(&canonical).then_some(canonical)
}

fn invalidate_object(
    object: ValueId,
    aliases: &HashMap<ValueId, ValueId>,
    sites: &mut HashMap<ValueId, LiteralSite>,
    var_bindings: &mut HashMap<String, ValueId>,
) {
    let Some(canonical) = is_tracked_object(object, aliases, sites) else {
        return;
    };
    sites.remove(&canonical);
    var_bindings.retain(|_, bound| *bound != canonical);
}

fn instruction_may_mutate_tracked_object(
    instruction: &Instruction,
    aliases: &HashMap<ValueId, ValueId>,
    sites: &HashMap<ValueId, LiteralSite>,
) -> bool {
    match instruction {
        Instruction::SetProp { object, .. }
        | Instruction::CreateDataProperty { object, .. }
        | Instruction::SetElem { object, .. }
        | Instruction::DeleteProp { object, .. } => {
            is_tracked_object(*object, aliases, sites).is_some()
        }
        Instruction::Call { this_val, args, .. }
        | Instruction::OptionalCall { this_val, args, .. }
        | Instruction::SuperCall { this_val, args, .. }
        | Instruction::ConstructCall { this_val, args, .. } => {
            is_tracked_object(*this_val, aliases, sites).is_some()
                || args
                    .iter()
                    .any(|arg| is_tracked_object(*arg, aliases, sites).is_some())
        }
        _ => false,
    }
}

fn fold_function(function: &mut wjsm_ir::Function, constants: &[Constant]) -> bool {
    let mut const_defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction {
                const_defs.insert(*dest, *constant);
            }
        }
    }

    let mut replacements = HashMap::new();
    let mut delete_targets = HashSet::new();

    for block in function.blocks() {
        let block_id = block.id();
        let mut sites: HashMap<ValueId, LiteralSite> = HashMap::new();
        let mut aliases: HashMap<ValueId, ValueId> = HashMap::new();
        let mut var_bindings: HashMap<String, ValueId> = HashMap::new();

        for (index, instruction) in block.instructions().iter().enumerate() {
            if instruction_may_mutate_tracked_object(instruction, &aliases, &sites) {
                match instruction {
                    Instruction::SetProp { object, .. }
                    | Instruction::CreateDataProperty { object, .. }
                    | Instruction::SetElem { object, .. }
                    | Instruction::DeleteProp { object, .. } => {
                        invalidate_object(*object, &aliases, &mut sites, &mut var_bindings);
                    }
                    Instruction::Call { this_val, args, .. }
                    | Instruction::OptionalCall { this_val, args, .. }
                    | Instruction::SuperCall { this_val, args, .. }
                    | Instruction::ConstructCall { this_val, args, .. } => {
                        if let Some(canonical) = is_tracked_object(*this_val, &aliases, &sites) {
                            invalidate_object(canonical, &aliases, &mut sites, &mut var_bindings);
                        }
                        for arg in args {
                            if let Some(canonical) = is_tracked_object(*arg, &aliases, &sites) {
                                invalidate_object(
                                    canonical,
                                    &aliases,
                                    &mut sites,
                                    &mut var_bindings,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }

            match instruction {
                Instruction::InitObjectLiteral {
                    dest,
                    template,
                    values,
                } => {
                    sites.insert(
                        *dest,
                        LiteralSite {
                            template: *template,
                            values: values.clone(),
                        },
                    );
                    aliases.insert(*dest, *dest);
                }
                Instruction::StoreVar { name, value } => {
                    let canonical = resolve_canonical(*value, &aliases);
                    if sites.contains_key(&canonical) {
                        var_bindings.insert(name.clone(), canonical);
                    } else {
                        // 变量被重新赋值为非跟踪值：残留旧绑定会把后续
                        // `LoadVar + GetProp` 错误折叠回字面量初值。
                        var_bindings.remove(name);
                    }
                }
                Instruction::LoadVar { dest, name } => {
                    if let Some(canonical) = var_bindings.get(name).copied() {
                        aliases.insert(*dest, canonical);
                    }
                }
                Instruction::GetProp { dest, object, key } => {
                    let canonical = resolve_canonical(*object, &aliases);
                    let Some(site) = sites.get(&canonical) else {
                        continue;
                    };
                    let Some(prop_index) =
                        template_property_index(constants, &const_defs, site.template, *key)
                    else {
                        continue;
                    };
                    let Some(replacement) = site.values.get(prop_index).copied() else {
                        continue;
                    };
                    replacements.insert(*dest, replacement);
                    delete_targets.insert((block_id, index));
                }
                _ => {}
            }
        }
    }

    if replacements.is_empty() && delete_targets.is_empty() {
        return false;
    }

    for (old, new) in &replacements {
        replace_all_uses_of(function, *old, *new);
    }

    let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
    for (block_id, index) in &delete_targets {
        by_block.entry(*block_id).or_default().push(*index);
    }
    for (block_id, mut indices) in by_block {
        indices.sort_unstable_by(|left, right| right.cmp(left));
        indices.dedup();
        if let Some(block) = function.block_by_id_mut(block_id) {
            let instructions = block.instructions_mut();
            for index in indices {
                if index < instructions.len() {
                    instructions.remove(index);
                }
            }
        }
    }

    true
}

pub(crate) fn run(module: &mut Module) {
    if module
        .functions()
        .iter()
        .any(|function| function.has_eval())
    {
        return;
    }

    let constants = module.constants().to_vec();
    let mut any_change = false;
    for function_index in 0..module.functions().len() {
        let function_id = FunctionId(function_index as u32);
        let function = module
            .function_mut(function_id)
            .expect("function id must be valid");
        if fold_function(function, &constants) {
            any_change = true;
        }
    }
    let _ = any_change;
}

#[cfg(test)]
mod tests {
    use wjsm_ir::{BasicBlock, Function, Program, Terminator};

    use super::*;

    fn sso_key(text: &str) -> u64 {
        let encoded = value::encode_inline_ascii(text.as_bytes()).expect("sso key");
        value::inline_property_key_raw(encoded).expect("property key")
    }

    fn build_tlab_like_function() -> (Program, FunctionId) {
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name"), sso_key("value"), sso_key("length")],
        });
        let key_name = program.add_constant(Constant::String("name".into()));
        let key_value = program.add_constant(Constant::String("value".into()));
        let key_length = program.add_constant(Constant::String("length".into()));

        let mut function = Function::new("tlab_like", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        for (dest, number) in [(ValueId(0), 1.0), (ValueId(1), 2.0), (ValueId(2), 3.0)] {
            block.push_instruction(Instruction::Const {
                dest,
                constant: program.add_constant(Constant::Number(number)),
            });
        }
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(3),
            template,
            values: vec![ValueId(0), ValueId(1), ValueId(2)],
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$0.object".into(),
            value: ValueId(3),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$0.object".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(5),
            constant: key_name,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(6),
            object: ValueId(4),
            key: ValueId(5),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(7),
            name: "$0.object".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(8),
            constant: key_value,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(9),
            object: ValueId(7),
            key: ValueId(8),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(10),
            name: "$0.object".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(11),
            constant: key_length,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(12),
            object: ValueId(10),
            key: ValueId(11),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(13),
            op: wjsm_ir::BinaryOp::Add,
            lhs: ValueId(6),
            rhs: ValueId(9),
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(14),
            op: wjsm_ir::BinaryOp::Add,
            lhs: ValueId(13),
            rhs: ValueId(12),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(14)),
        });
        function.push_block(block);
        let function_id = program.push_function(function);
        (program, function_id)
    }

    fn count_get_prop(function: &wjsm_ir::Function) -> usize {
        function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .filter(|instruction| matches!(instruction, Instruction::GetProp { .. }))
            .count()
    }

    #[test]
    fn folds_tlab_like_same_block_reads() {
        let (mut program, function_id) = build_tlab_like_function();
        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(count_get_prop(function), 3);

        run(&mut program);

        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(count_get_prop(function), 0);
        let block = function.blocks().first().expect("block");
        let add_uses_value_ssa = block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Binary {
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                    ..
                }
            )
        });
        assert!(
            add_uses_value_ssa,
            "expected add chain to use literal SSA values"
        );
    }

    #[test]
    fn folds_direct_ssa_get_prop() {
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name")],
        });
        let key = program.add_constant(Constant::String("name".into()));
        let mut function = Function::new("direct", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: program.add_constant(Constant::Number(1.0)),
        });
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(1),
            template,
            values: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: key,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(3),
            object: ValueId(1),
            key: ValueId(2),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(3)),
        });
        function.push_block(block);
        let function_id = program.push_function(function);

        run(&mut program);

        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(count_get_prop(function), 0);
        assert!(matches!(
            function.blocks()[0].terminator(),
            Terminator::Return {
                value: Some(ValueId(0)),
                ..
            }
        ));
    }

    #[test]
    fn does_not_fold_after_set_prop() {
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name")],
        });
        let key = program.add_constant(Constant::String("name".into()));
        let replacement = program.add_constant(Constant::Number(9.0));
        let mut function = Function::new("mutated", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: program.add_constant(Constant::Number(1.0)),
        });
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(1),
            template,
            values: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: key,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(3),
            constant: replacement,
        });
        block.push_instruction(Instruction::SetProp {
            dest: ValueId(4),
            object: ValueId(1),
            key: ValueId(2),
            value: ValueId(3),
            strict: false,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(5),
            object: ValueId(1),
            key: ValueId(2),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(5)),
        });
        function.push_block(block);
        let function_id = program.push_function(function);

        run(&mut program);

        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(count_get_prop(function), 1);
    }

    #[test]
    fn does_not_fold_after_var_rebind() {
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("x")],
        });
        let key = program.add_constant(Constant::String("x".into()));
        let number = program.add_constant(Constant::Number(5.0));
        let mut function = Function::new("rebound", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: program.add_constant(Constant::Number(1.0)),
        });
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(1),
            template,
            values: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$0.v".into(),
            value: ValueId(1),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: number,
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$0.v".into(),
            value: ValueId(2),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$0.v".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(4),
            constant: key,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(5),
            object: ValueId(3),
            key: ValueId(4),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(5)),
        });
        function.push_block(block);
        let function_id = program.push_function(function);

        run(&mut program);

        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(
            count_get_prop(function),
            1,
            "rebound variable read must not fold back to the literal value"
        );
    }

    fn name_ref_key(program: &mut Program, text: &str) -> u64 {
        let constant_idx = program.add_constant(Constant::String(text.into()));
        value::template_name_ref_key(constant_idx.0)
    }

    #[test]
    fn folds_long_key_name_ref_get_prop() {
        let mut program = Program::new();
        let name_ref = name_ref_key(&mut program, "firstName");
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![name_ref],
        });
        let key = program.add_constant(Constant::String("firstName".into()));
        let mut function = Function::new("long_key", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: program.add_constant(Constant::Number(1.0)),
        });
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(1),
            template,
            values: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: key,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(3),
            object: ValueId(1),
            key: ValueId(2),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(3)),
        });
        function.push_block(block);
        let function_id = program.push_function(function);

        run(&mut program);

        let function = &program.functions()[function_id.0 as usize];
        assert_eq!(count_get_prop(function), 0);
        assert!(matches!(
            function.blocks()[0].terminator(),
            Terminator::Return {
                value: Some(ValueId(0)),
                ..
            }
        ));
    }
}
