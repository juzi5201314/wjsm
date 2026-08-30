//! `Array.from(items, mapper)` 且 mapper 统一构造同类实例时，把结果绑定
//! 记成 elem-guard 模板来源。运行期 `GuardElementsKind` 仍会校验 packed
//! 与 shape；此处只提供静态候选，不改 Array.from 语义。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    Builtin, Constant, ConstantId, Function, FunctionId, Instruction, Module, ValueId, value,
};

use super::escape_scalar_record::template_key_names;
use crate::ir_walk::instruction_dest;

fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

fn const_string<'a>(
    defs: &HashMap<ValueId, &'a Instruction>,
    constants: &'a [Constant],
    value: ValueId,
) -> Option<&'a str> {
    let Instruction::Const { constant, .. } = defs.get(&value)? else {
        return None;
    };
    match constants.get(constant.0 as usize)? {
        Constant::String(text) => Some(text.as_str()),
        _ => None,
    }
}

fn defs_of(function: &Function) -> HashMap<ValueId, &Instruction> {
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction);
            }
        }
    }
    defs
}

/// 类构造器 `$this` 上按出现序收集的自有数据键；键序冲突则放弃该类名。
fn class_ctor_keys(module: &Module) -> HashMap<String, Vec<String>> {
    let constants = module.constants();
    let mut by_name: HashMap<String, Option<Vec<String>>> = HashMap::new();
    for function in module.functions() {
        let Some(name) = function.class_ctor_name() else {
            continue;
        };
        let Some(keys) = this_set_prop_keys(function, constants) else {
            continue;
        };
        match by_name.get(name) {
            None => {
                by_name.insert(name.to_owned(), Some(keys));
            }
            Some(Some(existing)) if existing == &keys => {}
            Some(_) => {
                by_name.insert(name.to_owned(), None);
            }
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, keys)| keys.map(|keys| (name, keys)))
        .collect()
}

fn this_set_prop_keys(function: &Function, constants: &[Constant]) -> Option<Vec<String>> {
    let defs = defs_of(function);
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::SetProp { object, key, .. } = instruction else {
                continue;
            };
            let Some(Instruction::LoadVar { name, .. }) = defs.get(object) else {
                continue;
            };
            if !is_this_name(name) {
                continue;
            }
            let Some(key) = const_string(&defs, constants, *key) else {
                return None;
            };
            let key = key.to_owned();
            if !seen.insert(key.clone()) {
                continue;
            }
            keys.push(key);
        }
    }
    if keys.is_empty() { None } else { Some(keys) }
}

fn function_ref_of(
    defs: &HashMap<ValueId, &Instruction>,
    constants: &[Constant],
    value: ValueId,
) -> Option<FunctionId> {
    match defs.get(&value)? {
        Instruction::Const { constant, .. } => match constants.get(constant.0 as usize)? {
            Constant::FunctionRef(function_id) => Some(*function_id),
            _ => None,
        },
        Instruction::CallBuiltin {
            builtin: Builtin::CreateClosure,
            args,
            ..
        } if !args.is_empty() => function_ref_of(defs, constants, args[0]),
        Instruction::Phi { sources, .. } => {
            let mut found = None;
            for source in sources {
                let Some(function_id) = function_ref_of(defs, constants, source.value) else {
                    continue;
                };
                match found {
                    None => found = Some(function_id),
                    Some(existing) if existing == function_id => {}
                    Some(_) => return None,
                }
            }
            found
        }
        _ => None,
    }
}

fn mapper_ctor_keys(
    module: &Module,
    ctor_keys: &HashMap<String, Vec<String>>,
    mapper: FunctionId,
) -> Option<Vec<String>> {
    let function = module.functions().get(mapper.0 as usize)?;
    let mut found = None;
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::ConstructCall { callsite, .. } = instruction else {
                continue;
            };
            let name = callsite.as_deref()?;
            let keys = ctor_keys.get(name)?;
            match &found {
                None => found = Some(keys.clone()),
                Some(existing) if existing == keys => {}
                Some(_) => return None,
            }
        }
    }
    found.filter(|keys| !keys.is_empty())
}

