//! licm 的 elem-guard 子阶段：`POINTS[i].x` 型「逐迭代变化 receiver」访问的
//! Inline Cache 守卫外提。
//!
//! 与常量键 GetProp 外提（receiver 循环不变，整条指令搬进 pre-header）不同，
//! `POINTS[i]` 每次迭代都是不同对象，指令本身不能移动。本阶段改为在
//! pre-header 插入一条带模板的 [`Instruction::GuardElementsKind`]：宿主一次性校验
//! 「数组 packed 无洞、全部元素持有同一烘焙模板 shape、元素值槽均非对象」，
//! 并把循环体内成对的 `GetElem`/`GetProp` 带上共享守卫值闩锁。
//! 守卫为真时 `GetProp` 快路径跳过逐迭代 tag/shape/proto 检查、直接按模板
//! 槽偏移单指令读取；任何可能执行用户代码的宿主回退路径都先把守卫值置
//! false（单向闩锁），之后所有访问退回通用 IC 路径，语义与未优化完全一致。
//!
//! 静态前提（任一不成立则整个循环放弃，一条也不替换）：
//!
//!     1. 候选 `GetElem` 的 receiver 定义在循环体外（往轮 licm 已把可外提的
//!    LoadVar 搬进 pre-header），且是「单赋值数组绑定」的 LoadVar 或
//!    LoadEnvSlot：该绑定全模块只有一次 StoreVar，写入同函数内 `NewArray +
//!    builtin.array.push(InitObjectLiteral)` 构造的字面量数组，或
//!    `Array.from` + 统一类构造器（自有数据键序一致）。绑定被改写过的数组
//!    在运行期由守卫重验兜底，这里只需要一个可信的模板来源。
//! 2. 候选 `GetProp` 的 receiver 是候选 `GetElem` 的 dest，键是模板自有键，
//!    或编译期识别的 hypot getter 属性（操作数键落在模板自有键上）。
//! 3. 循环体全部指令通过「守卫为真期间不执行用户代码」白名单：协变运算
//!    （Binary / 关系比较 / 字符串拼接 / abstract_compare）的操作数必须落在
//!    「守卫为真时必为原始值」集合内——数字值类证明、原始常量、Guarded 读取
//!    结果（守卫校验过元素值槽非对象）、单赋值数组绑定的 `length` 读取、
//!    以及只写原始值的本函数绑定。守卫为真 ⇒ 无 ToPrimitive 回调可达；
//!    守卫为假 ⇒ 即使触发用户代码，全部快路径已关闭，通用路径语义自洽。

use std::collections::{BTreeMap, HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Builtin, Constant, ConstantId, Function, Instruction, Module, Terminator,
    UnaryOp, ValueId,
};

use super::cfg_fold::terminator_successors;
use super::escape_scalar_record::template_key_names;
use super::hypot_getter::{collect_hypot_getters, hypot_own_slots_for_property};
use super::licm::LoopView;
use super::licm_apply::ElemGuard;
use super::licm_facts::{ModuleFacts, is_protocol_or_env_name};

// ── 模块级事实：单赋值数组绑定 → 元素模板 ───────────────────────────────

/// 数组字面量构造的跟踪状态。
enum ArrayState {
    /// 尚未 push 任何元素。
    Empty,
    /// 已 push 的元素全部是同键序模板的对象字面量。
    Uniform(ConstantId, Vec<String>),
    /// 出现非对象字面量元素或键序冲突，放弃。
    Poisoned,
}

/// 收集「全模块恰好一次 StoreVar、写入值是同函数内统一模板对象字面量数组
/// 或 `Array.from` + 统一类构造器」的绑定 → 元素模板。模板只是守卫的静态
/// 候选：绑定或数组内容后续被改写时，运行期 `GuardElementsKind`（带模板）
/// 校验失败、退回通用路径，正确性不受影响。
pub(crate) fn stable_elem_array_bindings(module: &mut Module) -> HashMap<String, ConstantId> {
    let mut store_sites: HashMap<String, u32> = HashMap::new();
    for function in module.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, .. } = instruction {
                    *store_sites.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut bindings = HashMap::new();
    for function in module.functions() {
        collect_function_array_bindings(module, function, &store_sites, &mut bindings);
    }
    super::licm_array_from::add_array_from_bindings(module, &store_sites, &mut bindings);
    bindings
}

