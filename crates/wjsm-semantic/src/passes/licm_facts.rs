//! licm pass 的模块级事实收集：绑定写站点、稳定 record 家族、可外提纯函数。
//!
//! 这些事实与块结构解耦（只引用稳定的块 id 与 ValueId），licm 的多轮变换只
//! 移动 Const/LoadVar/GetProp/Call（从不移动写指令），因此一次收集全程有效。

use std::collections::{HashMap, HashSet};

use wjsm_ir::value_class::{self, ValueClassSet};
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, Instruction, Module, Terminator, UnaryOp, ValueId,
};

use super::direct_call::{instr_uses, terminator_uses};
use super::escape_scalar_record::template_key_names;

/// 环境/协议槽：读写它们不是普通绑定语义，一律不参与外提。
pub(crate) fn is_protocol_or_env_name(name: &str) -> bool {
    matches!(name, "$this" | "$env" | wjsm_ir::EVAL_SCOPE_ENV_PARAM)
        || name.ends_with(".$env")
        || name.ends_with(".$shared_env")
}

/// 一个「稳定 record」：模板字面量分配，模块内全部使用都在白名单内，
/// 且属性写只发生在分配函数里经 alloc dest 直达的初始化路径上。
///
/// 满足这些条件时对象自始至终只有自有数据属性（不可能被 defineProperty 挂
/// getter），任何用户代码都拿不到能写它的引用——循环体内哪怕有调用/协变
/// 求值，也不可能改写它的属性值，常量键读取因此可以整体外提到 pre-header。
pub(crate) struct RecordFacts {
    /// 模板自有键名（解码后的文本）。
    pub(crate) keys: Vec<String>,
    /// 每函数的家族值集合（alloc dest、绑定 LoadVar 结果、全家族 Phi 等）。
    pub(crate) family: Vec<HashSet<ValueId>>,
    /// 每函数含家族属性写的块（用于「循环体内无写」检查）。
    pub(crate) write_blocks: Vec<HashSet<BasicBlockId>>,
}

/// StoreVar 站点：函数下标 + 块 id。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreSite {
    pub(crate) func: usize,
    pub(crate) block: BasicBlockId,
}

/// licm 消费的模块级事实。
pub(crate) struct ModuleFacts {
    /// 绑定名 → 全模块 StoreVar 站点。
    pub(crate) stores: HashMap<String, Vec<StoreSite>>,
    /// 合格的稳定 record。
    pub(crate) records: Vec<RecordFacts>,
    /// 每函数的值类集合（证明 Binary/Unary 不触发 ToPrimitive 用）。
    pub(crate) numbers: HashMap<u32, ValueClassSet>,
    /// 每函数是否为「可证明终止的纯函数」（T1：无环、无调用、无状态读写）。
    pub(crate) pure_callees: Vec<bool>,
}

impl ModuleFacts {
    pub(crate) fn build(module: &Module) -> Self {
        let numbers = value_class::infer_program(module);
        let pure_callees = module
            .functions()
            .iter()
            .enumerate()
            .map(|(index, function)| {
                let empty = ValueClassSet::default();
                let classes = numbers.get(&(index as u32)).unwrap_or(&empty);
                function_is_terminating_pure(function, classes)
            })
            .collect();
        Self {
            stores: collect_store_sites(module),
            records: collect_records(module),
            numbers,
            pure_callees,
        }
    }
}

fn collect_store_sites(module: &Module) -> HashMap<String, Vec<StoreSite>> {
    let mut stores: HashMap<String, Vec<StoreSite>> = HashMap::new();
    for (func, function) in module.functions().iter().enumerate() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, .. } = instruction {
                    stores.entry(name.clone()).or_default().push(StoreSite {
                        func,
                        block: block.id(),
                    });
                }
            }
        }
    }
    stores
}

// ── 稳定 record 家族分析 ────────────────────────────────────────────────

/// 家族传播的中间态。
struct FamilySeed {
    family: Vec<HashSet<ValueId>>,
    bindings: HashSet<String>,
}

fn collect_records(module: &Module) -> Vec<RecordFacts> {
    let constants = module.constants();
    let mut records = Vec::new();
    for (alloc_func, function) in module.functions().iter().enumerate() {
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
                if let Some(record) = qualify_record(module, alloc_func, *dest, keys) {
                    records.push(record);
                }
            }
        }
    }
    records
}

