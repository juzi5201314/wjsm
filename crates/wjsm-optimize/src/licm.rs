//! licm pass：循环不变量外提与 Shape 检查外提。
//!
//! 在 semantic IR 的自然循环上做三类外提，把被移动的指令搬进新建的
//! pre-header（循环头的全部外部前驱重定向到它，回边保持原状）：
//!
//! 1. **循环不变 `LoadVar`**：绑定的全部 StoreVar 都在当前函数、支配循环头
//!    且循环体不写；循环体含调用时要求当前函数是模块入口（不可重入），
//!    否则要求循环体不可能执行用户代码。
//! 2. **Shape 检查外提（常量键 `GetProp`）**：receiver 是 [`RecordFacts`]
//!    证明的稳定 record（纯数据属性、无 getter、引用不逃逸、循环期间不可能
//!    被写），键是模板自有键。整条 GetProp——连同后端为它发射的 Inline
//!    Cache（shape 检查 + 原型链 generation 验证）——移到 pre-header 只执行
//!    一次，循环体内直接复用寄存器值，不再有逐迭代的 shape/proto 检查。
//! 3. **纯直接调用**：callee 可静态解析且 T1 纯（CFG 无环 ⇒ 终止、无状态
//!    读写、无异常、无分配），实参全部循环不变。Cranelift egraph LICM 把
//!    call 硬编码为有副作用永远不提升，此处在 IR 层完成。
//! 4. **elem-guard 外提（`POINTS[i].x` 型逐迭代 receiver）**：pre-header 插
//!    一条带模板的 `GuardElementsKind` 运行期一次性校验数组与元素 shape，循环体内成对
//!    的 `GetElem`/`GetProp` 带上共享闩锁——守卫为真时属性读取跳过
//!    逐迭代 shape 检查、单指令直读模板槽。见 [`super::licm_elem_guard`]。
//!
//! 安全性基线：被外提指令必须「无可观察副作用、不抛异常、必然终止」，因此
//! 即使循环 0 次进入，pre-header 多执行一次也不可观察。读取类候选还要求
//! 「循环体内不存在能改写其结果的写」，保证 pre-header 的值与每次迭代读到
//! 的值逐位一致。
//!
//! `WJSM_DISABLE_LICM` 非空且非 0/false/off 时整体跳过（bench 保持真实性能，
//! 见 wjsm-bench runner）。

use std::collections::{HashMap, HashSet};

use wjsm_ir::cfg::ControlFlowGraph;
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, FunctionId, Instruction, Module, UnaryOp, ValueId,
    is_builtin_entry_ir_function, is_module_entry_ir_function,
};

use super::inline_for_ea::max_value_id_in_function;
use super::licm_apply::{Plan, apply_plan};
use super::licm_facts::{ModuleFacts, collect_const_strings, is_protocol_or_env_name};
use crate::ir_walk::instruction_dest;

/// 单函数外提轮数上限（每轮最多变换一个循环；正常在候选耗尽时提前收敛）。
const MAX_ROUNDS: usize = 64;

/// `WJSM_DISABLE_LICM` 是否生效：除空值与显式 0/false/off 外均视为禁用。
/// 该开关改变 lower 产物，输入寻址 artifact 缓存的键必须包含它
/// （经 [`crate::licm_disabled_by_env`] 暴露给缓存层）。
pub fn licm_disabled_by_env() -> bool {
    !matches!(
        std::env::var("WJSM_DISABLE_LICM").as_deref(),
        Err(_) | Ok("") | Ok("0") | Ok("false") | Ok("off")
    )
}

pub(crate) fn run(module: &mut Module) {
    if licm_disabled_by_env() {
        return;
    }
    // eval 可动态改写绑定与作用域，全部前提失效，保守禁用。
    if module.functions().iter().any(Function::has_eval) {
        return;
    }
    let facts = ModuleFacts::build(module);
    for func_idx in 0..module.functions().len() {
        hoist_function(module, func_idx, &facts);
    }
}

fn hoist_function(module: &mut Module, func_idx: usize, facts: &ModuleFacts) {
    let mut next_value = max_value_id_in_function(&module.functions()[func_idx]) + 1;
    for _ in 0..MAX_ROUNDS {
        let Some(plan) = plan_one_loop(module, func_idx, facts) else {
            break;
        };
        let function = module
            .function_mut(FunctionId(func_idx as u32))
            .expect("function index must be valid");
        apply_plan(function, &plan, &mut next_value);
    }
}

// ── 自然循环发现 ────────────────────────────────────────────────────────

struct NaturalLoop {
    header: BasicBlockId,
    /// 循环体（含 header 与全部 latch）。
    body: HashSet<BasicBlockId>,
}

