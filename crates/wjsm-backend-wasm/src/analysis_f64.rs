//! f64 值类型传播分析（编译期、跨函数）。
//!
//! 目标：识别**规范上必为 plain f64**（未 NaN-boxed 的 double 位模式）的 ValueId 与
//! 函数形参，供后端生成无检查的 f64 运算/比较——消除 spill（f64 不持 GC handle）、
//! 跳过 is_f64 类型检查、跳过 host 分派。
//!
//! 产出三个表（存到 `Compiler`，`compile_module` Pass 0 一次性计算）：
//! - `known_f64`：函数内规范必为 f64 的 ValueId。
//! - `known_bool`：比较指令（`AbstractCompare`/`StrictEq`/`StrictNotEq`）的 dest，
//!   规范必为 boxed bool（低 32 位仅 payload bit，无 GC handle），供 ToBoolean 简化
//!   与 spill 过滤。
//! - `param_is_f64`：direct_callable 函数每个声明形参（排除 `$env`/`$this`）是否
//!   必为 f64。由模块内全部调用点贡献的 AND 决定。
//!
//! ## 函数内传播规则（只收窄不扩大，任一不确定 → false）
//! - `Const(Number)` → 必为 f64（种子）。
//! - `Binary`（`Add` 除外——有字符串拼接语义）：lhs ∧ rhs 均 f64 → f64。
//! - `Unary`（`Neg`/`Pos`/`BitNot`）：操作数 f64 → f64（`-5n`/`~5n` 为 BigInt 结果，
//!   故必须排除非 f64 操作数；`+x` 对 BigInt/Symbol 抛异常，保守同样要求操作数 f64）。
//! - `Phi`：所有入边源均 f64 → f64。
//! - `LoadVar`：变量的所有 `StoreVar` 源均 f64 → f64；形参名无 StoreVar 时由
//!   `param_is_f64` 决定；捕获变量（本函数无 StoreVar 且非形参）→ 保守 false。
//!   形参若被 StoreVar 重赋值，要求形参 f64 **且** 全部源 f64。
//!
//! ## 跨函数传播（乐观起点 + 单调收紧至最大不动点）
//! 调用点 callee 解析（沿 def 链）：
//! - `Const(FunctionRef(fn))` → 精确 {fn}；
//! - `LoadVar(name ∈ known_callee_vars)` → 精确 {fn}；
//! - `Phi` → 所有源解析为**同一**函数才精确，否则 Unknown；
//! - 其余（GetProp/GetElem/动态值等）→ Unknown（可能是任意模块函数）。
//!
//! `param_is_f64[g][i]` = (存在指向 g 的调用点) ∧ ∀ 调用点 C（g ∈ possible_callees(C)）:
//!   `args.len() > i` 且 `arg_i ∈ known_f64[caller]`。
//! Unknown callee 视为「可能指向任意模块函数」——其参数对**所有** direct_callable
//! 函数的形参都必须是 f64，否则对应形参保守 false（sound：Unknown 调用点若真的
//! 调用了 g 且传非 f64，收紧即避免误优化）。
//!
//! 固定点从「全部形参 true」出发逐轮收紧：每轮用当前形参假设重算 known_f64，
//! 再用调用点 AND 修正形参；形参只从 true 翻 false，必然终止（最大不动点语义）。
//!
//! ## 已知边界（与优化计划一致）
//! 函数值逃逸到 host callback（如 `arr.forEach(fib)`）的调用不产生 IR `Call`
//! 指令，本分析不建模——该场景由「调用点参数检查」兜底：此类回调元素若非 f64，
//! 分析无从得知，形参可能被误标。semantic 的 direct_call pass 已把函数声明绑定
//! 替换为 `Const(FunctionRef)`，逃逸面很小；若未来要收紧，可加函数值逃逸扫描。
//!
//! 非 direct_callable 函数（module entry、eval）不参与形参分析（无 param_is_f64
//! 条目），但其函数内 known_f64/known_bool 照常计算（作为调用方约束其他函数）。
use std::collections::{HashMap, HashSet};
use wjsm_ir::{BinaryOp, Builtin, Constant, FunctionId, Instruction, Module, Terminator, UnaryOp, ValueId};

/// f64 值类型传播分析结果。
#[derive(Debug, Clone, Default)]
pub struct F64Analysis {
    /// 函数内规范必为 f64 的 ValueId。索引 = FunctionId.0。
    known_f64: Vec<HashSet<ValueId>>,
    /// 函数内规范必为 boxed bool 的 ValueId。索引 = FunctionId.0。
    known_bool: Vec<HashSet<ValueId>>,
    /// direct_callable 函数声明形参（排除 $env/$this）是否必为 f64。
    /// 索引 = FunctionId.0 → Vec<bool>（顺序 = 声明形参顺序）。
    param_is_f64: Vec<Vec<bool>>,
    /// 函数是否可能以异常状态返回（含 Terminator::Throw 与可能抛异常的调用）。
    /// 不动点：false→true 单调。消费方：调用结果标 f64（需 !can_throw(callee)）、
    /// is_exception 省略（需 !can_throw(callee)）。索引 = FunctionId.0。
    can_throw: Vec<bool>,
    /// 函数所有 return 值（含 None=undefined）是否必为 f64。乐观起点 true 单调收紧。
    /// 仅对 direct_callable 函数有效（非 direct_callable → false）。索引 = FunctionId.0。
    returns_f64: Vec<bool>,
    /// 调用结果必非异常（callee 已知且 !can_throw）的 ValueId。索引 = FunctionId.0。
    never_exception: Vec<HashSet<ValueId>>,
    /// IsException dest 且操作数 ∈ never_exception → 恒为 false。索引 = FunctionId.0。
    constant_false: Vec<HashSet<ValueId>>,
    /// 死异常块（constant_false 分支的异常目标块索引）。索引 = FunctionId.0。
    dead_exception_blocks: Vec<HashSet<usize>>,
    /// known_bool 且所有消费均为 branch 条件（truthiness-only）的 ValueId：
    /// 比较可直出 i32（0/1）跳过 boxed 构造与解包。索引 = FunctionId.0。
    truthiness_only: Vec<HashSet<ValueId>>,
}