fn collect_function_array_bindings(
    module: &Module,
    function: &Function,
    store_sites: &HashMap<String, u32>,
    bindings: &mut HashMap<String, ConstantId>,
) {
    let constants = module.constants();
    let mut literal_templates: HashMap<ValueId, ConstantId> = HashMap::new();
    let mut arrays: HashMap<ValueId, ArrayState> = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::NewArray { dest, .. } => {
                    arrays.insert(*dest, ArrayState::Empty);
                }
                Instruction::InitObjectLiteral { dest, template, .. } => {
                    literal_templates.insert(*dest, *template);
                }
                Instruction::CallBuiltin {
                    builtin: Builtin::ArrayPush,
                    args,
                    ..
                } if args.len() == 2 => {
                    if let Some(state) = arrays.get_mut(&args[0]) {
                        push_element(constants, state, literal_templates.get(&args[1]).copied());
                    }
                }
                Instruction::CallBuiltin {
                    builtin: Builtin::ArrayPushHole | Builtin::ArrayPushSpread,
                    args,
                    ..
                } => {
                    if let Some(state) = args.first().and_then(|array| arrays.get_mut(array)) {
                        *state = ArrayState::Poisoned;
                    }
                }
                Instruction::StoreVar { name, value } => {
                    if let Some(ArrayState::Uniform(template, _)) = arrays.get(value)
                        && store_sites.get(name).copied() == Some(1)
                    {
                        bindings.insert(name.clone(), *template);
                    }
                }
                _ => {}
            }
        }
    }
}

/// 追加一个元素：必须是对象字面量且键序与既有元素一致。
fn push_element(constants: &[Constant], state: &mut ArrayState, template: Option<ConstantId>) {
    let keys = template.and_then(|template| template_key_names(constants, template));
    match (
        std::mem::replace(state, ArrayState::Poisoned),
        template,
        keys,
    ) {
        (ArrayState::Empty, Some(template), Some(keys)) if !keys.is_empty() => {
            *state = ArrayState::Uniform(template, keys);
        }
        (ArrayState::Uniform(existing, existing_keys), Some(_), Some(keys))
            if existing_keys == keys =>
        {
            *state = ArrayState::Uniform(existing, existing_keys);
        }
        _ => {}
    }
}

// ── 循环级候选识别与安全性 ───────────────────────────────────────────────

/// elem 候选：`GetElem` 的 dest（及循环内 LoadVar 别名）→ 守卫信息。
#[derive(Clone, Copy)]
struct ElemCandidate {
    array: ValueId,
    template: ConstantId,
    site: (BasicBlockId, usize),
    /// 原始 `GetElem` dest；别名与源头共享，便于按数组分组时只插一次站点。
    origin: ValueId,
}

/// 为一个循环规划全部 elem-guard；候选存在但白名单不过时返回空。
pub(crate) fn plan_elem_guards(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
) -> Vec<ElemGuard> {
    if view.has_suspend {
        return Vec::new();
    }
    let mut elems = collect_elem_candidates(module, view, facts);
    alias_elem_load_vars(view, &mut elems);
    if elems.is_empty() {
        return Vec::new();
    }
    let props = collect_prop_candidates(module, view, &elems);
    if props.is_empty() {
        return Vec::new();
    }
    // 只保留有 GetProp 支撑的 GetElem：守卫收益来自属性读取快路径。
    let used: HashSet<ValueId> = props
        .iter()
        .filter_map(|(dest, _)| elems.get(dest).map(|candidate| candidate.origin))
        .collect();
    let elem_sites: HashSet<(BasicBlockId, usize)> = elems
        .iter()
        .filter(|(dest, candidate)| **dest == candidate.origin && used.contains(dest))
        .map(|(_, candidate)| candidate.site)
        .collect();
    let prop_sites: HashSet<(BasicBlockId, usize)> = props.iter().map(|(_, site)| *site).collect();
    let method_sites = method_load_sites(view, &elems);
    let prop_dests = prop_dest_set(view, &prop_sites);
    let prim = primitive_values(module, view, facts, &prop_dests);
    let safe_blocks = guard_true_body_blocks(view);
    if !body_safe_with_guard(
        view,
        facts,
        &prim,
        &elem_sites,
        &prop_sites,
        &method_sites,
        &safe_blocks,
    ) {
        return Vec::new();
    }
    group_guards(&elems, &props, &used)
}