/// 回边 u→h（h 支配 u）按 header 合并出自然循环，body 小的在前（先内层）。
fn natural_loops(function: &Function, cfg: &ControlFlowGraph) -> Vec<NaturalLoop> {
    let dom_sets = cfg.dominator_sets();
    let mut by_header: HashMap<BasicBlockId, HashSet<BasicBlockId>> = HashMap::new();
    for block in function.blocks() {
        let source = block.id();
        if !cfg.is_reachable(source) {
            continue;
        }
        for target in cfg.successors(source) {
            let is_back_edge = dom_sets
                .get(&source)
                .is_some_and(|dominators| dominators.contains(target));
            if !is_back_edge || *target == function.entry() {
                continue;
            }
            let body = by_header.entry(*target).or_insert_with(|| {
                let mut set = HashSet::new();
                set.insert(*target);
                set
            });
            // 自然循环体：从 latch 沿前驱回溯，不越过 header。
            let mut stack = vec![source];
            while let Some(current) = stack.pop() {
                if !body.insert(current) {
                    continue;
                }
                stack.extend(cfg.predecessors(current).iter().copied());
            }
        }
    }
    let mut loops: Vec<NaturalLoop> = by_header
        .into_iter()
        .map(|(header, body)| NaturalLoop { header, body })
        .collect();
    loops.sort_by_key(|natural| (natural.body.len(), natural.header.0));
    loops
}

// ── 候选收集 ────────────────────────────────────────────────────────────

/// 循环级上下文（对单个循环的只读快照分析；elem-guard 子阶段共用）。
pub(crate) struct LoopView<'a> {
    pub(crate) function: &'a Function,
    pub(crate) func_idx: usize,
    pub(crate) body: &'a HashSet<BasicBlockId>,
    /// header 的支配者集合（LoadVar 的 store 站点支配性检查用）。
    header_dominators: &'a HashSet<BasicBlockId>,
    /// ValueId → 定义位置（块 id + 下标）。
    pub(crate) defs: &'a HashMap<ValueId, (BasicBlockId, usize)>,
    /// 函数内常量字符串定义。
    pub(crate) strings: &'a HashMap<ValueId, String>,
    /// 循环体内被 StoreVar 的名字。
    store_names: HashSet<String>,
    /// 循环体内含 Suspend / GeneratorSuspend。
    pub(crate) has_suspend: bool,
    /// 当前函数是模块/builtin 入口（只执行一次、不可被调用 ⇒ 无重入）。
    is_entry: bool,
    /// 循环体内不可能执行任何用户代码（见 [`loop_never_runs_user_code`]）。
    call_free: bool,
}

fn plan_one_loop(module: &Module, func_idx: usize, facts: &ModuleFacts) -> Option<Plan> {
    let function = &module.functions()[func_idx];
    let cfg = ControlFlowGraph::build(function);
    let dom_sets = cfg.dominator_sets();
    let defs = build_defs(function);
    let strings = collect_const_strings(function, module.constants());
    for natural in natural_loops(function, &cfg) {
        let header_dominators = dom_sets.get(&natural.header)?;
        let view = build_loop_view(
            module,
            func_idx,
            &natural.body,
            header_dominators,
            &defs,
            &strings,
            facts,
        );
        let moves = collect_moves(module, &view, facts);
        // elem-guard 依赖往轮已把循环不变 LoadVar（数组 receiver）搬进
        // pre-header，因此只在常规候选耗尽后规划，一轮一类。
        let elem_guards = if moves.is_empty() {
            super::licm_elem_guard::plan_elem_guards(module, &view, facts)
        } else {
            Vec::new()
        };
        if !moves.is_empty() || !elem_guards.is_empty() {
            return Some(Plan {
                header: natural.header,
                body: natural.body,
                moves,
                elem_guards,
            });
        }
    }
    None
}

fn build_defs(function: &Function) -> HashMap<ValueId, (BasicBlockId, usize)> {
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for (index, instruction) in block.instructions().iter().enumerate() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, (block.id(), index));
            }
        }
    }
    defs
}

