//! 对象模板 install 期元数据的编译期辅助：模板溯源、属性键匹配与 IC 预填 hint。

use std::collections::HashMap;

use wjsm_ir::{Constant, ConstantId, Function, Instruction, Program, ValueId, value};

/// 单个 `ObjectTemplate` 在 install 烘焙表中的下标与模板常量。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TemplateSite {
    pub template: ConstantId,
    pub meta_index: u32,
}

/// 每个 IC 槽在 install 期预填所需的模板关联（按全局槽下标排列）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IcTemplateHint {
    pub property_key_raw: u64,
    pub template_meta_index: Option<u32>,
    pub prop_index: Option<u32>,
}

/// IC 槽分配结果：每函数 dest→槽下标、全局 hint 表、总槽数。
#[derive(Debug)]
pub(crate) struct IcSlotPlan {
    pub per_function: Vec<HashMap<ValueId, u32>>,
    pub hints: Vec<IcTemplateHint>,
    pub total: u32,
}

/// 为常量字符串键的属性访问分配 IC 槽，并记录 install 期预填 hint。
pub(crate) fn plan_ic_slots(program: &Program) -> IcSlotPlan {
    let template_origins: Vec<TemplateOriginMap> = build_template_origin_maps(program);
    let mut per_function = Vec::with_capacity(program.functions().len());
    let mut hints = Vec::new();
    let mut slot_index = 0_u32;
    for (function, origins) in program.functions().iter().zip(&template_origins) {
        let const_defs = const_defs_for_function(function);
        let mut slots = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let (dest, object, key) = match instruction {
                    Instruction::GetProp { dest, object, key }
                    | Instruction::OptionalGetProp { dest, object, key } => {
                        (*dest, Some(*object), *key)
                    }
                    Instruction::SetProp { dest, key, .. } => (*dest, None, *key),
                    _ => continue,
                };
                let Some(key_raw) =
                    const_property_key_raw(program.constants(), &const_defs, key)
                else {
                    continue;
                };
                let (template_meta_index, prop_index) = template_hint_for_access(
                    program.constants(),
                    &const_defs,
                    origins,
                    object,
                    key,
                );
                slots.insert(dest, slot_index);
                hints.push(IcTemplateHint {
                    property_key_raw: key_raw,
                    template_meta_index,
                    prop_index,
                });
                slot_index += 1;
            }
        }
        per_function.push(slots);
    }
    IcSlotPlan {
        per_function,
        hints,
        total: slot_index,
    }
}

/// 供宿主 install 期 IC 预填：与编译期 `plan_ic_slots` 使用同一算法，保证槽编号一致。
pub fn ic_template_hints(program: &Program) -> Vec<IcTemplateHint> {
    plan_ic_slots(program).hints
}

/// 每函数 `ValueId` 到模板站点的溯源表。
pub(crate) type TemplateOriginMap = HashMap<ValueId, TemplateSite>;

pub(crate) fn build_template_origin_maps(program: &Program) -> Vec<TemplateOriginMap> {
    program
        .functions()
        .iter()
        .map(|function| build_template_origin_map(function, program.constants()))
        .collect()
}

fn build_template_origin_map(
    function: &Function,
    constants: &[Constant],
) -> TemplateOriginMap {
    let mut value_origins = HashMap::new();
    let mut var_origins: HashMap<&str, TemplateSite> = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::InitObjectLiteral { dest, template, .. } => {
                    if let Some(meta_index) = object_template_meta_index(constants, *template) {
                        value_origins.insert(
                            *dest,
                            TemplateSite {
                                template: *template,
                                meta_index,
                            },
                        );
                    }
                }
                Instruction::StoreVar { name, value } => {
                    if let Some(site) = value_origins.get(value).copied() {
                        var_origins.insert(name.as_str(), site);
                    }
                }
                Instruction::LoadVar { dest, name } => {
                    if let Some(site) = var_origins.get(name.as_str()).copied() {
                        value_origins.insert(*dest, site);
                    }
                }
                Instruction::Phi { dest, sources } => {
                    let mut unique = None;
                    for source in sources {
                        if let Some(site) = value_origins.get(&source.value).copied() {
                            match unique {
                                None => unique = Some(site),
                                Some(existing) if existing == site => {}
                                _ => {
                                    unique = None;
                                    break;
                                }
                            }
                        } else {
                            unique = None;
                            break;
                        }
                    }
                    if let Some(site) = unique {
                        value_origins.insert(*dest, site);
                    }
                }
                _ => {}
            }
        }
    }
    value_origins
}

fn const_defs_for_function(function: &Function) -> HashMap<ValueId, ConstantId> {
    let mut const_defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction {
                const_defs.insert(*dest, *constant);
            }
        }
    }
    const_defs
}

fn template_hint_for_access(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    origins: &TemplateOriginMap,
    object: Option<ValueId>,
    key: ValueId,
) -> (Option<u32>, Option<u32>) {
    let Some(object) = object else {
        return (None, None);
    };
    let Some(site) = origins.get(&object) else {
        return (None, None);
    };
    let prop_index =
        template_property_index_for_key(constants, const_defs, site.template, key);
    (Some(site.meta_index), prop_index)
}

pub(crate) fn object_template_meta_index(
    constants: &[Constant],
    template: ConstantId,
) -> Option<u32> {
    let index = usize::try_from(template.0).ok()?;
    if !matches!(constants.get(index), Some(Constant::ObjectTemplate { .. })) {
        return None;
    }
    Some(
        constants[..index]
            .iter()
            .filter(|constant| matches!(constant, Constant::ObjectTemplate { .. }))
            .count() as u32,
    )
}

pub(crate) fn const_property_key_raw(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    key: ValueId,
) -> Option<u64> {
    let constant_id = const_defs.get(&key)?;
    let index = usize::try_from(constant_id.0).ok()?;
    let Constant::String(text) = constants.get(index)? else {
        return None;
    };
    let encoded = value::encode_inline_ascii(text.as_bytes())
        .or_else(|| value::encode_inline_latin1(text.as_bytes()))?;
    value::inline_property_key_raw(encoded)
}

pub(crate) fn template_property_index_for_key(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    template: ConstantId,
    key: ValueId,
) -> Option<u32> {
    let key_raw = const_property_key_raw(constants, const_defs, key)?;
    template_property_index_with_key_raw(constants, template, key_raw)
}

pub(crate) fn template_property_index_with_key_raw(
    constants: &[Constant],
    template: ConstantId,
    key_raw: u64,
) -> Option<u32> {
    let index = usize::try_from(template.0).ok()?;
    let Constant::ObjectTemplate { keys } = constants.get(index)? else {
        return None;
    };
    keys.iter()
        .position(|key| *key == key_raw)
        .map(|index| u32::try_from(index).expect("模板属性下标在 u32 内"))
}