/// 守卫 array 若定义在循环体内，把它及其循环内操作数（`LoadVar $env` /
/// 常量键 / `LoadEnvSlot`）一并外提。只读、无用户代码。
pub(crate) fn array_loads_to_hoist(
    view: &LoopView<'_>,
    guards: &[ElemGuard],
) -> Vec<(BasicBlockId, usize)> {
    let mut sites = HashSet::new();
    for guard in guards {
        collect_in_loop_def_chain(view, guard.array, &mut sites);
    }
    let mut ordered: Vec<(BasicBlockId, usize)> = sites.into_iter().collect();
    ordered.sort_unstable_by_key(|(block, index)| (block.0, *index));
    ordered
}

fn collect_in_loop_def_chain(
    view: &LoopView<'_>,
    value: ValueId,
    sites: &mut HashSet<(BasicBlockId, usize)>,
) {
    let Some((block, index)) = view.defs.get(&value).copied() else {
        return;
    };
    if !view.body.contains(&block) || !sites.insert((block, index)) {
        return;
    }
    let Some(instruction) = view
        .function
        .block_by_id(block)
        .and_then(|block| block.instructions().get(index))
    else {
        return;
    };
    match instruction {
        Instruction::LoadEnvSlot { env, key, .. } => {
            collect_in_loop_def_chain(view, *env, sites);
            collect_in_loop_def_chain(view, *key, sites);
        }
        Instruction::LoadVar { .. } | Instruction::Const { .. } => {}
        _ => {
            sites.remove(&(block, index));
        }
    }
}

/// 候选 `GetElem`：receiver 定义在循环体外，且定义是单赋值数组绑定的 LoadVar。
fn collect_elem_candidates(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
) -> HashMap<ValueId, ElemCandidate> {
    let mut candidates = HashMap::new();
    for block in view.function.blocks() {
        if !view.body.contains(&block.id()) {
            continue;
        }
        for (index, instruction) in block.instructions().iter().enumerate() {
            let Instruction::GetElem {
                dest,
                object,
                latch,
                ..
            } = instruction
            else {
                continue;
            };
            // 已带闩锁的站点是往轮外提产物，再规划会空插 pre-header。
            if latch.is_some() {
                continue;
            };
            let Some(template) = stable_array_template(view, facts, *object) else {
                continue;
            };
            if template_key_names(module.constants(), template).is_none() {
                continue;
            }
            candidates.insert(
                *dest,
                ElemCandidate {
                    array: *object,
                    template,
                    site: (block.id(), index),
                    origin: *dest,
                },
            );
        }
    }
    candidates
}

/// `value` 是否是单赋值数组绑定：循环外 LoadVar，或循环内对捕获槽的
/// `LoadEnvSlot`（`$env` 形参循环不变，键是绑定名）。
fn stable_array_template(
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    value: ValueId,
) -> Option<ConstantId> {
    let (block, index) = view.defs.get(&value).copied()?;
    let instruction = view
        .function
        .block_by_id(block)?
        .instructions()
        .get(index)?;
    match instruction {
        Instruction::LoadVar { name, .. } if !view.body.contains(&block) => {
            facts.elem_array_templates.get(name).copied()
        }
        Instruction::LoadEnvSlot { env, key, .. } => {
            if !env_is_loop_invariant(view, *env) {
                return None;
            }
            let name = view.strings.get(key)?;
            facts.elem_array_templates.get(name).copied()
        }
        _ => None,
    }
}

fn env_is_loop_invariant(view: &LoopView<'_>, env: ValueId) -> bool {
    let Some((block, index)) = view.defs.get(&env).copied() else {
        return false;
    };
    if !view.body.contains(&block) {
        return true;
    }
    matches!(
        view.function
            .block_by_id(block)
            .and_then(|block| block.instructions().get(index)),
        Some(Instruction::LoadVar { name, .. })
            if name == "$env" || name.ends_with(".$env")
    )
}

