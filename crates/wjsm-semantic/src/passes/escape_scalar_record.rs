//! 整模块 RECORD 标量替换：不逃逸的 `InitObjectLiteral` 降为字段 SSA。
//!
//! 识别模块绑定 / 闭包 env 别名上的对象身份，若全部使用都是模板自有常量键
//! 的 GetProp/SetProp，则把字段写成 `$sroa.*` 槽，再在各函数内做 mem2reg：
//! 循环头 Phi、循环内纯 SSA，仅在循环出口与调用边界写回。

use super::cfg_fold::terminator_successors;
use super::direct_call::{instr_uses, instruction_dest, terminator_uses};
use super::escape_scalar::{
    CandidateAnalysis, PropertyPhi, PropertyRead, PropertyWrite, apply_value_replacements,
    close_replacements, next_value_id, resolve_property_replacements,
};
use std::collections::{HashMap, HashSet};
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, ConstantId, Function, FunctionId, Instruction, Module,
    Terminator, ValueId, value,
};

#[derive(Clone)]
struct AllocSite {
    function_index: usize,
    dest: ValueId,
    keys: Vec<String>,
}

struct FuncView {
    family: HashSet<ValueId>,
    env_values: HashSet<ValueId>,
}

struct RecordPlan {
    alloc: AllocSite,
    bindings: HashSet<String>,
    scalars: Vec<(String, String)>,
    func_family: Vec<HashSet<ValueId>>,
}

fn is_env_var(name: &str) -> bool {
    name == "$env" || name.ends_with(".$env") || name.ends_with(".$shared_env")
}

fn collect_const_strings(function: &Function, constants: &[Constant]) -> HashMap<ValueId, String> {
    let mut strings = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction
                && let Some(Constant::String(text)) = constants.get(constant.0 as usize)
            {
                strings.insert(*dest, text.clone());
            }
        }
    }
    strings
}

fn decode_template_key(constants: &[Constant], raw: u64) -> Option<String> {
    if let Some(index) = value::template_key_name_ref(raw) {
        return match constants.get(index as usize)? {
            Constant::String(text) => Some(text.clone()),
            _ => None,
        };
    }
    const INLINE_NAMESPACE: u64 = 1 << 62;
    if raw & INLINE_NAMESPACE == 0 {
        return None;
    }
    let payload_mask = (1_u64 << (value::INLINE_STRING_MARKER_SHIFT + 3)) - 1;
    let encoded = (value::BOX_BASE | (raw & payload_mask)) as i64;
    let mut buf = [0_u8; 6];
    let bytes = value::decode_inline_string(encoded, &mut buf)?;
    Some(bytes.iter().map(|byte| char::from(*byte)).collect())
}

pub(crate) fn template_key_names(
    constants: &[Constant],
    template: ConstantId,
) -> Option<Vec<String>> {
    let Constant::ObjectTemplate { keys } = constants.get(template.0 as usize)? else {
        return None;
    };
    keys.iter()
        .map(|raw| decode_template_key(constants, *raw))
        .collect()
}

fn collect_allocs(module: &Module, constants: &[Constant]) -> Vec<AllocSite> {
    let mut allocs = Vec::new();
    for (function_index, function) in module.functions().iter().enumerate() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Instruction::InitObjectLiteral {
                    dest,
                    template,
                    values,
                } = instruction
                else {
                    continue;
                };
                let Some(keys) = template_key_names(constants, *template) else {
                    continue;
                };
                if keys.is_empty() || keys.len() != values.len() {
                    continue;
                }
                allocs.push(AllocSite {
                    function_index,
                    dest: *dest,
                    keys,
                });
            }
        }
    }
    allocs
}

