//! 对象模板 install 期元数据的编译期辅助：模板溯源、属性键匹配与 IC 预填 hint。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{Constant, ConstantId, Function, Instruction, Program, ValueId, value};

/// 单态 trio mega-slot 覆盖的三个自有数据键（与 property-key 热路径一致）。
pub(crate) const TRIO_KEY_NAME: &str = "name";
pub(crate) const TRIO_KEY_VALUE: &str = "value";
pub(crate) const TRIO_KEY_LENGTH: &str = "length";

/// `name` / `value` / `length` 在 trio 槽内的字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrioField {
    Name = 0,
    Value = 1,
    Length = 2,
}

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
    /// 模板含 name/value/length 时，三键在模板中的属性下标；预填一次写满 mega-slot。
    pub trio_prop_indices: Option<[u32; 3]>,
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
        let mut trio_slots: HashMap<u32, u32> = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let (dest, object, key) = match instruction {
                    Instruction::GetProp { dest, object, key }
                    | Instruction::OptionalGetProp { dest, object, key } => {
                        (*dest, Some(*object), *key)
                    }
                    // GetPropGuarded 的慢路径复用完整 GetProp IC，同样分配槽。
                    Instruction::GetPropGuarded {
                        dest, object, key, ..
                    } => (*dest, Some(*object), *key),
                    Instruction::SetProp {
                        dest, object, key, ..
                    } => (*dest, Some(*object), *key),
                    _ => continue,
                };
                let key_raw =
                    const_property_key_raw(program.constants(), &const_defs, key).unwrap_or(0);
                let (template_meta_index, prop_index) = template_hint_for_access(
                    program.constants(),
                    &const_defs,
                    origins,
                    object,
                    key,
                );
                if let Some(slot) = trio_slot_for_access(
                    program.constants(),
                    &const_defs,
                    origins,
                    object,
                    key,
                    &mut trio_slots,
                    &mut hints,
                    &mut slot_index,
                ) {
                    slots.insert(dest, slot);
                    continue;
                }
                slots.insert(dest, slot_index);
                hints.push(IcTemplateHint {
                    property_key_raw: key_raw,
                    template_meta_index,
                    prop_index,
                    trio_prop_indices: None,
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
    let seed = collect_shared_template_vars(program);
    program
        .functions()
        .iter()
        .map(|function| build_template_origin_map(function, program.constants(), &seed))
        .collect()
}

/// 模块绑定上的模板对象：任一函数 `StoreVar` 写入后，其它函数的 `LoadVar` 也能溯源。
fn collect_shared_template_vars(program: &Program) -> HashMap<String, TemplateSite> {
    let mut seeded: HashMap<String, TemplateSite> = HashMap::new();
    let mut conflict: HashSet<String> = HashSet::new();
    for function in program.functions() {
        for (name, site) in collect_template_var_stores(function, program.constants()) {
            if conflict.contains(&name) {
                continue;
            }
            match seeded.get(&name) {
                None => {
                    seeded.insert(name, site);
                }
                Some(existing) if *existing == site => {}
                Some(_) => {
                    conflict.insert(name.clone());
                    seeded.remove(&name);
                }
            }
        }
    }
    seeded
}

fn collect_template_var_stores(
    function: &Function,
    constants: &[Constant],
) -> Vec<(String, TemplateSite)> {
    let origins = build_template_origin_map(function, constants, &HashMap::new());
    let mut stores = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction
                && let Some(site) = origins.get(value).copied()
            {
                stores.push((name.clone(), site));
            }
        }
    }
    stores
}