impl F64Analysis {
    /// 查询某 ValueId 在该函数内是否规范必为 f64。
    pub fn value_known_f64(&self, func_id: FunctionId, value_id: ValueId) -> bool {
        self.known_f64
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&value_id))
    }

    /// 查询某 ValueId 在该函数内是否规范必为 boxed bool。
    pub fn value_known_bool(&self, func_id: FunctionId, value_id: ValueId) -> bool {
        self.known_bool
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&value_id))
    }

    /// 查询函数声明形参 i 是否必为 f64（非 direct_callable 或越界 → false）。
    pub fn param_is_f64(&self, func_id: FunctionId, param_idx: usize) -> bool {
        self.param_is_f64
            .get(func_id.0 as usize)
            .and_then(|v| v.get(param_idx))
            .copied()
            .unwrap_or(false)
    }

    /// 查询函数是否可能以异常状态返回（含显式 throw 与可能抛异常的调用）。
    pub fn function_can_throw(&self, func_id: FunctionId) -> bool {
        self.can_throw
            .get(func_id.0 as usize)
            .copied()
            .unwrap_or(true)
    }

    /// 查询函数的所有 return 值是否必为 f64（非 direct_callable → false）。
    pub fn function_returns_f64(&self, func_id: FunctionId) -> bool {
        self.returns_f64
            .get(func_id.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    /// 查询某 ValueId 是否必非异常（来自 !can_throw(callee) 的精确调用结果）。
    pub fn value_never_exception(&self, func_id: FunctionId, value_id: ValueId) -> bool {
        self.never_exception
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&value_id))
    }

    /// 查询某 ValueId 是否编译期恒为 false（is_exception 且操作数必非异常）。
    pub fn condition_constant_false(&self, func_id: FunctionId, value_id: ValueId) -> bool {
        self.constant_false
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&value_id))
    }

    /// 查询某块是否死异常块（constant_false 分支的异常目标，不生成代码）。
    pub fn is_dead_exception_block(&self, func_id: FunctionId, block_idx: usize) -> bool {
        self.dead_exception_blocks
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&block_idx))
    }

    /// 查询某 ValueId 是否已知 boxed bool 且仅被 branch 条件消费
    /// （比较可直出 i32 0/1，跳过 boxed 构造与解包）。
    pub fn value_truthiness_only(&self, func_id: FunctionId, value_id: ValueId) -> bool {
        self.truthiness_only
            .get(func_id.0 as usize)
            .is_some_and(|s| s.contains(&value_id))
    }

    /// 供单测/调试：完整形参表。
    pub fn param_is_f64_vec(&self, func_id: FunctionId) -> Option<&[bool]> {
        self.param_is_f64
            .get(func_id.0 as usize)
            .map(|v| v.as_slice())
    }

    /// 执行 f64 值类型传播分析。
    pub fn analyze(module: &Module) -> Self {
        analyze_inner(module)
    }
}

/// 是否为 env 变量 IR 名（`$env` 或 `${scope}.$env`）。
fn is_env_name(name: &str) -> bool {
    name == "$env" || name.ends_with(".$env")
}

/// 是否为 this 变量 IR 名。
fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

/// 声明形参（排除 $env/$this）——与 compile_js_function 的过滤规则保持一致。
fn declared_params(function: &wjsm_ir::Function) -> Vec<String> {
    function
        .params()
        .iter()
        .filter(|p| is_declared_param(p))
        .cloned()
        .collect()
}

/// 是否为声明形参（排除 $env/$this）。
fn is_declared_param(p: &str) -> bool {
    p != "$env" && p != "$this" && !is_env_name(p) && !is_this_name(p)
}

/// 取 producing 指令的 dest。非 producing 返回 None。
fn instruction_dest(ins: &Instruction) -> Option<ValueId> {
    use Instruction::*;
    Some(match ins {
        Const { dest, .. }
        | Binary { dest, .. }
        | Unary { dest, .. }
        | Compare { dest, .. }
        | Phi { dest, .. }
        | StringConcatVa { dest, .. }
        | LoadVar { dest, .. }
        | NewObject { dest, .. }
        | GetProp { dest, .. }
        | DeleteProp { dest, .. }
        | NewArray { dest, .. }
        | GetElem { dest, .. }
        | OptionalGetProp { dest, .. }
        | OptionalGetElem { dest, .. }
        | OptionalCall { dest, .. }
        | ObjectSpread { dest, .. }
        | GetSuperBase { dest }
        | GetSuperConstructor { dest }
        | NewPromise { dest }
        | CollectRestArgs { dest, .. }
        | IsException { dest, .. }
        | EncodeException { dest, .. }
        | ExceptionToObject { dest, .. } => *dest,
        Call { dest, .. }
        | CallBuiltin { dest, .. }
        | SuperCall { dest, .. }
        | ConstructCall { dest, .. } => (*dest)?,
        StoreVar { .. }
        | SetProp { .. }
        | SetProto { .. }
        | SetElem { .. }
        | PromiseResolve { .. }
        | PromiseReject { .. }
        | Suspend { .. }
        | GeneratorSuspend { .. }
        | DebugCheck { .. } => return None,
    })
}

/// 每函数的静态结构（供固定点迭代复用，避免重复扫描）。
struct FnInfo<'a> {
    /// 所属 IR 函数（block/指令扫描用）。
    function: &'a wjsm_ir::Function,
    /// 声明形参名 → 形参序号。
    param_index: HashMap<&'a str, usize>,
    /// 变量名 → 全部 StoreVar 源 ValueId。
    var_stores: HashMap<&'a str, Vec<ValueId>>,
    /// LoadVar 的 (dest, 变量名)。
    loads: Vec<(ValueId, &'a str)>,
    /// Phi 的 (dest, 源 ValueId 列表)。
    phis: Vec<(ValueId, Vec<ValueId>)>,
    /// Binary 的 (dest, op, lhs, rhs)。
    binaries: Vec<(ValueId, BinaryOp, ValueId, ValueId)>,
    /// Unary 的 (dest, op, 操作数)。
    unaries: Vec<(ValueId, UnaryOp, ValueId)>,
    /// Call 的 (dest, callee)（调用结果 f64 传播用；dest None 不收集）。
    calls: Vec<(ValueId, ValueId)>,
    /// ValueId → def 指令（callee 解析用）。
    defs: HashMap<ValueId, &'a Instruction>,
    /// known_callee_vars（函数声明绑定 → FunctionId）。
    known_callees: &'a HashMap<String, FunctionId>,
}

/// 模块级调用点：(caller FunctionId, callee ValueId, 实参 ValueId 列表)。
struct CallSite {
    caller: FunctionId,
    callee: ValueId,
    args: Vec<ValueId>,
}