/// 家族不动点：alloc dest 经绑定 StoreVar/LoadVar、全家族 Phi、
/// CreateDataProperty 结果传播（与 escape_scalar_record 的 seed 同思想，
/// 不追 env 别名——env 参与直接判失败）。
fn seed_family(module: &Module, alloc_func: usize, alloc_dest: ValueId) -> FamilySeed {
    let mut family: Vec<HashSet<ValueId>> =
        module.functions().iter().map(|_| HashSet::new()).collect();
    family[alloc_func].insert(alloc_dest);
    let mut bindings = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (func, function) in module.functions().iter().enumerate() {
            for block in function.blocks() {
                for instruction in block.instructions() {
                    match instruction {
                        Instruction::StoreVar { name, value } if family[func].contains(value) => {
                            changed |= bindings.insert(name.clone());
                        }
                        Instruction::LoadVar { dest, name } if bindings.contains(name) => {
                            changed |= family[func].insert(*dest);
                        }
                        Instruction::Phi { dest, sources }
                            if !sources.is_empty()
                                && sources
                                    .iter()
                                    .all(|source| family[func].contains(&source.value)) =>
                        {
                            changed |= family[func].insert(*dest);
                        }
                        Instruction::CreateDataProperty { dest, object, .. }
                            if family[func].contains(object) =>
                        {
                            changed |= family[func].insert(*dest);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    FamilySeed { family, bindings }
}

/// 函数内常量字符串定义表（键解析用）。
pub(crate) fn collect_const_strings(
    function: &Function,
    constants: &[Constant],
) -> HashMap<ValueId, String> {
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

/// 家族值出现在指令里的裁决。
enum UseVerdict {
    /// 使用安全，无属性写。
    Ok,
    /// 家族属性写；`alias` = 非「分配函数内经 alloc dest 直达」的写。
    Write { alias: bool },
    /// 逃逸或不可分析的使用，整个 record 不合格。
    Reject,
}

/// 打印/纯谓词类 builtin：宿主实现只读渲染（`render_value` 无 ctx 不可能
/// 回调用户代码），且不保留对象引用。
fn builtin_reads_only(builtin: Builtin) -> bool {
    matches!(
        builtin,
        Builtin::ConsoleLog
            | Builtin::ConsoleInfo
            | Builtin::ConsoleDebug
            | Builtin::ConsoleWarn
            | Builtin::ConsoleError
            | Builtin::ConsoleTrace
            | Builtin::IsJsObject
    )
}

fn qualify_record(
    module: &Module,
    alloc_func: usize,
    alloc_dest: ValueId,
    keys: Vec<String>,
) -> Option<RecordFacts> {
    let seed = seed_family(module, alloc_func, alloc_dest);
    if seed
        .bindings
        .iter()
        .any(|name| is_protocol_or_env_name(name))
    {
        return None;
    }
    let constants = module.constants();
    let mut write_blocks: Vec<HashSet<BasicBlockId>> =
        module.functions().iter().map(|_| HashSet::new()).collect();
    let mut has_alias_writes = false;
    for (func, function) in module.functions().iter().enumerate() {
        let fam = &seed.family[func];
        let strings = collect_const_strings(function, constants);
        for block in function.blocks() {
            // 家族值不得流入终止器（Return/Throw/Branch/Switch）。
            if terminator_uses(block.terminator())
                .into_iter()
                .any(|value| fam.contains(&value))
            {
                return None;
            }
            for instruction in block.instructions() {
                // 绑定冲突：绑定被写入非家族值，LoadVar 结果不再必然是本对象。
                if let Instruction::StoreVar { name, value } = instruction
                    && seed.bindings.contains(name)
                    && !fam.contains(value)
                {
                    return None;
                }
                match record_use_verdict(instruction, fam, &strings, alloc_dest, func, alloc_func) {
                    UseVerdict::Ok => {}
                    UseVerdict::Write { alias } => {
                        has_alias_writes |= alias;
                        write_blocks[func].insert(block.id());
                    }
                    UseVerdict::Reject => return None,
                }
            }
        }
    }
    // 存在经绑定别名的属性写：循环体内的任意调用/协变求值都可能触发该写，
    // 无法证明循环期间值不变，整个 record 放弃（v1 只接受纯初始化写）。
    if has_alias_writes {
        return None;
    }
    Some(RecordFacts {
        keys,
        family: seed.family,
        write_blocks,
    })
}

fn record_use_verdict(
    instruction: &Instruction,
    fam: &HashSet<ValueId>,
    strings: &HashMap<ValueId, String>,
    alloc_dest: ValueId,
    func: usize,
    alloc_func: usize,
) -> UseVerdict {
    let mut uses = instr_uses(instruction);
    if let Instruction::Phi { sources, .. } = instruction {
        uses.extend(sources.iter().map(|source| source.value));
    }
    if !uses.iter().any(|value| fam.contains(value)) {
        return UseVerdict::Ok;
    }
    match instruction {
        // 家族传播已接受的形状。
        Instruction::StoreVar { value, .. } if fam.contains(value) => UseVerdict::Ok,
        Instruction::Phi { dest, sources }
            if fam.contains(dest) && sources.iter().all(|source| fam.contains(&source.value)) =>
        {
            UseVerdict::Ok
        }
        // 读取：对象只有自有数据属性，读不产生用户代码也不泄漏引用。
        // 键本身是家族值则会流入 ToPropertyKey（this=对象的 toString）→ 拒绝。
        Instruction::GetProp { object, key, .. }
        | Instruction::OptionalGetProp { object, key, .. }
            if fam.contains(object) && !fam.contains(key) =>
        {
            UseVerdict::Ok
        }
        Instruction::GetElem { object, index, .. }
        | Instruction::OptionalGetElem {
            object, key: index, ..
        } if fam.contains(object) && !fam.contains(index) => UseVerdict::Ok,
        // 常量键数据写：保持数据属性，不可能引入 accessor。
        Instruction::SetProp {
            object, key, value, ..
        }
        | Instruction::CreateDataProperty {
            object, key, value, ..
        } if fam.contains(object) && !fam.contains(value) && !fam.contains(key) => {
            match strings.get(key) {
                Some(text) if text != "__proto__" => UseVerdict::Write {
                    alias: func != alloc_func || *object != alloc_dest,
                },
                _ => UseVerdict::Reject,
            }
        }
        // 改写自己的原型：自有数据属性读取不受影响。
        Instruction::SetProto { object, value } if fam.contains(object) && !fam.contains(value) => {
            UseVerdict::Ok
        }
        // 纯谓词。
        Instruction::IsException { value, .. } if fam.contains(value) => UseVerdict::Ok,
        // 只读渲染 builtin（console 系列 / IsJsObject）：格式化后即弃引用。
        Instruction::CallBuiltin { builtin, .. } if builtin_reads_only(*builtin) => UseVerdict::Ok,
        _ => UseVerdict::Reject,
    }
}

// ── T1 纯函数（可证明终止、无任何可观察副作用/异常/分配）────────────────

/// callee 是否可整体外提：CFG 无环（终止性），指令全部在纯白名单内，
/// 终止器无 Throw。Binary / 算术 Unary 需值类证明为 Number（排除
/// ToPrimitive 触发用户代码与 BigInt 混用抛错）。
fn function_is_terminating_pure(function: &Function, classes: &ValueClassSet) -> bool {
    if function_has_cycle(function) {
        return false;
    }
    for block in function.blocks() {
        if matches!(block.terminator(), Terminator::Throw { .. }) {
            return false;
        }
        for instruction in block.instructions() {
            let allowed = match instruction {
                Instruction::Const { .. }
                | Instruction::Phi { .. }
                | Instruction::Compare { .. }
                | Instruction::IsException { .. }
                | Instruction::GuardSameFunction { .. }
                | Instruction::DebugCheck { .. } => true,
                Instruction::Unary {
                    op: UnaryOp::Not | UnaryOp::Void | UnaryOp::IsNullish,
                    ..
                } => true,
                Instruction::Unary {
                    op: UnaryOp::Neg | UnaryOp::Pos,
                    dest,
                    ..
                } => classes.numbers.contains(dest),
                Instruction::Binary { dest, .. } => classes.numbers.contains(dest),
                Instruction::LoadVar { name, .. } => {
                    function.params().iter().any(|param| param == name)
                }
                _ => false,
            };
            if !allowed {
                return false;
            }
        }
    }
    true
}

/// CFG 是否含环（迭代三色 DFS，避免深链递归栈溢出）。
fn function_has_cycle(function: &Function) -> bool {
    let count = function.blocks().len();
    // 0 = 未访问，1 = 在栈上，2 = 完成。
    let mut state = vec![0u8; count];
    for start in 0..count {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;
        while let Some((block, cursor)) = stack.pop() {
            let successors =
                wjsm_ir::cfg::terminator_successors(function.blocks()[block].terminator());
            if cursor < successors.len() {
                stack.push((block, cursor + 1));
                let next = successors[cursor].0 as usize;
                if next >= count {
                    continue;
                }
                match state[next] {
                    0 => {
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                    1 => return true,
                    _ => {}
                }
            } else {
                state[block] = 2;
            }
        }
    }
    false
}