/// `const p = POINTS[i]`：循环内 LoadVar 的绑定若全部 StoreVar 写入值都是
/// 同一源头的 elem dest（或已解析别名），则 LoadVar dest 也视为该元素。
fn alias_elem_load_vars(view: &LoopView<'_>, elems: &mut HashMap<ValueId, ElemCandidate>) {
    let mut stores: HashMap<String, Vec<ValueId>> = HashMap::new();
    for block in view.function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction {
                stores.entry(name.clone()).or_default().push(*value);
            }
        }
    }
    loop {
        let mut changed = false;
        for block in view.function.blocks() {
            if !view.body.contains(&block.id()) {
                continue;
            }
            for instruction in block.instructions() {
                changed |= try_alias_load_var(elems, &stores, instruction);
            }
        }
        if !changed {
            return;
        }
    }
}

fn try_alias_load_var(
    elems: &mut HashMap<ValueId, ElemCandidate>,
    stores: &HashMap<String, Vec<ValueId>>,
    instruction: &Instruction,
) -> bool {
    let Instruction::LoadVar { dest, name } = instruction else {
        return false;
    };
    if elems.contains_key(dest) {
        return false;
    }
    let Some(values) = stores.get(name) else {
        return false;
    };
    if values.is_empty() || !values.iter().all(|value| elems.contains_key(value)) {
        return false;
    }
    let origin = elems[&values[0]].origin;
    if !values.iter().all(|value| elems[value].origin == origin) {
        return false;
    }
    let source = elems[&values[0]].clone();
    elems.insert(*dest, source);
    true
}

/// 阶段 C 方法加载：`GetProp` dest 被同循环 `GuardSameFunction` 消费。
/// 不闩锁（不是模板自有键），只从白名单豁免——pre-header 会核对原型身份。
fn method_load_sites(
    view: &LoopView<'_>,
    elems: &HashMap<ValueId, ElemCandidate>,
) -> HashSet<(BasicBlockId, usize)> {
    let callees: HashSet<ValueId> = view
        .function
        .blocks()
        .iter()
        .filter(|block| view.body.contains(&block.id()))
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::GuardSameFunction { callee, .. } => Some(*callee),
            _ => None,
        })
        .collect();
    let mut sites = HashSet::new();
    for block in view.function.blocks() {
        if !view.body.contains(&block.id()) {
            continue;
        }
        for (index, instruction) in block.instructions().iter().enumerate() {
            let Instruction::GetProp {
                dest,
                object,
                latch,
                ..
            } = instruction
            else {
                continue;
            };
            if latch.is_some() || !callees.contains(dest) || !elems.contains_key(object) {
                continue;
            }
            sites.insert((block.id(), index));
        }
    }
    sites
}

/// 守卫为真时可达的循环体：不走 `GuardSameFunction` 的 false 边。
/// 阶段 C 失败路径含 Call / 未内联 GetProp，不能出现在守卫为真的前缀里。
fn guard_true_body_blocks(view: &LoopView<'_>) -> HashSet<BasicBlockId> {
    let mut skip: HashSet<(u32, u32)> = HashSet::new();
    for block in view.function.blocks() {
        if !view.body.contains(&block.id()) {
            continue;
        }
        let Some(dest) =
            block
                .instructions()
                .iter()
                .rev()
                .find_map(|instruction| match instruction {
                    Instruction::GuardSameFunction { dest, .. } => Some(*dest),
                    _ => None,
                })
        else {
            continue;
        };
        if let Terminator::Branch {
            condition,
            false_block,
            ..
        } = block.terminator()
            && *condition == dest
        {
            skip.insert((block.id().0, false_block.0));
        }
    }
    let mut seen = HashSet::new();
    let mut stack = vec![view.header];
    while let Some(block_id) = stack.pop() {
        if !view.body.contains(&block_id) || !seen.insert(block_id) {
            continue;
        }
        let Some(block) = view.function.block_by_id(block_id) else {
            continue;
        };
        for successor in terminator_successors(block.terminator()) {
            if skip.contains(&(block_id.0, successor.0)) {
                continue;
            }
            stack.push(successor);
        }
    }
    seen
}