/// 执行 f64 值类型传播分析（内部实现，公开入口为 `F64Analysis::analyze`）。
fn analyze_inner(module: &Module) -> F64Analysis {
    let num_functions = module.functions().len();
    if num_functions == 0 {
        return F64Analysis::default();
    }

    // ── 模块级变量 StoreVar 计数（不可变绑定判定，供 GetProp callee 解析）──
    let mut var_store_count: HashMap<&str, u32> = HashMap::new();
    for function in module.functions() {
        for bb in function.blocks() {
            for ins in bb.instructions() {
                if let Instruction::StoreVar { name, .. } = ins {
                    *var_store_count.entry(name.as_str()).or_insert(0) += 1;
                }
            }
        }
    }

    let infos: Vec<FnInfo> = module
        .functions()
        .iter()
        .map(|f| {
            let mut param_index = HashMap::new();
            let mut declared_idx = 0usize;
            for p in f.params() {
                if is_declared_param(p) {
                    param_index.insert(p.as_str(), declared_idx);
                    declared_idx += 1;
                }
            }
            let mut info = FnInfo {
                function: f,
                param_index,
                var_stores: HashMap::new(),
                loads: Vec::new(),
                phis: Vec::new(),
                binaries: Vec::new(),
                unaries: Vec::new(),
                calls: Vec::new(),
                defs: HashMap::new(),
                known_callees: f.known_callee_vars(),
            };
            for bb in f.blocks() {
                for ins in bb.instructions() {
                    match ins {
                        Instruction::StoreVar { name, value } => {
                            info.var_stores.entry(name.as_str()).or_default().push(*value);
                        }
                        Instruction::LoadVar { dest, name } => {
                            info.loads.push((*dest, name.as_str()));
                        }
                        Instruction::Phi { dest, sources } => {
                            info.phis
                                .push((*dest, sources.iter().map(|s| s.value).collect()));
                        }
                        Instruction::Binary { dest, op, lhs, rhs } => {
                            info.binaries.push((*dest, *op, *lhs, *rhs));
                        }
                        Instruction::Unary { dest, op, value } => {
                            info.unaries.push((*dest, *op, *value));
                        }
                        Instruction::Call { dest: Some(dest), callee, .. } => {
                            info.calls.push((*dest, *callee));
                        }
                        _ => {}
                    }
                    if let Some(dest) = instruction_dest(ins) {
                        info.defs.insert(dest, ins);
                    }
                }
            }
            info
        })
        .collect();

    // ── known_bool：单遍确定性分类 ──
    let mut known_bool: Vec<HashSet<ValueId>> = vec![HashSet::new(); num_functions];
    for (func_idx, function) in module.functions().iter().enumerate() {
        let set = &mut known_bool[func_idx];
        for bb in function.blocks() {
            for ins in bb.instructions() {
                match ins {
                    // AbstractCompare/StrictEq/StrictNotEq 的 dest 规范必为 boxed bool。
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare | Builtin::StrictEq,
                        ..
                    }
                    | Instruction::Compare {
                        dest,
                        op: wjsm_ir::CompareOp::StrictEq | wjsm_ir::CompareOp::StrictNotEq,
                        ..
                    } => {
                        set.insert(*dest);
                    }
                    _ => {}
                }
            }
        }
    }

    // ── 收集全部调用点（Call/ConstructCall/非 forward_args 的 SuperCall/OptionalCall）──
    let mut call_sites: Vec<CallSite> = Vec::new();
    for (func_idx, function) in module.functions().iter().enumerate() {
        let caller = FunctionId(func_idx as u32);
        for bb in function.blocks() {
            for ins in bb.instructions() {
                match ins {
                    Instruction::Call { callee, args, .. }
                    | Instruction::ConstructCall { callee, args, .. }
                    | Instruction::OptionalCall { callee, args, .. } => {
                        call_sites.push(CallSite {
                            caller,
                            callee: *callee,
                            args: args.clone(),
                        });
                    }
                    // forward_args 的 super(...)：实参在 shadow stack，静态未知 → 不贡献。
                    Instruction::SuperCall {
                        callee,
                        args,
                        forward_args: false,
                        ..
                    } => {
                        call_sites.push(CallSite {
                            caller,
                            callee: *callee,
                            args: args.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    // ── 跨函数固定点（乐观起点 true，单调收紧）──
    // direct_callable 函数才有形参表；其余保持空 Vec。
    // ── 逃逸分析：每个函数值是否"动态可达"（可能被 Unknown 调用点调用）──
    // Unknown callee 调用点只约束动态可达的函数（见 compute_param_is_f64）。
    let dynamically_reachable = compute_dynamically_reachable(module, &infos, &var_store_count);

    let mut param_is_f64: Vec<Vec<bool>> = module
        .functions()
        .iter()
        .map(|f| {
            let n = declared_params(f).len();
            if f.direct_callable() {
                vec![true; n]
            } else {
                Vec::new()
            }
        })
        .collect();
    // can_throw：乐观 false 起点，false→true 单调传播（仅可能抛异常才翻 true）。
    let mut can_throw = vec![false; num_functions];
    // returns_f64：乐观 true 起点（仅 direct_callable 参与；非 direct_callable 无返回
    // 值契约，恒 false）。随 known_f64 收缩单调收紧。
    let mut returns_f64: Vec<bool> = module
        .functions()
        .iter()
        .map(|f| f.direct_callable())
        .collect();

    // 联合不动点：can_throw（死块感知，死块集单调增）∧ known_f64（单调缩）∧
    // returns_f64（单调缩）∧ param_is_f64（单调缩）∧ 派生表（never_exception/
    // constant_false/dead_blocks 单调增）。can_throw 依赖 known_f64（builtin f64 特例）
    // 与 dead_blocks（死路径不计）；派生表依赖 can_throw——同轮内按依赖顺序计算。
    // 收敛判据覆盖全部表；迭代上限防非单调振荡（dead 跳过 vs known_f64 收缩方向
    // 相反），超限以最后状态返回（保守）。
    // 首轮 known_f64 基于乐观形参假设（全 true）预计算，供 can_throw 首轮判定。
    let mut dead_blocks: Vec<HashSet<usize>> = vec![HashSet::new(); num_functions];
    let mut known_f64: Vec<HashSet<ValueId>> = (0..num_functions)
        .map(|i| {
            compute_known_f64(
                module,
                &infos[i],
                &param_is_f64[i],
                &var_store_count,
                &can_throw,
                &returns_f64,
            )
        })
        .collect();
    for _ in 0..64 {
        // 1. can_throw：用上一轮 known_f64/dead_blocks 判定（每轮乐观起点重算）。
        let next_can_throw = compute_can_throw(
            module,
            &infos,
            &var_store_count,
            &known_f64,
            &dead_blocks,
        );

        // 2. known_f64：用新 can_throw/returns_f64 重算（形参收缩 → 集合单调收缩）。
        let next_known_f64: Vec<HashSet<ValueId>> = (0..num_functions)
            .map(|i| {
                compute_known_f64(
                    module,
                    &infos[i],
                    &param_is_f64[i],
                    &var_store_count,
                    &next_can_throw,
                    &returns_f64,
                )
            })
            .collect();

        // 3. returns_f64：用新 known_f64 收紧。
        let next_returns = compute_returns_f64(module, &next_known_f64);

        // 4. 调用点 AND 修正形参。
        let mut next_params = param_is_f64.clone();
        for g_idx in 0..num_functions {
            if next_params[g_idx].is_empty() {
                continue;
            }
            for i in 0..next_params[g_idx].len() {
                next_params[g_idx][i] = compute_param_is_f64(
                    module,
                    &call_sites,
                    &infos,
                    &var_store_count,
                    &dynamically_reachable,
                    &next_known_f64,
                    g_idx,
                    i,
                );
            }
        }

        // 5. 派生表（依赖 can_throw，随死块集单调扩展）：
        //    - never_exception：!can_throw(callee) 的精确调用结果。
        //    - constant_false：is_exception 且操作数必非异常。
        //    - dead_exception_blocks：constant_false 分支的异常目标块。
        let next_never_exception =
            compute_never_exception(module, &infos, &var_store_count, &next_can_throw);
        let next_constant_false = compute_constant_false(module, &next_never_exception);
        let next_dead_blocks = compute_dead_exception_blocks(module, &next_constant_false);

        if next_can_throw == can_throw
            && next_returns == returns_f64
            && next_params == param_is_f64
            && next_dead_blocks == dead_blocks
        {
            let truthiness_only = compute_truthiness_only(module, &known_bool);
            return F64Analysis {
                known_f64: next_known_f64,
                known_bool,
                param_is_f64,
                can_throw: next_can_throw,
                returns_f64: next_returns,
                never_exception: next_never_exception,
                constant_false: next_constant_false,
                dead_exception_blocks: next_dead_blocks,
                truthiness_only,
            };
        }
        can_throw = next_can_throw;
        returns_f64 = next_returns;
        param_is_f64 = next_params;
        known_f64 = next_known_f64;
        dead_blocks = next_dead_blocks;
    }
    // 迭代上限（非单调振荡保护）：以最后状态返回（保守：派生表随最后 can_throw）。
    let truthiness_only = compute_truthiness_only(module, &known_bool);
    let never_exception =
        compute_never_exception(module, &infos, &var_store_count, &can_throw);
    F64Analysis {
        known_f64,
        known_bool,
        param_is_f64,
        can_throw,
        returns_f64,
        never_exception: never_exception.clone(),
        constant_false: compute_constant_false(module, &never_exception),
        dead_exception_blocks: dead_blocks,
        truthiness_only,
    }
}

/// 计算函数内 known_f64（给定形参假设的固定点）。
///
/// `can_throw`/`returns_f64` 供调用结果 f64 传播：精确 callee 且
/// `!can_throw(callee) ∧ returns_f64(callee)` → 调用结果标 f64（排除 TAG_EXCEPTION）。
fn compute_known_f64(
    module: &Module,
    info: &FnInfo,
    param_flags: &[bool],
    var_store_count: &HashMap<&str, u32>,
    can_throw: &[bool],
    returns_f64: &[bool],
) -> HashSet<ValueId> {
    let constants = module.constants();
    let mut known: HashSet<ValueId> = HashSet::new();

    // 种子：Const(Number)。
    for (dest, ins) in &info.defs {
        if let Instruction::Const { constant, .. } = ins
            && let Some(Constant::Number(_)) = constants.get(constant.0 as usize)
        {
            known.insert(*dest);
        }
    }

    // 固定点迭代：Binary/Unary/Phi/LoadVar/Call 逐轮传播直至不动点。
    loop {
        let mut changed = false;

        // Binary：lhs ∧ rhs 均 f64 → f64。Add 亦含（双 f64 无字符串拼接语义）。
        for (dest, _, lhs, rhs) in &info.binaries {
            if !known.contains(dest)
                && known.contains(lhs)
                && known.contains(rhs)
            {
                known.insert(*dest);
                changed = true;
            }
        }

        // Unary（Neg/Pos/BitNot）。
        for (dest, op, value) in &info.unaries {
            if matches!(op, UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot)
                && !known.contains(dest)
                && known.contains(value)
            {
                known.insert(*dest);
                changed = true;
            }
        }

        // Phi：所有入边源均 f64。
        for (dest, sources) in &info.phis {
            if !known.contains(dest)
                && !sources.is_empty()
                && sources.iter().all(|s| known.contains(s))
            {
                known.insert(*dest);
                changed = true;
            }
        }

        // LoadVar：
        // - 有 StoreVar：形参（若为形参）与全部源均 f64 才可（重赋值路径）。
        // - 无 StoreVar：仅当是声明形参且 param_is_f64（捕获变量保守 false）。
        for (dest, name) in &info.loads {
            if known.contains(dest) {
                continue;
            }
            let param_ok = info
                .param_index
                .get(name)
                .and_then(|&i| param_flags.get(i))
                .copied()
                .unwrap_or(false);
            let is_f64 = match info.var_stores.get(name) {
                Some(sources) if !sources.is_empty() => {
                    param_ok && sources.iter().all(|s| known.contains(s))
                }
                _ => param_ok,
            };
            if is_f64 {
                known.insert(*dest);
                changed = true;
            }
        }

        // Call 结果：精确 callee 且 !can_throw ∧ returns_f64 → f64
        // （TAG_EXCEPTION handle 不可能出现，调用结果规范必为 f64 返回）。
        for (dest, callee) in &info.calls {
            if !known.contains(dest)
                && let Some(g) =
                    resolve_callee(module, info, var_store_count, *callee)
                && !can_throw[g.0 as usize]
                && returns_f64[g.0 as usize]
            {
                known.insert(*dest);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    known
}

/// 判断 builtin 是否可能抛 JS 异常（或返回 exception handle）。
///
/// 白名单仅含确定纯的 builtin；已知 f64 实参的算术 builtin 走 f64 特例（无
/// ToNumeric/ToPrimitive/reentrant）。其余一律保守 true。
fn builtin_may_throw(
    builtin: Builtin,
    known: &HashSet<ValueId>,
    args: &[ValueId],
) -> bool {
    let all_f64 = |need: usize| args.len() >= need && args.iter().take(need).all(|a| known.contains(a));
    match builtin {
        // 纯 builtin：构造/检查/读取，不抛 JS 异常。
        Builtin::CreateClosure
        | Builtin::IsException
        | Builtin::ExceptionValue
        | Builtin::CreateException
        | Builtin::NewTarget => false,
        // 双已知 f64 → 编译期直出纯 f64 运算（无 ToNumeric/ToPrimitive）。
        Builtin::AbstractCompare if all_f64(2) => false,
        Builtin::F64Mod | Builtin::F64Exp if all_f64(2) => false,
        _ => true,
    }
}

/// 计算函数级 can_throw（false→true 单调不动点）。
///
/// 判定规则（任一命中即 may-throw）：
/// - `Terminator::Throw`；
/// - 可能抛异常的 `CallBuiltin`（`builtin_may_throw`）；
/// - `Call`/`ConstructCall`/`OptionalCall`/`SuperCall`：未知 callee 保守 true，
///   已知 callee 按目标函数 can_throw 传播；
/// - 可能产生 exception handle 的 IR 指令（GetProp/NewObject/数组操作等 host
///   路径——getter/proxy/分配失败都会以 exception handle 形态传播）。
///
/// 纯指令（Binary/Unary/Compare/Const/Phi/LoadVar/StoreVar/IsException/ExceptionValue
/// 等）不贡献。can_throw=false 的充要保证：函数绝不返回/抛出 exception handle。
///
/// `dead_blocks`（上一轮派生）：死异常块中的 Throw/调用不计——该路径编译期
/// 已被折叠，不构成真实抛异常路径（死块判定依赖 can_throw，见 analyze_inner
/// 联合不动点）。
fn compute_can_throw(
    module: &Module,
    infos: &[FnInfo],
    var_store_count: &HashMap<&str, u32>,
    known_f64: &[HashSet<ValueId>],
    dead_blocks: &[HashSet<usize>],
) -> Vec<bool> {
    // 每轮从乐观起点重算（不含 prev 记忆）：外层死块集单调增 → can_throw 随之
    // 单调降（true→false 可能发生，如 work 的异常路径被折叠后不再抛）。
    let mut can_throw = vec![false; module.functions().len()];
    loop {
        let mut changed = false;
        for (fidx, f) in module.functions().iter().enumerate() {
            if can_throw[fidx] {
                continue;
            }
            let info = &infos[fidx];
            let mut throws = false;
            for (bidx, bb) in f.blocks().iter().enumerate() {
                if dead_blocks[fidx].contains(&bidx) {
                    continue;
                }
                if let Terminator::Throw { .. } = bb.terminator() {
                    throws = true;
                    break;
                }
                for ins in bb.instructions() {
                    use Instruction::*;
                    match ins {
                        CallBuiltin { builtin, args, .. } => {
                            if builtin_may_throw(*builtin, &known_f64[fidx], args) {
                                throws = true;
                                break;
                            }
                        }
                        Call { callee, .. }
                        | ConstructCall { callee, .. }
                        | OptionalCall { callee, .. } => {
                            match resolve_callee(module, info, var_store_count, *callee) {
                                Some(g) => {
                                    if can_throw[g.0 as usize] {
                                        throws = true;
                                        break;
                                    }
                                }
                                // Unknown callee：可能抛。
                                None => {
                                    throws = true;
                                    break;
                                }
                            }
                        }
                        // 可能产生 exception handle 的 host 路径（保守）。
                        SuperCall { .. }
                        | GetProp { .. }
                        | GetElem { .. }
                        | SetProp { .. }
                        | SetElem { .. }
                        | SetProto { .. }
                        | DeleteProp { .. }
                        | NewObject { .. }
                        | NewArray { .. }
                        | OptionalGetProp { .. }
                        | OptionalGetElem { .. }
                        | ObjectSpread { .. }
                        | CollectRestArgs { .. }
                        | GetSuperBase { .. }
                        | GetSuperConstructor { .. }
                        | NewPromise { .. }
                        | PromiseResolve { .. }
                        | PromiseReject { .. }
                        | StringConcatVa { .. }
                        | Suspend { .. }
                        | GeneratorSuspend { .. } => {
                            throws = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if throws {
                    break;
                }
            }
            if throws {
                can_throw[fidx] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    can_throw
}

/// 计算函数级 returns_f64（乐观起点 true，逐 return 收紧）。
///
/// 仅 direct_callable 参与（非 direct_callable 无返回值契约，恒 false）。
/// 任一 `Return { value: Some(v) }` 的 v ∉ known_f64，或 `Return { value: None }`
/// （显式 return undefined）→ false。Throw/Unreachable 终止路径不贡献返回值。
fn compute_returns_f64(module: &Module, known_f64: &[HashSet<ValueId>]) -> Vec<bool> {
    let mut returns = vec![false; module.functions().len()];
    for (fidx, f) in module.functions().iter().enumerate() {
        if !f.direct_callable() {
            continue;
        }
        let mut all_f64 = true;
        for bb in f.blocks() {
            match bb.terminator() {
                Terminator::Return { value: Some(v) } => {
                    if !known_f64[fidx].contains(v) {
                        all_f64 = false;
                        break;
                    }
                }
                Terminator::Return { value: None } => {
                    all_f64 = false;
                    break;
                }
                _ => {}
            }
        }
        returns[fidx] = all_f64;
    }
    returns
}

/// 计算调用结果必非异常的 ValueId（精确 callee 且 !can_throw）。
fn compute_never_exception(
    module: &Module,
    infos: &[FnInfo],
    var_store_count: &HashMap<&str, u32>,
    can_throw: &[bool],
) -> Vec<HashSet<ValueId>> {
    let mut out: Vec<HashSet<ValueId>> = vec![HashSet::new(); infos.len()];
    for (fidx, info) in infos.iter().enumerate() {
        for (dest, callee) in &info.calls {
            if let Some(g) = resolve_callee(module, info, var_store_count, *callee)
                && !can_throw[g.0 as usize]
            {
                out[fidx].insert(*dest);
            }
        }
    }
    out
}

/// 计算恒 false 的 IsException 结果（操作数 ∈ never_exception）。
fn compute_constant_false(
    module: &Module,
    never_exception: &[HashSet<ValueId>],
) -> Vec<HashSet<ValueId>> {
    let mut out: Vec<HashSet<ValueId>> = vec![HashSet::new(); module.functions().len()];
    for (fidx, f) in module.functions().iter().enumerate() {
        for bb in f.blocks() {
            for ins in bb.instructions() {
                match ins {
                    // 独立指令（新 IR 形态）：`%d = is_exception %v`。
                    Instruction::IsException { dest, value } => {
                        if never_exception[fidx].contains(value) {
                            out[fidx].insert(*dest);
                        }
                    }
                    // 旧 CallBuiltin 形态（兼容路径）。
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::IsException,
                        args,
                        ..
                    } => {
                        if let Some(v) = args.first()
                            && never_exception[fidx].contains(v)
                        {
                            out[fidx].insert(*dest);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// 计算死异常块（constant_false 分支的异常目标块索引）。
fn compute_dead_exception_blocks(
    module: &Module,
    constant_false: &[HashSet<ValueId>],
) -> Vec<HashSet<usize>> {
    let mut out: Vec<HashSet<usize>> = vec![HashSet::new(); module.functions().len()];
    for (fidx, f) in module.functions().iter().enumerate() {
        for (_, bb) in f.blocks().iter().enumerate() {
            if let Terminator::Branch {
                condition,
                true_block,
                ..
            } = bb.terminator()
            {
                if constant_false[fidx].contains(condition) {
                    out[fidx].insert(true_block.0 as usize);
                }
            }
        }
    }
    out
}

/// 计算 truthiness-only 的 known_bool 值（仅被 branch 条件消费）。
///
/// 任何非 `Terminator::Branch { condition }` 的消费（指令实参、Return/Throw/Switch
/// 值、Phi 源等）都会排除——这些位置需要完整 boxed bool 语义。保守：任一不确定 →
/// 不标。
fn compute_truthiness_only(
    module: &Module,
    known_bool: &[HashSet<ValueId>],
) -> Vec<HashSet<ValueId>> {
    let mut out: Vec<HashSet<ValueId>> = vec![HashSet::new(); module.functions().len()];
    for (fidx, f) in module.functions().iter().enumerate() {
        for &v in &known_bool[fidx] {
            let mut only = true;
            for bb in f.blocks() {
                for ins in bb.instructions() {
                    if instruction_uses_value(ins, v) {
                        only = false;
                        break;
                    }
                }
                if !only {
                    break;
                }
                match bb.terminator() {
                    Terminator::Return {
                        value: Some(rv),
                    } if *rv == v => {
                        only = false;
                    }
                    Terminator::Throw { value: tv } if *tv == v => {
                        only = false;
                    }
                    Terminator::Switch { value: sv, .. } if *sv == v => {
                        only = false;
                    }
                    _ => {}
                }
                if !only {
                    break;
                }
            }
            if only {
                out[fidx].insert(v);
            }
        }
    }
    out
}

/// 计算 direct_callable 函数 g 的形参 i 是否必为 f64。
///
/// = (存在指向 g 的调用点) ∧ ∀ 调用点 C（g ∈ possible_callees(C)）:
///   `args.len() > i` 且 `arg_i ∈ known_f64[caller]`。
/// Unknown callee 视为可能指向任意模块函数（含 g）。
fn compute_param_is_f64(
    module: &Module,
    call_sites: &[CallSite],
    infos: &[FnInfo],
    var_store_count: &HashMap<&str, u32>,
    dynamically_reachable: &[bool],
    known_f64: &[HashSet<ValueId>],
    g_idx: usize,
    param_idx: usize,
) -> bool {
    let g = FunctionId(g_idx as u32);
    let mut has_site = false;
    for site in call_sites {
        // 该调用点是否可能调用 g。
        // Unknown callee：仅当 g 动态可达（函数值逃逸到可能被动态调用的位置）时
        // 才可能被该调用点调用——否则（如 performance.now 等 host 方法调用点）
        // 不约束 g 的形参。
        let targets_g = match resolve_callee(
            module,
            &infos[site.caller.0 as usize],
            var_store_count,
            site.callee,
        ) {
            Some(f) => f == g,
            None => dynamically_reachable[g_idx],
        };
        if !targets_g {
            continue;
        }
        has_site = true;
        // 实参不足 → 形参为 undefined，非 f64。
        if site.args.len() <= param_idx {
            return false;
        }
        // 实参必须在该调用方 known_f64 中。
        if !known_f64[site.caller.0 as usize].contains(&site.args[param_idx]) {
            return false;
        }
    }
    has_site
}

/// 沿 def 链解析调用点 callee 的**精确**函数目标。
///
/// 返回 `Some(fn)` = 精确解析为单一函数；`None` = Unknown（可能是任意模块函数）。
/// 递归深度受限（visited 集合防环）；Phi 需全部源解析为同一函数才精确。
///
/// `GetProp` 特例：共享 env（`$N.$shared_env`）上的不可变函数声明绑定读取
/// （key ∈ known_callee_vars 且该绑定全模块仅 StoreVar 一次）恒等于初始
/// FunctionRef——与 semantic direct_call pass 的替换前提一致（该 pass 已把
/// 可解析的 LoadVar/GetProp 原地替换为 Const(FunctionRef)，此处补齐 shared-env
/// has_own 检查路径的 Phi 场景）。
fn resolve_callee(
    module: &Module,
    info: &FnInfo,
    var_store_count: &HashMap<&str, u32>,
    value_id: ValueId,
) -> Option<FunctionId> {
    fn resolve_inner(
        module: &Module,
        info: &FnInfo,
        var_store_count: &HashMap<&str, u32>,
        value_id: ValueId,
        visited: &mut HashSet<ValueId>,
    ) -> Option<FunctionId> {
        if !visited.insert(value_id) {
            return None; // def 环 → 保守 Unknown。
        }
        let def = info.defs.get(&value_id)?;
        match def {
            Instruction::Const { constant, .. } => {
                match module.constants().get(constant.0 as usize) {
                    Some(Constant::FunctionRef(f)) => Some(*f),
                    _ => None,
                }
            }
            Instruction::LoadVar { name, .. } => info.known_callees.get(name).copied(),
            Instruction::Phi { sources, .. } => {
                let mut first: Option<FunctionId> = None;
                for s in sources {
                    let r = resolve_inner(module, info, var_store_count, s.value, visited);
                    match (first, r) {
                        (None, Some(f)) => first = Some(f),
                        (Some(f1), Some(f2)) if f1 == f2 => {}
                        _ => return None,
                    }
                }
                first
            }
            Instruction::GetProp { object, key, .. } => {
                // 仅解析共享 env 上的不可变函数声明绑定读取。
                let object_is_shared_env = match info.defs.get(object) {
                    Some(Instruction::LoadVar { name, .. }) => {
                        name == "$shared_env" || name.ends_with(".$shared_env")
                    }
                    _ => false,
                };
                if !object_is_shared_env {
                    return None;
                }
                match info.defs.get(key) {
                    Some(Instruction::Const { constant, .. }) => {
                        if let Some(Constant::String(s)) =
                            module.constants().get(constant.0 as usize)
                            && let Some(&f) = info.known_callees.get(s)
                            && var_store_count.get(s.as_str()) == Some(&1)
                        {
                            Some(f)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            // GetElem/动态值等 → Unknown。
            _ => None,
        }
    }
    resolve_inner(module, info, var_store_count, value_id, &mut HashSet::new())
}

/// 指令是否使用 ValueId `v`（含 Phi 源）。
fn instruction_uses_value(ins: &Instruction, v: ValueId) -> bool {
    use Instruction::*;
    match ins {
        Binary { lhs, rhs, .. } => *lhs == v || *rhs == v,
        Unary { value, .. } | IsException { value, .. } | EncodeException { value, .. }
        | ExceptionToObject { value, .. } => *value == v,
        Compare { lhs, rhs, .. } => *lhs == v || *rhs == v,
        Phi { sources, .. } => sources.iter().any(|s| s.value == v),
        CallBuiltin { args, .. } => args.contains(&v),
        StringConcatVa { parts, .. } => parts.contains(&v),
        StoreVar { value, .. } => *value == v,
        Call {
            callee,
            this_val,
            args,
            ..
        }
        | SuperCall {
            callee,
            this_val,
            args,
            ..
        }
        | ConstructCall {
            callee,
            this_val,
            args,
            ..
        }
        | OptionalCall {
            callee,
            this_val,
            args,
            ..
        } => *callee == v || *this_val == v || args.contains(&v),
        GetProp { object, key, .. }
        | DeleteProp { object, key, .. }
        | OptionalGetProp { object, key, .. } => *object == v || *key == v,
        SetProp {
            object,
            key,
            value,
            ..
        } => *object == v || *key == v || *value == v,
        SetProto { object, value } => *object == v || *value == v,
        GetElem { object, index, .. } => *object == v || *index == v,
        OptionalGetElem { object, key, .. } => *object == v || *key == v,
        SetElem {
            object,
            index,
            value,
        } => *object == v || *index == v || *value == v,
        ObjectSpread { source, .. } => *source == v,
        PromiseResolve { promise, value } => *promise == v || *value == v,
        PromiseReject { promise, reason } => *promise == v || *reason == v,
        Const { .. }
        | LoadVar { .. }
        | NewObject { .. }
        | NewArray { .. }
        | GetSuperBase { .. }
        | GetSuperConstructor { .. }
        | NewPromise { .. }
        | CollectRestArgs { .. }
        | Suspend { .. }
        | GeneratorSuspend { .. }
        | DebugCheck { .. } => false,
    }
}

/// 判断函数 g 是否"动态可达"：其函数值（FunctionRef / create_closure 结果）是否
/// 逃逸到可能被动态调用的位置。
///
/// 逃逸位置（任一命中即逃逸）：
/// - 作为 CallBuiltin 实参（`create_closure` 除外——只是包装闭包）；
/// - 作为 Call/SuperCall/ConstructCall/OptionalCall 实参；
/// - StoreVar 到非不可变绑定（name ∉ known_callee_vars 或全模块 store 次数 ≠ 1）；
/// - SetProp/SetElem 值（SetProp key 非 known_callee_vars 绑定时）；
/// - GetProp/GetElem 的 object/key、SetProto 值、Return/Throw 值；
/// - 作为调用点 callee 但该调用点不精确解析为 g（const-fnref 必然精确，保守兜底）。
///
/// Phi 使用传递：值作为 Phi 源 → 继续检查 Phi dest 的使用（如 has_own 检查路径）。
/// 非逃逸函数不可能被 Unknown 调用点调用 → Unknown 调用点不约束其形参。
fn compute_dynamically_reachable(
    module: &Module,
    infos: &[FnInfo],
    var_store_count: &HashMap<&str, u32>,
) -> Vec<bool> {
    let num_functions = infos.len();
    let mut reachable = vec![false; num_functions];
    // worklist: (定义函数索引, ValueId, 所属函数 g 索引)。
    // ValueId 每函数独立编号，使用扫描只在本函数内进行。
    let mut worklist: Vec<(usize, ValueId, usize)> = Vec::new();
    for (fn_idx, info) in infos.iter().enumerate() {
        for (dest, ins) in &info.defs {
            match ins {
                Instruction::Const { constant, .. } => {
                    if let Some(Constant::FunctionRef(f)) =
                        module.constants().get(constant.0 as usize)
                    {
                        worklist.push((fn_idx, *dest, f.0 as usize));
                    }
                }
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::CreateClosure,
                    args,
                    ..
                } => {
                    if let Some(a) = args.first()
                        && let Some(f) = resolve_callee(module, info, var_store_count, *a)
                    {
                        worklist.push((fn_idx, *dest, f.0 as usize));
                    }
                }
                _ => {}
            }
        }
    }

    let mut visited: HashSet<(usize, ValueId)> = HashSet::new();
    while let Some((fn_idx, v, g_idx)) = worklist.pop() {
        if !visited.insert((fn_idx, v)) {
            continue;
        }
        let info = &infos[fn_idx];
        let mut phi_dests: Vec<(usize, ValueId, usize)> = Vec::new();
        for bb in info.function.blocks() {
            for ins in bb.instructions() {
                if !instruction_uses_value(ins, v) {
                    continue;
                }
                match ins {
                    // 作为调用点 callee：直接调用位——须精确解析为 g（const-fnref
                    // 做 callee 必然精确，保守兜底）。若 v 是 this/实参则逃逸。
                    Instruction::Call { callee, .. }
                    | Instruction::ConstructCall { callee, .. }
                    | Instruction::OptionalCall { callee, .. }
                    | Instruction::SuperCall { callee, .. } => {
                        if *callee != v
                            || resolve_callee(module, info, var_store_count, *callee)
                                != Some(FunctionId(g_idx as u32))
                        {
                            reachable[g_idx] = true;
                        }
                    }
                    // create_closure 包装：安全。
                    Instruction::CallBuiltin {
                        builtin: Builtin::CreateClosure,
                        args,
                        ..
                    } => {
                        if !args.contains(&v) {
                            reachable[g_idx] = true;
                        }
                    }
                    // 其他 CallBuiltin 实参：逃逸（host 可能调用回调）。
                    Instruction::CallBuiltin { .. } => {
                        reachable[g_idx] = true;
                    }
                    // StoreVar：不可变函数声明绑定（known_callee_vars + 唯一 store）
                    // 安全；否则逃逸。
                    Instruction::StoreVar { name, .. } => {
                        let immutable = info.known_callees.contains_key(name.as_str())
                            && var_store_count.get(name.as_str()) == Some(&1);
                        if !immutable {
                            reachable[g_idx] = true;
                        }
                    }
                    // SetProp：key 为 known_callee_vars 绑定（共享 env 函数属性）
                    // 安全；否则逃逸。
                    Instruction::SetProp { key, .. } => {
                        let key_is_known_binding = match info.defs.get(key) {
                            Some(Instruction::Const { constant, .. }) => {
                                if let Some(Constant::String(s)) =
                                    module.constants().get(constant.0 as usize)
                                {
                                    info.known_callees.contains_key(s)
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        };
                        if !key_is_known_binding {
                            reachable[g_idx] = true;
                        }
                    }
                    // SetElem/SetProto/属性读写/运算/返回等其余使用：逃逸。
                    Instruction::SetElem { .. }
                    | Instruction::SetProto { .. }
                    | Instruction::GetProp { .. }
                    | Instruction::GetElem { .. }
                    | Instruction::OptionalGetProp { .. }
                    | Instruction::OptionalGetElem { .. }
                    | Instruction::DeleteProp { .. }
                    | Instruction::ObjectSpread { .. }
                    | Instruction::Unary { .. }
                    | Instruction::Binary { .. }
                    | Instruction::Compare { .. }
                    | Instruction::StringConcatVa { .. }
                    | Instruction::IsException { .. }
                    | Instruction::EncodeException { .. }
                    | Instruction::ExceptionToObject { .. }
                    | Instruction::PromiseResolve { .. }
                    | Instruction::PromiseReject { .. }
                    | Instruction::Const { .. }
                    | Instruction::LoadVar { .. }
                    | Instruction::NewObject { .. }
                    | Instruction::NewArray { .. }
                    | Instruction::GetSuperBase { .. }
                    | Instruction::GetSuperConstructor { .. }
                    | Instruction::NewPromise { .. }
                    | Instruction::CollectRestArgs { .. }
                    | Instruction::Suspend { .. }
                    | Instruction::GeneratorSuspend { .. }
                    | Instruction::DebugCheck { .. } => {
                        reachable[g_idx] = true;
                    }
                    // Phi 源：传递检查 Phi dest 的使用。
                    Instruction::Phi { dest, .. } => {
                        phi_dests.push((fn_idx, *dest, g_idx));
                    }
                }
            }
            // 终止器使用（Return/Throw 值）→ 逃逸。
            match bb.terminator() {
                wjsm_ir::Terminator::Return { value: Some(val) } if *val == v => {
                    reachable[g_idx] = true;
                }
                wjsm_ir::Terminator::Throw { value } if *value == v => {
                    reachable[g_idx] = true;
                }
                _ => {}
            }
        }
        worklist.extend(phi_dests);
    }
    reachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::Program;
    use wjsm_parser::parse_module;
    use wjsm_semantic::lower_module;

    fn lower(source: &str) -> Program {
        lower_module(parse_module(source).expect("parse"), false).expect("lower")
    }

    use wjsm_parser::parse_script_as_module;

    /// 声明名匹配的函数 FunctionId。
    fn function_id(program: &Program, name: &str) -> FunctionId {
        let idx = program
            .functions()
            .iter()
            .position(|f| f.name() == name)
            .unwrap_or_else(|| panic!("function `{name}` not found"));
        FunctionId(idx as u32)
    }

    /// 验证某 ValueId（由产生它的指令模式定位）在函数内 known_f64。
    fn assert_value_known_f64(
        program: &Program,
        analysis: &F64Analysis,
        fn_name: &str,
        value_id: ValueId,
    ) {
        let fid = function_id(program, fn_name);
        assert!(
            analysis.value_known_f64(fid, value_id),
            "value {value_id} in `{fn_name}` should be known f64"
        );
    }

    #[test]
    fn fib_param_propagates_from_entry_call_and_recursion() {
        // fib(10) 经 module_main 动态 Phi 调用点（Unknown callee，实参 const 10）
        // 播种形参；递归 `fib(n-1)`/`fib(n-2)` 在形参 f64 假设下自洽。期望 [true]。
        // 用与 CLI `-e` 完全一致的 lowering（script 模式）验证。
        let program = lower_module(
            parse_script_as_module(
                "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } console.log(fib(10));",
            )
            .expect("parse"),
            true,
        )
        .expect("lower");
        let analysis = F64Analysis::analyze(&program);
        let fib = function_id(&program, "fib");

        assert_eq!(
            analysis.param_is_f64_vec(fib),
            Some(&[true][..]),
            "fib 形参 n 必须传播为 f64"
        );
        // 递归子表达式也应被标记。
        for block in program.functions()[fib.0 as usize].blocks() {
            for ins in block.instructions() {
                if let Instruction::Binary {
                    dest, op: BinaryOp::Sub, ..
                } = ins
                {
                    assert_value_known_f64(&program, &analysis, "fib", *dest);
                }
            }
        }
    }

    #[test]
    fn unknown_callee_arg_kills_param() {
        // g 被 f 以形参 n（未知）调用：g 的形参必须 [false]。
        let program = lower("function f(n) { return g(n); } function g(x) { return x; }");
        let analysis = F64Analysis::analyze(&program);
        let g = function_id(&program, "g");
        assert_eq!(
            analysis.param_is_f64_vec(g),
            Some(&[false][..]),
            "g 的形参 x 由 f 的未知形参 n 传入，必须保守 false"
        );
    }

    #[test]
    fn no_call_site_means_false() {
        // f 无任何调用点：形参乐观起点必须被收紧为 false（空 AND 不成立）。
        let program = lower("function f(n) { return n + 1; }");
        let analysis = F64Analysis::analyze(&program);
        let f = function_id(&program, "f");
        assert_eq!(
            analysis.param_is_f64_vec(f),
            Some(&[false][..]),
            "无调用点的函数形参必须 false"
        );
    }

    #[test]
    fn string_arg_to_known_callee_kills_param() {
        // h 只被 h("abc") 调用（实参字符串，非 f64）：形参必须 false。
        let program = lower("function h(s) { return s; } console.log(h(\"abc\"));");
        let analysis = F64Analysis::analyze(&program);
        let h = function_id(&program, "h");
        assert_eq!(
            analysis.param_is_f64_vec(h),
            Some(&[false][..]),
            "字符串实参必须使形参保守 false"
        );
    }

    #[test]
    fn fewer_args_than_params_kills_param() {
        // f 有 2 个形参但只被 f(1) 调用：形参 1 实际是 undefined → false。
        let program = lower("function f(a, b) { return a; } console.log(f(1));");
        let analysis = F64Analysis::analyze(&program);
        let f = function_id(&program, "f");
        assert_eq!(
            analysis.param_is_f64_vec(f),
            Some(&[true, false][..]),
            "实参不足的形参必须 false"
        );
    }

    #[test]
    fn compare_dest_is_known_bool() {
        let program = lower(
            "function f(n) { return n < 2; } console.log(f(1));",
        );
        let analysis = F64Analysis::analyze(&program);
        let f = function_id(&program, "f");
        let mut compare_dests = Vec::new();
        for block in program.functions()[f.0 as usize].blocks() {
            for ins in block.instructions() {
                if let Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::AbstractCompare,
                    ..
                } = ins
                {
                    compare_dests.push(*dest);
                }
            }
        }
        assert_eq!(compare_dests.len(), 1, "`n < 2` 应降低为一次 AbstractCompare");
        assert!(
            analysis.value_known_bool(f, compare_dests[0]),
            "AbstractCompare 的 dest 必须 known_bool"
        );
    }

    #[test]
    fn dynamic_host_method_calls_do_not_kill_param() {
        // 复刻 bench/scenarios/fib30.js 的调用图：module_main 经共享 env Phi 调用
        // work()（Unknown callee、0 实参），另有 performance.now() 这类 host 方法
        // 动态调用点（Unknown callee、0 实参）。fib 的值不逃逸到这些调用点
        // （仅 work 的直接调用 + 递归），故 fib 形参必须仍为 [true]。
        // 回归：早期实现把所有 Unknown 调用点当作"可能调用任意模块函数"，
        // performance.now() 的 0 实参把 fib 形参误杀为 false。
        let program = lower(
            r#"
function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
function work() {
  fib(30);
}
var t0 = performance.now();
work();
console.log(performance.now() - t0);
"#,
        );
        let analysis = F64Analysis::analyze(&program);
        let fib = function_id(&program, "fib");
        assert_eq!(
            analysis.param_is_f64_vec(fib),
            Some(&[true][..]),
            "host 方法动态调用点不得杀死 fib 形参"
        );
    }

    #[test]
    fn function_value_escape_kills_param() {
        // fib 的值被赋值给非函数声明绑定（f = fib）后动态调用：
        // f 是 let 绑定（非 known_callee_vars）→ fib 逃逸 → f("x") 的 Unknown
        // 调用点（字符串实参）必须把 fib 形参杀为 false。
        let program = lower(
            r#"
function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }
var f = fib;
f("abc");
"#,
        );
        let analysis = F64Analysis::analyze(&program);
        let fib = function_id(&program, "fib");
        assert_eq!(
            analysis.param_is_f64_vec(fib),
            Some(&[false][..]),
            "函数值逃逸到动态绑定后必须保守 false"
        );
    }
}