#[allow(clippy::too_many_arguments)]
fn build_loop_view<'a>(
    module: &'a Module,
    func_idx: usize,
    body: &'a HashSet<BasicBlockId>,
    header_dominators: &'a HashSet<BasicBlockId>,
    defs: &'a HashMap<ValueId, (BasicBlockId, usize)>,
    strings: &'a HashMap<ValueId, String>,
    facts: &ModuleFacts,
) -> LoopView<'a> {
    let function = &module.functions()[func_idx];
    let mut store_names = HashSet::new();
    let mut has_suspend = false;
    for block_id in body {
        let Some(block) = function.block_by_id(*block_id) else {
            continue;
        };
        for instruction in block.instructions() {
            match instruction {
                Instruction::StoreVar { name, .. } => {
                    store_names.insert(name.clone());
                }
                Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. } => {
                    has_suspend = true;
                }
                _ => {}
            }
        }
    }
    let is_entry = is_module_entry_ir_function(function.name())
        || is_builtin_entry_ir_function(function.name());
    let call_free = loop_never_runs_user_code(function, func_idx, body, facts);
    LoopView {
        function,
        func_idx,
        body,
        header_dominators,
        defs,
        strings,
        store_names,
        has_suspend,
        is_entry,
        call_free,
    }
}

/// 循环体内是否不可能执行任何用户 JS 代码：无调用/构造/suspend，builtin 仅
/// 只读渲染类，协变运算（Binary / 算术 Unary）须有 Number 值类证明（排除
/// ToPrimitive 回调），属性/元素读取仅允许稳定 record 家族（无 getter）。
fn loop_never_runs_user_code(
    function: &Function,
    func_idx: usize,
    body: &HashSet<BasicBlockId>,
    facts: &ModuleFacts,
) -> bool {
    let numbers = facts.numbers.get(&(func_idx as u32));
    let in_family = |value: &ValueId| {
        facts
            .records
            .iter()
            .any(|record| record.family[func_idx].contains(value))
    };
    for block_id in body {
        let Some(block) = function.block_by_id(*block_id) else {
            continue;
        };
        for instruction in block.instructions() {
            let safe = match instruction {
                Instruction::Const { .. }
                | Instruction::Phi { .. }
                | Instruction::Compare { .. }
                | Instruction::LoadVar { .. }
                | Instruction::StoreVar { .. }
                | Instruction::IsException { .. }
                | Instruction::GuardSameFunction { .. }
                | Instruction::GuardSamePrototypeAccessor { .. }
                | Instruction::DebugCheck { .. } => true,
                Instruction::Unary {
                    op: UnaryOp::Not | UnaryOp::Void | UnaryOp::IsNullish,
                    ..
                } => true,
                Instruction::Unary {
                    op: UnaryOp::Neg | UnaryOp::Pos,
                    dest,
                    ..
                }
                | Instruction::Binary { dest, .. } => {
                    numbers.is_some_and(|classes| classes.numbers.contains(dest))
                }
                Instruction::GetProp { object, .. } | Instruction::GetElem { object, .. } => {
                    in_family(object)
                }
                Instruction::CallBuiltin { builtin, .. } => matches!(
                    builtin,
                    Builtin::ConsoleLog
                        | Builtin::ConsoleInfo
                        | Builtin::ConsoleDebug
                        | Builtin::ConsoleWarn
                        | Builtin::ConsoleError
                        | Builtin::ConsoleTrace
                        | Builtin::IsJsObject
                        | Builtin::ToBoolean
                ),
                _ => false,
            };
            if !safe {
                return false;
            }
        }
    }
    true
}

/// 收集本循环的全部可外提指令位置（候选 + 循环内可移动依赖，去重排序）。
fn collect_moves(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
) -> Vec<(BasicBlockId, usize)> {
    let mut moves: HashSet<(BasicBlockId, usize)> = HashSet::new();
    for block_id in view.body {
        let Some(block) = view.function.block_by_id(*block_id) else {
            continue;
        };
        for (index, instruction) in block.instructions().iter().enumerate() {
            let candidate = match instruction {
                Instruction::LoadVar { name, .. } => {
                    loadvar_hoistable(view, facts, name).then(|| vec![(*block_id, index)])
                }
                Instruction::GetProp { object, key, .. } => {
                    getprop_moves(module, view, facts, *object, *key).map(|mut deps| {
                        deps.push((*block_id, index));
                        deps
                    })
                }
                Instruction::Call {
                    callee,
                    this_val,
                    args,
                    ..
                } => pure_call_moves(module, view, facts, *callee, *this_val, args).map(
                    |mut deps| {
                        deps.push((*block_id, index));
                        deps
                    },
                ),
                _ => None,
            };
            if let Some(positions) = candidate {
                moves.extend(positions);
            }
        }
    }
    let mut ordered: Vec<(BasicBlockId, usize)> = moves.into_iter().collect();
    ordered.sort_unstable_by_key(|(block, index)| (block.0, *index));
    ordered
}

