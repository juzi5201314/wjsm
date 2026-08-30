//! 从类构造器 `this.key = …` 提取自有属性键序，追加 `ObjectTemplate` 常量。
//!
//! 实例由 `ConstructCall` 创建时后端尚无字面量模板；本 pass 在 optimize 阶段
//! 为每个类构造器合成与构造器写入顺序一致的烘焙 shape，供 `template_meta`
//! 对 `ConstructCall` 结果做模板溯源与固定槽偏移读取。

use std::collections::HashSet;

use wjsm_ir::{Constant, Function, Instruction, Module, ValueId, value};

/// 为模块内全部类构造器追加缺失的实例 `ObjectTemplate` 常量。
pub(crate) fn augment_class_instance_templates(module: &mut Module) {
    let constructors: Vec<_> = module
        .functions()
        .iter()
        .filter(|f| f.is_class_constructor())
        .cloned()
        .collect();
    let mut known = existing_template_key_sets(module.constants());
    for function in constructors {
        let Some(keys) = extract_constructor_own_keys(&function, module.constants()) else {
            continue;
        };
        if known.contains(&keys) {
            continue;
        }
        module.add_constant(Constant::ObjectTemplate { keys: keys.clone() });
        known.insert(keys);
    }
}

fn existing_template_key_sets(constants: &[Constant]) -> HashSet<Vec<u64>> {
    constants
        .iter()
        .filter_map(|constant| match constant {
            Constant::ObjectTemplate { keys } => Some(keys.clone()),
            _ => None,
        })
        .collect()
}

/// 扫描构造器体：按出现顺序收集 `this` 上的常量键 `SetProp`。
fn extract_constructor_own_keys(function: &Function, constants: &[Constant]) -> Option<Vec<u64>> {
    let const_defs = const_defs_for_function(function);
    let mut keys = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::SetProp { object, key, .. } = instruction else {
                continue;
            };
            if !object_is_this(function, *object, &const_defs) {
                return None;
            }
            let key_raw = const_property_key_raw(constants, &const_defs, *key)?;
            keys.push(key_raw);
        }
    }
    (!keys.is_empty()).then_some(keys)
}

fn const_defs_for_function(function: &Function) -> std::collections::HashMap<ValueId, wjsm_ir::ConstantId> {
    let mut const_defs = std::collections::HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction {
                const_defs.insert(*dest, *constant);
            }
        }
    }
    const_defs
}

fn object_is_this(
    function: &Function,
    object: ValueId,
    const_defs: &std::collections::HashMap<ValueId, wjsm_ir::ConstantId>,
) -> bool {
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::LoadVar { dest, name } = instruction else {
                continue;
            };
            if *dest != object {
                continue;
            }
            return name == "$this" || name.ends_with(".$this");
        }
    }
    let _ = const_defs;
    false
}

fn const_property_key_raw(
    constants: &[Constant],
    const_defs: &std::collections::HashMap<ValueId, wjsm_ir::ConstantId>,
    key: ValueId,
) -> Option<u64> {
    let constant_id = const_defs.get(&key)?;
    let index = usize::try_from(constant_id.0).ok()?;
    let Constant::String(text) = constants.get(index)? else {
        return None;
    };
    let encoded = value::encode_inline_ascii(text.as_bytes())?;
    value::inline_property_key_raw(encoded)
}