fn build_template_origin_map(
    function: &Function,
    constants: &[Constant],
    seed: &HashMap<String, TemplateSite>,
) -> TemplateOriginMap {
    let mut value_origins = HashMap::new();
    let mut var_origins: HashMap<&str, TemplateSite> = seed
        .iter()
        .map(|(name, site)| (name.as_str(), *site))
        .collect();
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
                    } else {
                        var_origins.remove(name.as_str());
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

/// 模板溯源且含完整 trio 键时，本访问使用共享 mega-slot。
pub(crate) fn trio_field_for_access(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    origins: &TemplateOriginMap,
    object: ValueId,
    key: ValueId,
) -> Option<TrioField> {
    let field = trio_field_for_key(constants, const_defs, key)?;
    let site = origins.get(&object)?;
    let _ = trio_prop_indices(constants, site.template)?;
    Some(field)
}

fn trio_slot_for_access(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    origins: &TemplateOriginMap,
    object: Option<ValueId>,
    key: ValueId,
    trio_slots: &mut HashMap<u32, u32>,
    hints: &mut Vec<IcTemplateHint>,
    slot_index: &mut u32,
) -> Option<u32> {
    let object = object?;
    trio_field_for_access(constants, const_defs, origins, object, key)?;
    let site = origins.get(&object)?;
    let indices = trio_prop_indices(constants, site.template)?;
    if let Some(slot) = trio_slots.get(&site.meta_index) {
        return Some(*slot);
    }
    let slot = *slot_index;
    trio_slots.insert(site.meta_index, slot);
    hints.push(IcTemplateHint {
        property_key_raw: 0,
        template_meta_index: Some(site.meta_index),
        prop_index: None,
        trio_prop_indices: Some(indices),
    });
    *slot_index += 1;
    Some(slot)
}

fn trio_field_for_key(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    key: ValueId,
) -> Option<TrioField> {
    if let Some(key_raw) = const_property_key_raw(constants, const_defs, key) {
        return trio_field_for_key_raw(key_raw);
    }
    let constant_id = const_defs.get(&key)?;
    let index = usize::try_from(constant_id.0).ok()?;
    let Constant::String(text) = constants.get(index)? else {
        return None;
    };
    TrioField::from_text(text)
}

fn trio_field_for_key_raw(key_raw: u64) -> Option<TrioField> {
    for field in [TrioField::Name, TrioField::Value, TrioField::Length] {
        let text = field.as_str();
        if let Some(encoded) = value::encode_inline_ascii(text.as_bytes())
            && value::inline_property_key_raw(encoded) == Some(key_raw)
        {
            return Some(field);
        }
    }
    None
}

impl TrioField {
    fn as_str(self) -> &'static str {
        match self {
            Self::Name => TRIO_KEY_NAME,
            Self::Value => TRIO_KEY_VALUE,
            Self::Length => TRIO_KEY_LENGTH,
        }
    }

    fn from_text(text: &str) -> Option<Self> {
        match text {
            TRIO_KEY_NAME => Some(Self::Name),
            TRIO_KEY_VALUE => Some(Self::Value),
            TRIO_KEY_LENGTH => Some(Self::Length),
            _ => None,
        }
    }
}

fn trio_prop_indices(constants: &[Constant], template: ConstantId) -> Option<[u32; 3]> {
    Some([
        template_property_index_by_key_text(constants, template, TRIO_KEY_NAME)?,
        template_property_index_by_key_text(constants, template, TRIO_KEY_VALUE)?,
        template_property_index_by_key_text(constants, template, TRIO_KEY_LENGTH)?,
    ])
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
    let prop_index = template_property_index_for_key(constants, const_defs, site.template, key);
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
    let encoded = value::encode_inline_ascii(text.as_bytes())?;
    value::inline_property_key_raw(encoded)
}

fn template_property_index_by_key_text(
    constants: &[Constant],
    template: ConstantId,
    key_text: &str,
) -> Option<u32> {
    let index = usize::try_from(template.0).ok()?;
    let Constant::ObjectTemplate { keys } = constants.get(index)? else {
        return None;
    };
    if let Some(encoded) = value::encode_inline_ascii(key_text.as_bytes()) {
        if let Some(key_raw) = value::inline_property_key_raw(encoded) {
            if let Some(prop_index) = keys.iter().position(|key| *key == key_raw) {
                return Some(u32::try_from(prop_index).expect("模板属性下标在 u32 内"));
            }
        }
    }
    keys.iter()
        .position(|key| {
            value::template_key_name_ref(*key).is_some_and(|idx| {
                matches!(
                    constants.get(idx as usize),
                    Some(Constant::String(text)) if text == key_text
                )
            })
        })
        .map(|index| u32::try_from(index).expect("模板属性下标在 u32 内"))
}

pub(crate) fn template_property_index_for_key(
    constants: &[Constant],
    const_defs: &HashMap<ValueId, ConstantId>,
    template: ConstantId,
    key: ValueId,
) -> Option<u32> {
    if let Some(key_raw) = const_property_key_raw(constants, const_defs, key) {
        return template_property_index_with_key_raw(constants, template, key_raw);
    }
    let constant_id = const_defs.get(&key)?;
    let index = usize::try_from(constant_id.0).ok()?;
    let Constant::String(key_text) = constants.get(index)? else {
        return None;
    };
    template_property_index_by_key_text(constants, template, key_text)
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
        .or_else(|| {
            value::template_key_name_ref(key_raw).and_then(|constant_idx| {
                let Constant::String(key_text) = constants.get(constant_idx as usize)? else {
                    return None;
                };
                template_property_index_by_key_text(constants, template, key_text)
            })
        })
}