/// 候选 `GetProp`：receiver 是候选 elem 的 dest，键是该模板的自有键，
/// 或 hypot getter 属性（双操作数键均在模板上）。
fn collect_prop_candidates(
    module: &Module,
    view: &LoopView<'_>,
    elems: &HashMap<ValueId, ElemCandidate>,
) -> Vec<(ValueId, (BasicBlockId, usize))> {
    let hypot_getters = collect_hypot_getters(module);
    let mut props = Vec::new();
    for block in view.function.blocks() {
        if !view.body.contains(&block.id()) {
            continue;
        }
        for (index, instruction) in block.instructions().iter().enumerate() {
            let Instruction::GetProp {
                object, key, latch, ..
            } = instruction
            else {
                continue;
            };
            if latch.is_some() {
                continue;
            };
            let Some(candidate) = elems.get(object) else {
                continue;
            };
            let Some(key_text) = view.strings.get(key) else {
                continue;
            };
            let Some(keys) = template_key_names(module.constants(), candidate.template) else {
                continue;
            };
            let owns_key = keys.iter().any(|name| name == key_text);
            let hypot_key = hypot_own_slots_for_property(&hypot_getters, key_text, &keys).is_some();
            if owns_key || hypot_key {
                props.push((*object, (block.id(), index)));
            }
        }
    }
    props
}

fn prop_dest_set(
    view: &LoopView<'_>,
    prop_sites: &HashSet<(BasicBlockId, usize)>,
) -> HashSet<ValueId> {
    let mut dests = HashSet::new();
    for (block, index) in prop_sites {
        if let Some(Instruction::GetProp { dest, .. }) = view
            .function
            .block_by_id(*block)
            .and_then(|block| block.instructions().get(*index))
        {
            dests.insert(*dest);
        }
    }
    dests
}

/// 按 array 值分组为最终守卫计划（BTreeMap 保证输出确定性）。
fn group_guards(
    elems: &HashMap<ValueId, ElemCandidate>,
    props: &[(ValueId, (BasicBlockId, usize))],
    used: &HashSet<ValueId>,
) -> Vec<ElemGuard> {
    let mut grouped: BTreeMap<u32, ElemGuard> = BTreeMap::new();
    for (dest, candidate) in elems {
        if !used.contains(dest) {
            continue;
        }
        grouped
            .entry(candidate.array.0)
            .or_insert_with(|| ElemGuard {
                array: candidate.array,
                template: candidate.template,
                elem_sites: Vec::new(),
                prop_sites: Vec::new(),
            })
            .elem_sites
            .push(candidate.site);
    }
    for (elem_dest, site) in props {
        if let Some(candidate) = elems.get(elem_dest)
            && let Some(guard) = grouped.get_mut(&candidate.array.0)
        {
            guard.prop_sites.push(*site);
        }
    }
    let mut guards: Vec<ElemGuard> = grouped.into_values().collect();
    for guard in &mut guards {
        guard
            .elem_sites
            .sort_unstable_by_key(|(block, index)| (block.0, *index));
        guard
            .prop_sites
            .sort_unstable_by_key(|(block, index)| (block.0, *index));
    }
    guards
}

// ── 「守卫为真时必为原始值」集合 ─────────────────────────────────────────

/// 不动点求出当前函数内「守卫为真的执行前缀中必为原始值」的 SSA 值集合。
///
/// 结构性成员（结果类型恒为原始值，与执行路径无关）：原始常量、Binary /
/// Unary / Compare / 字符串拼接 / IsException 的 dest、只读谓词与
/// abstract_compare 等白名单 builtin 的 dest。条件性成员：候选 `GetProp`
/// 的 dest（守卫校验过元素值槽非对象）、单赋值数组绑定的 `length` 读取、
/// 全部来源已入集的 Phi、以及「全模块写站点都在本函数且写入值已入集」的
/// 非参数绑定 LoadVar。
fn primitive_values(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    guarded_prop_dests: &HashSet<ValueId>,
) -> HashSet<ValueId> {
    let constants = module.constants();
    let function = view.function;
    let mut prim: HashSet<ValueId> = facts
        .numbers
        .get(&(view.func_idx as u32))
        .map(|classes| classes.numbers.iter().copied().collect())
        .unwrap_or_default();
    prim.extend(guarded_prop_dests.iter().copied());
    let mut prim_bindings: HashSet<&str> = HashSet::new();
    loop {
        let mut changed = false;
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Some(dest) =
                    prim_dest(view, facts, constants, instruction, &prim, &prim_bindings)
                    && prim.insert(dest)
                {
                    changed = true;
                }
            }
        }
        for name in binding_candidates(function) {
            if !prim_bindings.contains(name) && binding_is_primitive(view, facts, name, &prim) {
                prim_bindings.insert(name);
                changed = true;
            }
        }
        if !changed {
            return prim;
        }
    }
}