/// `LoadVar name` 是否可外提出本循环。
///
/// 条件：非协议/环境槽；循环体不写该名、无 suspend；全部 StoreVar 站点都在
/// 当前函数、不在循环体内且支配循环头（⇒ pre-header 处已初始化，读不到
/// TDZ/未定值，且值与每次迭代一致）；无可见 store 时仅接受当前函数形参。
/// 重入防护：当前函数是入口（不可被调用）或循环体不可能执行用户代码——
/// 否则循环内的调用可能重入当前函数、重放 StoreVar 改写共享槽。
fn loadvar_hoistable(view: &LoopView<'_>, facts: &ModuleFacts, name: &str) -> bool {
    if is_protocol_or_env_name(name)
        || view.store_names.contains(name)
        || view.has_suspend
        || !(view.is_entry || view.call_free)
    {
        return false;
    }
    match facts.stores.get(name) {
        None => view.function.params().iter().any(|param| param == name),
        Some(sites) => {
            !sites.is_empty()
                && sites.iter().all(|site| {
                    site.func == view.func_idx
                        && !view.body.contains(&site.block)
                        && view.header_dominators.contains(&site.block)
                })
        }
    }
}

/// 操作数的循环不变性：定义在循环外 → 无需移动；定义在循环内 → 仅当
/// def 是 Const 或可外提 LoadVar 时随候选一起移动。
enum Operand {
    Invariant,
    Move(BasicBlockId, usize),
}

fn resolve_operand(view: &LoopView<'_>, facts: &ModuleFacts, value: ValueId) -> Option<Operand> {
    let Some((block, index)) = view.defs.get(&value).copied() else {
        return None;
    };
    if !view.body.contains(&block) {
        return Some(Operand::Invariant);
    }
    let instruction = view
        .function
        .block_by_id(block)?
        .instructions()
        .get(index)?;
    match instruction {
        Instruction::Const { .. } => Some(Operand::Move(block, index)),
        Instruction::LoadVar { name, .. } if loadvar_hoistable(view, facts, name) => {
            Some(Operand::Move(block, index))
        }
        _ => None,
    }
}

/// Shape 检查外提候选：常量键 GetProp，receiver 是稳定 record 家族值，
/// 键是模板自有键，且本循环体内无该 record 的属性写。
fn getprop_moves(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    object: ValueId,
    key: ValueId,
) -> Option<Vec<(BasicBlockId, usize)>> {
    let key_text = view.strings.get(&key)?;
    let mut deps = Vec::new();
    match resolve_operand(view, facts, key)? {
        Operand::Invariant => {}
        Operand::Move(block, index) => deps.push((block, index)),
    }
    match resolve_operand(view, facts, object)? {
        Operand::Invariant => {}
        Operand::Move(block, index) => {
            // 循环内定义的 receiver 只能是可外提 LoadVar（Const 不可能是对象）。
            let instruction = view
                .function
                .block_by_id(block)?
                .instructions()
                .get(index)?;
            if !matches!(instruction, Instruction::LoadVar { .. }) {
                return None;
            }
            deps.push((block, index));
        }
    }
    let record = facts.records.iter().find(|record| {
        record.family[view.func_idx].contains(&object)
            && record.keys.iter().any(|candidate| candidate == key_text)
    })?;
    // 循环体内存在该 record 的属性写 → 值可能逐迭代变化，放弃。
    if record.write_blocks[view.func_idx]
        .iter()
        .any(|block| view.body.contains(block))
    {
        return None;
    }
    let _ = module;
    Some(deps)
}

/// 纯直接调用候选：callee 静态可解析且 T1 纯，callee/this/实参全部循环不变
/// 或可随之移动。
fn pure_call_moves(
    module: &Module,
    view: &LoopView<'_>,
    facts: &ModuleFacts,
    callee: ValueId,
    this_val: ValueId,
    args: &[ValueId],
) -> Option<Vec<(BasicBlockId, usize)>> {
    let (callee_block, callee_index) = view.defs.get(&callee).copied()?;
    let callee_def = view
        .function
        .block_by_id(callee_block)?
        .instructions()
        .get(callee_index)?;
    let target = match callee_def {
        Instruction::Const { constant, .. } => match module.constants().get(constant.0 as usize) {
            Some(Constant::FunctionRef(id)) => *id,
            _ => return None,
        },
        Instruction::LoadVar { name, .. } => *view.function.known_callee_vars().get(name)?,
        _ => return None,
    };
    if !facts
        .pure_callees
        .get(target.0 as usize)
        .copied()
        .unwrap_or(false)
    {
        return None;
    }
    let mut deps = Vec::new();
    for value in std::iter::once(callee)
        .chain(std::iter::once(this_val))
        .chain(args.iter().copied())
    {
        match resolve_operand(view, facts, value)? {
            Operand::Invariant => {}
            Operand::Move(block, index) => deps.push((block, index)),
        }
    }
    Some(deps)
}