fn seed_views(
    module: &Module,
    alloc: &AllocSite,
    constants: &[Constant],
) -> (Vec<FuncView>, HashSet<String>) {
    let mut views: Vec<FuncView> = module
        .functions()
        .iter()
        .map(|_| FuncView {
            family: HashSet::new(),
            env_values: HashSet::new(),
        })
        .collect();
    views[alloc.function_index].family.insert(alloc.dest);
    let mut bindings = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (function_index, function) in module.functions().iter().enumerate() {
            let strings = collect_const_strings(function, constants);
            for block in function.blocks() {
                for instruction in block.instructions() {
                    match instruction {
                        Instruction::LoadVar { dest, name } if is_env_var(name) => {
                            changed |= views[function_index].env_values.insert(*dest);
                        }
                        Instruction::StoreVar { name, value } if is_env_var(name) => {
                            changed |= views[function_index].env_values.insert(*value);
                        }
                        Instruction::StoreVar { name, value }
                            if views[function_index].family.contains(value) =>
                        {
                            changed |= bindings.insert(name.clone());
                        }
                        Instruction::LoadVar { dest, name } if bindings.contains(name) => {
                            changed |= views[function_index].family.insert(*dest);
                        }
                        Instruction::GetProp { dest, object, key }
                            if views[function_index].env_values.contains(object) =>
                        {
                            if strings
                                .get(key)
                                .is_some_and(|key_name| bindings.contains(key_name))
                            {
                                changed |= views[function_index].family.insert(*dest);
                            }
                        }
                        Instruction::Phi { dest, sources }
                            if sources.iter().any(|source| {
                                views[function_index].family.contains(&source.value)
                            }) && sources.iter().all(|source| {
                                views[function_index].family.contains(&source.value)
                            }) =>
                        {
                            changed |= views[function_index].family.insert(*dest);
                        }
                        Instruction::CreateDataProperty { dest, object, .. }
                            if views[function_index].family.contains(object) =>
                        {
                            changed |= views[function_index].family.insert(*dest);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    (views, bindings)
}

fn key_allowed(keys: &[String], name: &str) -> bool {
    keys.iter().any(|key| key == name)
}

fn analyze_function_uses(
    function: &Function,
    view: &FuncView,
    bindings: &HashSet<String>,
    keys: &[String],
    strings: &HashMap<ValueId, String>,
) -> bool {
    let family = &view.family;
    if family.is_empty() {
        return true;
    }
    if function.blocks().iter().any(|block| {
        terminator_uses(block.terminator())
            .into_iter()
            .any(|value| family.contains(&value))
    }) {
        return false;
    }
    for block in function.blocks() {
        for instruction in block.instructions() {
            if !instruction_uses_family(instruction, family) {
                continue;
            }
            if !use_is_allowed(instruction, view, bindings, keys, strings) {
                return false;
            }
        }
    }
    true
}

fn instruction_uses_family(instruction: &Instruction, family: &HashSet<ValueId>) -> bool {
    if instr_uses(instruction)
        .into_iter()
        .any(|value| family.contains(&value))
    {
        return true;
    }
    if let Instruction::Phi { sources, dest, .. } = instruction {
        return family.contains(dest)
            || sources.iter().any(|source| family.contains(&source.value));
    }
    instruction_dest(instruction).is_some_and(|dest| family.contains(&dest))
}

fn use_is_allowed(
    instruction: &Instruction,
    view: &FuncView,
    bindings: &HashSet<String>,
    keys: &[String],
    strings: &HashMap<ValueId, String>,
) -> bool {
    let family = &view.family;
    match instruction {
        Instruction::InitObjectLiteral { dest, .. } if family.contains(dest) => true,
        Instruction::StoreVar { name, value } if family.contains(value) => {
            bindings.contains(name) && !is_env_var(name)
        }
        Instruction::LoadVar { dest, name } if family.contains(dest) => bindings.contains(name),
        Instruction::Phi { dest, sources } if family.contains(dest) => {
            sources.iter().all(|source| family.contains(&source.value))
        }
        Instruction::CreateDataProperty {
            object, key, value, ..
        }
        | Instruction::SetProp {
            object, key, value, ..
        } if family.contains(object) => {
            !family.contains(value)
                && strings
                    .get(key)
                    .is_some_and(|key_name| key_allowed(keys, key_name))
        }
        Instruction::SetProp {
            object, key, value, ..
        } if view.env_values.contains(object) && family.contains(value) => strings
            .get(key)
            .is_some_and(|key_name| bindings.contains(key_name)),
        Instruction::GetProp { object, key, .. } if family.contains(object) => strings
            .get(key)
            .is_some_and(|key_name| key_allowed(keys, key_name)),
        Instruction::GetProp { object, key, dest } if view.env_values.contains(object) => {
            family.contains(dest)
                && strings
                    .get(key)
                    .is_some_and(|key_name| bindings.contains(key_name))
        }
        Instruction::SetProto { object, value } if family.contains(object) => {
            !family.contains(value)
        }
        Instruction::CallBuiltin {
            builtin: Builtin::IsJsObject,
            args,
            dest,
        } if args.iter().any(|value| family.contains(value)) => dest.is_none() && args.len() == 1,
        Instruction::StoreVar { value, .. } if !family.contains(value) => true,
        _ => false,
    }
}

fn store_conflict(module: &Module, bindings: &HashSet<String>, views: &[FuncView]) -> bool {
    for (function_index, function) in module.functions().iter().enumerate() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, value } = instruction
                    && bindings.contains(name)
                    && !views[function_index].family.contains(value)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn has_field_access(
    module: &Module,
    views: &[FuncView],
    keys: &[String],
    constants: &[Constant],
) -> bool {
    for (function_index, function) in module.functions().iter().enumerate() {
        let family = &views[function_index].family;
        if family.is_empty() {
            continue;
        }
        let strings = collect_const_strings(function, constants);
        for block in function.blocks() {
            for instruction in block.instructions() {
                match instruction {
                    Instruction::GetProp { object, key, .. }
                    | Instruction::SetProp { object, key, .. }
                    | Instruction::CreateDataProperty { object, key, .. }
                        if family.contains(object) =>
                    {
                        if strings
                            .get(key)
                            .is_some_and(|key_name| key_allowed(keys, key_name))
                        {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

fn plan_record(module: &Module, alloc: AllocSite, constants: &[Constant]) -> Option<RecordPlan> {
    let (views, bindings) = seed_views(module, &alloc, constants);
    if store_conflict(module, &bindings, &views) {
        return None;
    }
    for (function_index, function) in module.functions().iter().enumerate() {
        let strings = collect_const_strings(function, constants);
        if !analyze_function_uses(
            function,
            &views[function_index],
            &bindings,
            &alloc.keys,
            &strings,
        ) {
            return None;
        }
    }
    if !has_field_access(module, &views, &alloc.keys, constants) {
        return None;
    }
    let owner = bindings
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| format!("$anon.{}.{}", alloc.function_index, alloc.dest.0));
    let scalars = alloc
        .keys
        .iter()
        .map(|key| (key.clone(), format!("$sroa.{owner}.{key}")))
        .collect();
    Some(RecordPlan {
        alloc,
        bindings,
        scalars,
        func_family: views.into_iter().map(|view| view.family).collect(),
    })
}

fn scalar_name<'a>(plan: &'a RecordPlan, key: &str) -> Option<&'a str> {
    plan.scalars
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, name)| name.as_str())
}

fn rewrite_function_to_scalars(
    function: &mut Function,
    plan: &RecordPlan,
    function_index: usize,
    strings: &HashMap<ValueId, String>,
    replacements: &mut HashMap<ValueId, ValueId>,
) {
    let family = &plan.func_family[function_index];
    let is_alloc = function_index == plan.alloc.function_index;
    for block in function.blocks_mut() {
        let original = std::mem::take(block.instructions_mut());
        let mut rewritten = Vec::with_capacity(original.len());
        for instruction in original {
            match instruction {
                Instruction::InitObjectLiteral { dest, values, .. }
                    if is_alloc && dest == plan.alloc.dest =>
                {
                    for (key, value) in plan.alloc.keys.iter().zip(values.into_iter()) {
                        rewritten.push(Instruction::StoreVar {
                            name: scalar_name(plan, key).expect("key").to_string(),
                            value,
                        });
                    }
                }
                Instruction::StoreVar { name, value }
                    if family.contains(&value) && plan.bindings.contains(&name) => {}
                Instruction::LoadVar { dest, name }
                    if family.contains(&dest) && plan.bindings.contains(&name) => {}
                Instruction::GetProp {
                    dest, object, key, ..
                } if family.contains(&object) => {
                    if let Some(key_name) = strings.get(&key)
                        && let Some(name) = scalar_name(plan, key_name)
                    {
                        rewritten.push(Instruction::LoadVar {
                            dest,
                            name: name.to_string(),
                        });
                    }
                }
                Instruction::SetProp {
                    dest,
                    object,
                    key,
                    value,
                } if family.contains(&object) => {
                    if let Some(key_name) = strings.get(&key)
                        && let Some(name) = scalar_name(plan, key_name)
                    {
                        replacements.insert(dest, value);
                        rewritten.push(Instruction::StoreVar {
                            name: name.to_string(),
                            value,
                        });
                    }
                }
                Instruction::CreateDataProperty {
                    dest,
                    object,
                    key,
                    value,
                    ..
                } if family.contains(&object) => {
                    if let Some(key_name) = strings.get(&key)
                        && let Some(name) = scalar_name(plan, key_name)
                    {
                        replacements.insert(dest, value);
                        rewritten.push(Instruction::StoreVar {
                            name: name.to_string(),
                            value,
                        });
                    }
                }
                Instruction::GetProp { key, dest, .. }
                    if strings
                        .get(&key)
                        .is_some_and(|key_name| plan.bindings.contains(key_name))
                        && family.contains(&dest) => {}
                Instruction::SetProp { value, key, .. }
                    if family.contains(&value)
                        && strings
                            .get(&key)
                            .is_some_and(|key_name| plan.bindings.contains(key_name)) => {}
                Instruction::SetProto { object, .. } if family.contains(&object) => {}
                Instruction::CallBuiltin {
                    builtin: Builtin::IsJsObject,
                    args,
                    ..
                } if args.iter().any(|value| family.contains(value)) => {}
                Instruction::Phi { dest, .. } if family.contains(&dest) => {}
                other => rewritten.push(other),
            }
        }
        *block.instructions_mut() = rewritten;
    }
}

fn cyclic_blocks(function: &Function) -> HashSet<BasicBlockId> {
    let n = function.blocks().len();
    let mut cyclic = HashSet::new();
    for block in function.blocks() {
        let start = block.id();
        let mut seen = vec![false; n];
        let mut stack = terminator_successors(block.terminator());
        while let Some(current) = stack.pop() {
            if current == start {
                cyclic.insert(start);
                break;
            }
            let index = current.0 as usize;
            if index >= n || seen[index] {
                continue;
            }
            seen[index] = true;
            stack.extend(terminator_successors(function.blocks()[index].terminator()));
        }
    }
    cyclic
}

fn is_js_call(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Call { .. }
            | Instruction::OptionalCall { .. }
            | Instruction::ConstructCall { .. }
            | Instruction::SuperCall { .. }
    )
}

fn insert_entry_loads(function: &mut Function, plan: &RecordPlan, next_value: &mut u32) {
    let entry = function.entry();
    if cyclic_blocks(function).contains(&entry) {
        return;
    }
    let Some(block) = function.block_by_id_mut(entry) else {
        return;
    };
    let mut loads = Vec::new();
    for (_, name) in &plan.scalars {
        let dest = ValueId(*next_value);
        *next_value = next_value.saturating_add(1);
        loads.push(Instruction::LoadVar {
            dest,
            name: name.clone(),
        });
    }
    let instructions = block.instructions_mut();
    for (offset, load) in loads.into_iter().enumerate() {
        instructions.insert(offset, load);
    }
}

fn collect_scalar_analysis(function: &Function, plan: &RecordPlan) -> CandidateAnalysis {
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    let mut delete_targets = Vec::new();
    let entry = function.entry();
    for block in function.blocks() {
        for (index, instruction) in block.instructions().iter().enumerate() {
            match instruction {
                Instruction::StoreVar { name, value } => {
                    if let Some((key, _)) = plan.scalars.iter().find(|(_, slot)| slot == name) {
                        writes.push(PropertyWrite {
                            key: key.clone(),
                            block: block.id(),
                            index,
                            value: *value,
                        });
                    }
                }
                Instruction::LoadVar { dest, name } => {
                    if let Some((key, _)) = plan.scalars.iter().find(|(_, slot)| slot == name) {
                        if block.id() == entry && index < plan.scalars.len() {
                            writes.push(PropertyWrite {
                                key: key.clone(),
                                block: block.id(),
                                index,
                                value: *dest,
                            });
                        } else {
                            reads.push(PropertyRead {
                                key: key.clone(),
                                block: block.id(),
                                index,
                                dest: *dest,
                            });
                            delete_targets.push((block.id(), index));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    CandidateAnalysis {
        writes,
        reads,
        delete_targets,
        result_replacements: Vec::new(),
        escapes: false,
    }
}

fn insert_phis(function: &mut Function, phis: Vec<PropertyPhi>) {
    for phi in phis {
        if let Some(block) = function.block_by_id_mut(phi.block) {
            block.instructions_mut().insert(
                0,
                Instruction::Phi {
                    dest: phi.dest,
                    sources: phi.sources,
                },
            );
        }
    }
}

fn delete_indices(function: &mut Function, targets: &HashSet<(BasicBlockId, usize)>) {
    let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
    for (block, index) in targets {
        by_block.entry(*block).or_default().push(*index);
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
}

fn predecessors_of(function: &Function) -> Vec<Vec<BasicBlockId>> {
    let mut predecessors = vec![Vec::new(); function.blocks().len()];
    for block in function.blocks() {
        for successor in terminator_successors(block.terminator()) {
            predecessors[successor.0 as usize].push(block.id());
        }
    }
    predecessors
}

fn block_exit_scalars(
    function: &Function,
    plan: &RecordPlan,
    phi_keys: &HashMap<ValueId, String>,
) -> HashMap<BasicBlockId, HashMap<String, ValueId>> {
    let predecessors = predecessors_of(function);
    let mut exit: Vec<HashMap<String, ValueId>> = vec![HashMap::new(); function.blocks().len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            let index = block.id().0 as usize;
            let mut current = HashMap::new();
            for predecessor in &predecessors[index] {
                for (key, value) in &exit[predecessor.0 as usize] {
                    current.insert(key.clone(), *value);
                }
            }
            for instruction in block.instructions() {
                match instruction {
                    Instruction::Phi { dest, .. } => {
                        if let Some(key) = phi_keys.get(dest) {
                            current.insert(key.clone(), *dest);
                        }
                    }
                    Instruction::StoreVar { name, value } => {
                        if let Some((key, _)) = plan.scalars.iter().find(|(_, slot)| slot == name) {
                            current.insert(key.clone(), *value);
                        }
                    }
                    Instruction::LoadVar { dest, name } => {
                        if let Some((key, _)) = plan.scalars.iter().find(|(_, slot)| slot == name) {
                            current.insert(key.clone(), *dest);
                        }
                    }
                    _ => {}
                }
            }
            if exit[index] != current {
                exit[index] = current;
                changed = true;
            }
        }
    }
    function
        .blocks()
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id(), std::mem::take(&mut exit[index])))
        .collect()
}

struct StoreSite {
    block: BasicBlockId,
    at_start: bool,
    before_call: bool,
    values: HashMap<String, ValueId>,
}

fn insert_boundary_stores(
    function: &mut Function,
    plan: &RecordPlan,
    phi_keys: &HashMap<ValueId, String>,
) {
    let exits = block_exit_scalars(function, plan, phi_keys);
    let mut sites = Vec::new();
    for block in function.blocks() {
        let values = exits.get(&block.id()).cloned().unwrap_or_default();
        if matches!(
            block.terminator(),
            Terminator::Return { .. } | Terminator::Throw { .. }
        ) {
            sites.push(StoreSite {
                block: block.id(),
                at_start: false,
                before_call: false,
                values: values.clone(),
            });
        }
        if block.instructions().iter().any(is_js_call) {
            sites.push(StoreSite {
                block: block.id(),
                at_start: false,
                before_call: true,
                values: values.clone(),
            });
        }
    }
    for site in sites {
        let mut stores = Vec::new();
        for (key, name) in &plan.scalars {
            let Some(value) = site.values.get(key).copied() else {
                continue;
            };
            stores.push(Instruction::StoreVar {
                name: name.clone(),
                value,
            });
        }
        if stores.is_empty() {
            continue;
        }
        let Some(block) = function.block_by_id_mut(site.block) else {
            continue;
        };
        let instructions = block.instructions_mut();
        if site.at_start {
            for (offset, store) in stores.into_iter().enumerate() {
                instructions.insert(offset, store);
            }
        } else if site.before_call {
            let insert_at = instructions
                .iter()
                .position(is_js_call)
                .unwrap_or(instructions.len());
            for (offset, store) in stores.into_iter().enumerate() {
                instructions.insert(insert_at + offset, store);
            }
        } else {
            instructions.extend(stores);
        }
    }
}

fn prune_cyclic_stores(function: &mut Function, plan: &RecordPlan) {
    let cyclic = cyclic_blocks(function);
    if cyclic.is_empty() {
        return;
    }
    for block in function.blocks_mut() {
        if !cyclic.contains(&block.id()) {
            continue;
        }
        block.instructions_mut().retain(|instruction| {
            !matches!(instruction, Instruction::StoreVar { name, .. }
                if plan.scalars.iter().any(|(_, slot)| slot == name))
        });
    }
}

fn mem2reg_function(function: &mut Function, plan: &RecordPlan, is_alloc: bool) {
    let mut next_value = next_value_id(function);
    if !is_alloc {
        insert_entry_loads(function, plan, &mut next_value);
    }
    let analysis = collect_scalar_analysis(function, plan);
    if analysis.writes.is_empty() {
        return;
    }
    let Some((mut replacements, mut phis)) =
        resolve_property_replacements(function, &analysis, &mut next_value)
    else {
        return;
    };
    close_replacements(&mut replacements);
    for phi in &mut phis {
        for source in &mut phi.sources {
            if let Some(value) = replacements.get(&source.value) {
                source.value = *value;
            }
        }
    }
    apply_value_replacements(function, &replacements);
    delete_indices(function, &analysis.delete_targets.iter().copied().collect());
    let phi_keys: HashMap<ValueId, String> =
        phis.iter().map(|phi| (phi.dest, phi.key.clone())).collect();
    insert_phis(function, phis);
    prune_cyclic_stores(function, plan);
    insert_boundary_stores(function, plan, &phi_keys);
}

fn apply_plan(module: &mut Module, plan: &RecordPlan, constants: &[Constant]) {
    let mut per_func_strings = Vec::new();
    for function in module.functions() {
        per_func_strings.push(collect_const_strings(function, constants));
    }
    for function_index in 0..module.functions().len() {
        if plan.func_family[function_index].is_empty()
            && function_index != plan.alloc.function_index
        {
            continue;
        }
        let mut replacements = HashMap::new();
        let function_id = FunctionId(function_index as u32);
        let function = module
            .function_mut(function_id)
            .expect("function id must be valid");
        rewrite_function_to_scalars(
            function,
            plan,
            function_index,
            &per_func_strings[function_index],
            &mut replacements,
        );
        if !replacements.is_empty() {
            close_replacements(&mut replacements);
            apply_value_replacements(function, &replacements);
        }
        mem2reg_function(function, plan, function_index == plan.alloc.function_index);
    }
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
    let allocs = collect_allocs(module, &constants);
    for alloc in allocs {
        let Some(plan) = plan_record(module, alloc, &constants) else {
            continue;
        };
        apply_plan(module, &plan, &constants);
    }
}

#[cfg(test)]
mod tests {
    use wjsm_ir::{BasicBlock, Function, Program, Terminator};

    use super::*;

    fn sso_key(text: &str) -> u64 {
        let encoded = value::encode_inline_ascii(text.as_bytes()).expect("sso key");
        value::inline_property_key_raw(encoded).expect("property key")
    }

    fn count_prop_access(function: &Function) -> usize {
        function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::GetProp { .. } | Instruction::SetProp { .. }
                )
            })
            .count()
    }

    #[test]
    fn scalar_replaces_module_record_reads() {
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name"), sso_key("value"), sso_key("length")],
        });
        let key_name = program.add_constant(Constant::String("name".into()));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        for (dest, number) in [(ValueId(0), 0.0), (ValueId(1), 1.0), (ValueId(2), 2.0)] {
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
            name: "$0.RECORD".into(),
            value: ValueId(3),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$0.RECORD".into(),
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
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(6)),
        });
        function.push_block(block);
        program.push_function(function);

        run(&mut program);

        let function = &program.functions()[0];
        assert_eq!(count_prop_access(function), 0);
        assert!(
            !function
                .blocks()
                .iter()
                .flat_map(|block| block.instructions())
                .any(|instruction| matches!(instruction, Instruction::InitObjectLiteral { .. })),
            "allocation should be scalar-replaced"
        );
    }
}