/// 一条指令的 dest 是否可加入原始值集合（见 [`primitive_values`]）。
fn prim_dest(
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    constants: &[Constant],
    instruction: &Instruction,
    prim: &HashSet<ValueId>,
    prim_bindings: &HashSet<&str>,
) -> Option<ValueId> {
    match instruction {
        Instruction::Const { dest, constant } => {
            primitive_constant(constants, *constant).then_some(*dest)
        }
        Instruction::Binary { dest, .. }
        | Instruction::Unary { dest, .. }
        | Instruction::Compare { dest, .. }
        | Instruction::StringConcatVa { dest, .. }
        | Instruction::IsException { dest, .. } => Some(*dest),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            ..
        } if builtin_result_primitive(*builtin) => Some(*dest),
        Instruction::Phi { dest, sources } if !sources.is_empty() => sources
            .iter()
            .all(|source| prim.contains(&source.value))
            .then_some(*dest),
        Instruction::LoadVar { dest, name } => {
            prim_bindings.contains(name.as_str()).then_some(*dest)
        }
        Instruction::GetProp {
            dest, object, key, ..
        } => stable_array_length_read(view, facts, *object, *key).then_some(*dest),
        _ => None,
    }
}

fn primitive_constant(constants: &[Constant], constant: ConstantId) -> bool {
    matches!(
        constants.get(constant.0 as usize),
        Some(
            Constant::Number(_)
                | Constant::String(_)
                | Constant::Bool(_)
                | Constant::Null
                | Constant::Undefined
                | Constant::BigInt(_)
        )
    )
}

/// 白名单 builtin 的返回值恒为原始值（布尔 / undefined / 异常编码）。
fn builtin_result_primitive(builtin: Builtin) -> bool {
    matches!(
        builtin,
        Builtin::AbstractCompare
            | Builtin::ToBoolean
            | Builtin::IsJsObject
            | Builtin::ConsoleLog
            | Builtin::ConsoleInfo
            | Builtin::ConsoleDebug
            | Builtin::ConsoleWarn
            | Builtin::ConsoleError
            | Builtin::ConsoleTrace
            | Builtin::MathHypot
            | Builtin::ExceptionValue
    )
}

/// `GetProp(object, "length")` 且 object 是单赋值数组绑定的 LoadVar：宿主对
/// 数组 `length` 直读长度、对 undefined 直接返回 undefined，均不执行用户代码，
/// 结果必为原始值。绑定单赋值 ⇒ 值只可能是该数组或 TDZ 前的 undefined。
fn stable_array_length_read(
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    object: ValueId,
    key: ValueId,
) -> bool {
    if view.strings.get(&key).map(String::as_str) != Some("length") {
        return false;
    }
    let Some((block, index)) = view.defs.get(&object).copied() else {
        return false;
    };
    let Some(instruction) = view
        .function
        .block_by_id(block)
        .and_then(|block| block.instructions().get(index))
    else {
        return false;
    };
    let name = match instruction {
        Instruction::LoadVar { name, .. } => name.as_str(),
        Instruction::LoadEnvSlot { key, .. } => match view.strings.get(key) {
            Some(name) => name.as_str(),
            None => return false,
        },
        _ => return false,
    };
    facts.elem_array_templates.contains_key(name)
}

fn binding_candidates(function: &Function) -> Vec<&str> {
    let mut names = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::LoadVar { name, .. } = instruction {
                names.push(name.as_str());
            }
        }
    }
    names
}