fn array_from_dest_keys(
    defs: &HashMap<ValueId, &Instruction>,
    constants: &[Constant],
    module: &Module,
    ctor_keys: &HashMap<String, Vec<String>>,
    dest: ValueId,
    visiting: &mut HashSet<ValueId>,
) -> Option<Vec<String>> {
    if !visiting.insert(dest) {
        return None;
    }
    match defs.get(&dest)? {
        Instruction::CallBuiltin {
            builtin: Builtin::ArrayFrom,
            args,
            ..
        } if args.len() >= 2 => {
            let mapper = function_ref_of(defs, constants, args[1])?;
            mapper_ctor_keys(module, ctor_keys, mapper)
        }
        Instruction::Phi { sources, .. } => {
            merge_phi_keys(defs, constants, module, ctor_keys, sources, visiting)
        }
        Instruction::CallBuiltin {
            builtin: Builtin::ExceptionValue,
            ..
        } => None,
        _ => None,
    }
}

fn merge_phi_keys(
    defs: &HashMap<ValueId, &Instruction>,
    constants: &[Constant],
    module: &Module,
    ctor_keys: &HashMap<String, Vec<String>>,
    sources: &[wjsm_ir::PhiSource],
    visiting: &mut HashSet<ValueId>,
) -> Option<Vec<String>> {
    let mut found = None;
    for source in sources {
        let Some(keys) =
            array_from_dest_keys(defs, constants, module, ctor_keys, source.value, visiting)
        else {
            continue;
        };
        match &found {
            None => found = Some(keys),
            Some(existing) if existing == &keys => {}
            Some(_) => return None,
        }
    }
    found
}

fn collect_array_from_candidates(
    module: &Module,
    store_sites: &HashMap<String, u32>,
) -> Vec<(String, Vec<String>)> {
    let ctor_keys = class_ctor_keys(module);
    if ctor_keys.is_empty() {
        return Vec::new();
    }
    let constants = module.constants();
    let mut out = Vec::new();
    for function in module.functions() {
        let defs = defs_of(function);
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Instruction::StoreVar { name, value } = instruction else {
                    continue;
                };
                if store_sites.get(name).copied() != Some(1) {
                    continue;
                }
                let Some(keys) = array_from_dest_keys(
                    &defs,
                    constants,
                    module,
                    &ctor_keys,
                    *value,
                    &mut HashSet::new(),
                ) else {
                    continue;
                };
                out.push((name.clone(), keys));
            }
        }
    }
    out
}

fn encode_template_key(module: &mut Module, key: &str) -> Option<u64> {
    if key.is_ascii() && key.len() <= 6 {
        let encoded = value::encode_inline_ascii(key.as_bytes())?;
        value::inline_property_key_raw(encoded)
    } else {
        let constant_idx = module.add_constant(Constant::String(key.to_owned()));
        Some(value::template_name_ref_key(constant_idx.0))
    }
}

fn intern_or_reuse_template(module: &mut Module, keys: &[String]) -> Option<ConstantId> {
    if keys.is_empty() {
        return None;
    }
    for (index, _) in module.constants().iter().enumerate() {
        let template = ConstantId(index as u32);
        if template_key_names(module.constants(), template).as_deref() == Some(keys) {
            return Some(template);
        }
    }
    let encoded: Option<Vec<u64>> = keys
        .iter()
        .map(|key| encode_template_key(module, key))
        .collect();
    let encoded = encoded?;
    Some(module.add_constant(Constant::ObjectTemplate { keys: encoded }))
}

/// 把合格的 `Array.from` + 统一构造器绑定写入 `bindings`（已有字面量绑定优先）。
pub(crate) fn add_array_from_bindings(
    module: &mut Module,
    store_sites: &HashMap<String, u32>,
    bindings: &mut HashMap<String, ConstantId>,
) {
    let candidates = collect_array_from_candidates(module, store_sites);
    for (name, keys) in candidates {
        if bindings.contains_key(&name) {
            continue;
        }
        if let Some(template) = intern_or_reuse_template(module, &keys) {
            bindings.insert(name, template);
        }
    }
}