/// 绑定的读取是否必为原始值：非协议槽、非本函数形参，全模块写站点都在本
/// 函数内且写入值已证明原始（无站点 ⇒ 只能读到 undefined）。
///
/// direct eval 不构成漏网写者：eval 桥接对每个可见绑定发出
/// `eval_get_binding` + StoreVar 写回，可被 eval 改写的绑定必然带有
/// 一个值不可证明原始的写站点，本检查自动拒绝。
fn binding_is_primitive(
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    name: &str,
    prim: &HashSet<ValueId>,
) -> bool {
    if is_protocol_or_env_name(name) || view.function.params().iter().any(|param| param == name) {
        return false;
    }
    match facts.stores.get(name) {
        None => true,
        Some(sites) => {
            sites.iter().all(|site| site.func == view.func_idx)
                && view.function.blocks().iter().all(|block| {
                    block.instructions().iter().all(|instruction| {
                        !matches!(
                            instruction,
                            Instruction::StoreVar { name: stored, value }
                                if stored == name && !prim.contains(value)
                        )
                    })
                })
        }
    }
}

// ── 循环体白名单 ─────────────────────────────────────────────────────────

/// 循环体在「守卫为真」期间不可能执行用户代码：候选站点自身（替换后其
/// 宿主回退路径会先熄灭守卫）、无协变的纯指令、操作数已证明原始的协变
/// 运算、稳定 record 家族的属性读取、单赋值数组绑定的 `length` 读取，
/// 以及只读渲染类 builtin。其余一律拒绝。
fn body_safe_with_guard(
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    prim: &HashSet<ValueId>,
    elem_sites: &HashSet<(BasicBlockId, usize)>,
    prop_sites: &HashSet<(BasicBlockId, usize)>,
    method_sites: &HashSet<(BasicBlockId, usize)>,
    safe_blocks: &HashSet<BasicBlockId>,
) -> bool {
    let numbers = facts.numbers.get(&(view.func_idx as u32));
    let number_proved =
        |value: &ValueId| numbers.is_some_and(|classes| classes.numbers.contains(value));
    let in_family = |value: &ValueId| {
        facts
            .records
            .iter()
            .any(|record| record.family[view.func_idx].contains(value))
    };
    for block in view.function.blocks() {
        if !view.body.contains(&block.id()) || !safe_blocks.contains(&block.id()) {
            continue;
        }
        for (index, instruction) in block.instructions().iter().enumerate() {
            if elem_sites.contains(&(block.id(), index))
                || prop_sites.contains(&(block.id(), index))
                || method_sites.contains(&(block.id(), index))
            {
                continue;
            }
            let safe = match instruction {
                Instruction::Const { .. }
                | Instruction::Phi { .. }
                | Instruction::LoadVar { .. }
                | Instruction::LoadEnvSlot { .. }
                | Instruction::StoreVar { .. }
                | Instruction::IsException { .. }
                | Instruction::GuardSameFunction { .. }
                | Instruction::DebugCheck { .. } => true,
                Instruction::Unary {
                    op: UnaryOp::Not | UnaryOp::Void | UnaryOp::IsNullish,
                    ..
                } => true,
                Instruction::Unary {
                    op: UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot,
                    dest,
                    value,
                } => number_proved(dest) || prim.contains(value),
                Instruction::Binary { dest, lhs, rhs, .. } => {
                    number_proved(dest) || (prim.contains(lhs) && prim.contains(rhs))
                }
                Instruction::Compare { op, lhs, rhs, .. } => {
                    !op.is_relational() || (prim.contains(lhs) && prim.contains(rhs))
                }
                Instruction::StringConcatVa { parts, .. } => {
                    parts.iter().all(|part| prim.contains(part))
                }
                Instruction::GetProp { object, key, .. } => {
                    in_family(object) || stable_array_length_read(view, facts, *object, *key)
                }
                Instruction::GetElem { object, .. } => in_family(object),
                Instruction::CallBuiltin { builtin, args, .. } => match builtin {
                    Builtin::ConsoleLog
                    | Builtin::ConsoleInfo
                    | Builtin::ConsoleDebug
                    | Builtin::ConsoleWarn
                    | Builtin::ConsoleError
                    | Builtin::ConsoleTrace
                    | Builtin::IsJsObject
                    | Builtin::ToBoolean
                    | Builtin::ExceptionValue => true,
                    Builtin::AbstractCompare => args.iter().all(|arg| prim.contains(arg)),
                    Builtin::MathHypot => args.iter().all(|arg| prim.contains(arg)),
                    _ => false,
                },
                _ => false,
            };
            if !safe {
                return false;
            }
        }
    }
    true
}
