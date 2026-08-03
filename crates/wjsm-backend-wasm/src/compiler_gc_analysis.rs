//! 模块级 GC 分析（Layer 3）。
//!
//! 分析每个 IR 函数是否可能触发 GC：
//! - 含 NewObject/NewArray/ObjectSpread/CollectRestArgs/NewPromise/PromiseResolve/PromiseReject
//!   /StringConcatVa 等 → 直接 may-GC
//! - CallBuiltin：查 `builtin_may_trigger_gc` 白名单反面
//! - Call：若 callee 是 LoadVar 且 name∈known_callee_vars → 追溯 callee 函数的 may-GC 状态
//!   否则 unknown callee → 保守 may-GC
//!
//! 不动点迭代求传递闭包：`may_gc[f] = direct_may_gc[f] OR ∃edge f→t: may_gc[t] OR has_unknown_callee[f]`
//!
//! **GC 正确性红线**：unknown callee 一律保守 may-GC；只对单次赋值的函数声明变量建映射。

use std::collections::HashMap;
use wjsm_ir::{BinaryOp, Builtin, Constant, FunctionId, Instruction, Module, ValueId};

/// callee 的静态来源（ValueId 的 def 判别）。
#[derive(Debug, Clone)]
enum CalleeSource {
    /// `LoadVar(name)`：绑定读取，需经 known_callee_vars 解析。
    LoadVar(String),
    /// `Const(FunctionRef(id))`：direct_call pass 替换后的函数引用，可精确解析。
    FunctionRef(FunctionId),
}

/// 该 builtin 是否**可能**触发 GC（分配堆对象或 reentrant 回用户 JS）。
///
/// 这是 `builtin_returns_scalar` 的逻辑补集：`may_trigger_gc = !returns_scalar`。
/// 统一由 `crate::analysis_value_ty::builtin_returns_scalar` 维护单一白名单，
/// 新增 Builtin variant 时只需更新一处，编译器自动保证两层一致性。
///
/// 返回 `true` 表示该 builtin **可能**触发 GC，需要 safepoint spill；
/// 返回 `false` 表示该 builtin **规范保证**不触发 GC，可省 spill。
///
/// **保守原则**：任何不确定的 builtin 一律返回 true（宁滥勿缺）。
fn builtin_may_trigger_gc(b: &Builtin) -> bool {
    !crate::analysis_value_ty::builtin_returns_scalar(b)
}

/// GC 分析结果。
#[derive(Debug, Clone)]
pub struct GcAnalysis {
    /// 每个函数是否可能触发 GC。索引 = FunctionId.0。
    /// `true` = 可能触发 GC，Call 该函数需要 safepoint spill。
    /// `false` = 不触发 GC，可省 spill（仅当 callee 是已知函数声明）。
    may_gc: Vec<bool>,

    /// Per-Call no-GC 信息：`(function_id, call_callee_value_id) → callee_FunctionId`。
    /// 仅记录可追溯到已知函数声明（known_callee_vars / Const FunctionRef）的 Call。
    /// 如果 callee 函数 may_gc == false，该 Call 可省 safepoint spill。
    call_targets: HashMap<(FunctionId, ValueId), FunctionId>,

    /// 可发射直接调用的目标：`(调用函数, callee ValueId) → callee FunctionId`。
    /// 仅当 callee 的 def 是 `Const(FunctionRef)` 且目标函数 `direct_callable`（Phase 3）。
    direct_call_targets: HashMap<(FunctionId, ValueId), FunctionId>,
}

impl GcAnalysis {
    /// 执行模块级 GC 分析。
    ///
    /// `f64_analysis` 供 Layer 2 的 f64 值类型传播（Step 1）消费：
    /// `AbstractCompare` 若实参均为已知 f64，编译期直出 `f64.lt`（fast path 必走，
    /// 无 ToPrimitive/reentrant）→ 不算 GC 指令，从而让纯数值函数 `may_gc=false`，
    /// 递归调用点省 spill。
    ///
    /// 返回 `GcAnalysis { may_gc: Vec<bool> }`，每个函数一个 bool。
    pub fn analyze(module: &Module, f64_analysis: &crate::analysis_f64::F64Analysis) -> Self {
        let num_functions = module.functions().len();
        if num_functions == 0 {
            return Self {
                may_gc: Vec::new(),
                call_targets: HashMap::new(),
                direct_call_targets: HashMap::new(),
            };
        }

        // ── 阶段 1：扫描每个函数体，收集 direct_may_gc + call_edges + unknown_callee ──
        let mut direct_may_gc = vec![false; num_functions];
        let mut call_edges: Vec<Vec<FunctionId>> = vec![Vec::new(); num_functions];
        let mut unknown_callee = vec![false; num_functions];
        let mut call_targets: HashMap<(FunctionId, ValueId), FunctionId> = HashMap::new();
        let mut direct_call_targets: HashMap<(FunctionId, ValueId), FunctionId> = HashMap::new();

        for (func_idx, function) in module.functions().iter().enumerate() {
            let func_id = FunctionId(func_idx as u32);
            let known_callees = function.known_callee_vars();

            // 构建该函数体内 ValueId → def 来源的映射（用于追溯 Call 的 callee）
            let mut callee_sources: HashMap<ValueId, CalleeSource> = HashMap::new();

            for (bb_index, bb) in function.blocks().iter().enumerate() {
                // 死异常块（is_exception 恒 false 分支的异常目标）不可达：
                // 其 host 调用（exception_value/throw 等）不贡献 may-GC，
                // 使纯化后的 work 类函数（无真正异常路径）判定为 no-GC。
                if f64_analysis.is_dead_exception_block(func_id, bb_index) {
                    continue;
                }
                for ins in bb.instructions() {
                    // 记录 def：LoadVar → name；Const(FunctionRef) → 直接函数引用。
                    match ins {
                        Instruction::LoadVar { dest, name } => {
                            callee_sources.insert(*dest, CalleeSource::LoadVar(name.clone()));
                        }
                        Instruction::Const { dest, constant } => {
                            if let Some(Constant::FunctionRef(id)) =
                                module.constants().get(constant.0 as usize)
                            {
                                callee_sources
                                    .insert(*dest, CalleeSource::FunctionRef(*id));
                            }
                        }
                        _ => {}
                    }

                    match ins {
                        // ── 直接 GC 指令（分配堆对象）──
                        Instruction::NewObject { .. }
                        | Instruction::NewArray { .. }
                        | Instruction::ObjectSpread { .. }
                        | Instruction::CollectRestArgs { .. }
                        | Instruction::NewPromise { .. }
                        | Instruction::PromiseResolve { .. }
                        | Instruction::PromiseReject { .. }
                        | Instruction::StringConcatVa { .. } => {
                            direct_may_gc[func_idx] = true;
                        }

                        // ── CallBuiltin：按白名单反面判定 ──
                        // AbstractCompare 双已知 f64 → 编译期直出 f64.lt，不算 GC 指令。
                        Instruction::CallBuiltin {
                            builtin: Builtin::AbstractCompare,
                            args,
                            ..
                        } => {
                            let both_known_f64 = args.len() >= 2
                                && f64_analysis.value_known_f64(func_id, args[0])
                                && f64_analysis.value_known_f64(func_id, args[1]);
                            if !both_known_f64 {
                                // AbstractCompare 不在标量白名单，slow path 可能触发 GC。
                                direct_may_gc[func_idx] = true;
                            }
                        }
                        Instruction::CallBuiltin { builtin, .. } => {
                            if builtin_may_trigger_gc(builtin) {
                                direct_may_gc[func_idx] = true;
                            }
                        }

                        // ── Binary：加法可能触发字符串拼接（host string_concat 产 runtime string handle）──
                        // 双已知 f64 的 add 编译期直出 f64.add（无字符串语义）→ 不算 GC 指令；
                        // 其余 add 运行期可能落 string_concat（host 字符串表分配，可能触发
                        // 字符串清扫/GC）→ 保守 may-GC（与 AbstractCompare 同款判定）。
                        Instruction::Binary {
                            op: BinaryOp::Add,
                            lhs,
                            rhs,
                            ..
                        } => {
                            let both_known_f64 = f64_analysis.value_known_f64(func_id, *lhs)
                                && f64_analysis.value_known_f64(func_id, *rhs);
                            if !both_known_f64 {
                                direct_may_gc[func_idx] = true;
                            }
                        }

                        // ── Call：追溯 callee ──
                        Instruction::Call { callee, .. } => {
                            match callee_sources.get(callee) {
                                // callee 是 direct_call pass 替换后的 Const(FunctionRef)
                                Some(CalleeSource::FunctionRef(callee_fn_id)) => {
                                    let callee_fn_id = *callee_fn_id;
                                    call_edges[func_idx].push(callee_fn_id);
                                    call_targets.insert((func_id, *callee), callee_fn_id);
                                    // 目标函数 direct_callable → 调用点可发射直接 call。
                                    if module
                                        .functions()
                                        .get(callee_fn_id.0 as usize)
                                        .is_some_and(|f| f.direct_callable())
                                    {
                                        direct_call_targets
                                            .insert((func_id, *callee), callee_fn_id);
                                    }
                                }
                                // callee 来自 LoadVar：经 known_callee_vars 解析（原有逻辑）
                                Some(CalleeSource::LoadVar(var_name)) => {
                                    if let Some(&callee_fn_id) = known_callees.get(var_name) {
                                        // 精确追溯：callee 是已知函数声明
                                        call_edges[func_idx].push(callee_fn_id);
                                        call_targets.insert((func_id, *callee), callee_fn_id);
                                    } else {
                                        // LoadVar 但不在 known_callee_vars → unknown callee
                                        unknown_callee[func_idx] = true;
                                    }
                                }
                                // callee 不来自 LoadVar/FunctionRef → unknown callee
                                // (可能来自 GetProp/GetElem/Phi/Call result 等)
                                None => {
                                    unknown_callee[func_idx] = true;
                                }
                            }
                        }

                        // SuperCall/ConstructCall：构造调用几乎必分配，保守 may-GC
                        Instruction::SuperCall { .. } | Instruction::ConstructCall { .. } => {
                            direct_may_gc[func_idx] = true;
                        }

                        _ => {}
                    }
                }
            }
        }

        // ── 阶段 2：不动点迭代求传递闭包 ──
        let mut may_gc = direct_may_gc;

        let mut changed = true;
        while changed {
            changed = false;

            for func_idx in 0..num_functions {
                if may_gc[func_idx] {
                    continue;
                }

                // 如果有 unknown callee，保守 may-GC
                if unknown_callee[func_idx] {
                    may_gc[func_idx] = true;
                    changed = true;
                    continue;
                }

                // 如果调用的已知 callee 中有 may-GC 的，则 caller 也 may-GC
                for &callee_fn_id in &call_edges[func_idx] {
                    if may_gc[callee_fn_id.0 as usize] {
                        may_gc[func_idx] = true;
                        changed = true;
                        break;
                    }
                }
            }
        }

        Self {
            may_gc,
            call_targets,
            direct_call_targets,
        }
    }

    /// 查询某函数是否可能触发 GC。
    ///
    /// 返回 `true` = 可能触发 GC，Call 该函数需要 safepoint spill。
    /// 返回 `false` = 不触发 GC，可省 spill。
    ///
    /// **保守原则**：超出范围的 FunctionId 一律返回 true。
    pub fn function_may_gc(&self, func_id: FunctionId) -> bool {
        self.may_gc.get(func_id.0 as usize).copied().unwrap_or(true)
    }

    /// 查询特定 Call 指令是否需要 safepoint spill（Layer 3d）。
    ///
    /// `caller_func_id` = 当前正在编译的函数的 FunctionId
    /// `callee_value_id` = Call 指令的 callee ValueId
    ///
    /// 返回 `true` = 需要 spill（callee 可能触发 GC，或无法追溯到已知函数声明）。
    /// 返回 `false` = 可省 spill（callee 是已知 no-GC 函数）。
    pub fn call_may_trigger_gc(
        &self,
        caller_func_id: FunctionId,
        callee_value_id: ValueId,
    ) -> bool {
        // 查 call_targets 精确追溯结果
        if let Some(&callee_fn_id) = self.call_targets.get(&(caller_func_id, callee_value_id)) {
            // callee 是已知函数声明，查其 may-GC 状态
            self.function_may_gc(callee_fn_id)
        } else {
            // 无法追溯 → 保守 may-GC
            true
        }
    }

    /// 查询调用点是否可发射直接调用（Phase 3）。
    ///
    /// 仅当 callee 的 def 是 `Const(FunctionRef)` 且目标函数 `direct_callable` 时返回 Some。
    pub fn direct_call_target(
        &self,
        caller_func_id: FunctionId,
        callee_value_id: ValueId,
    ) -> Option<FunctionId> {
        self.direct_call_targets
            .get(&(caller_func_id, callee_value_id))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_may_trigger_gc_scalar_builtin() {
        // 白名单中的 builtin 不触发 GC
        use Builtin::*;
        assert!(!builtin_may_trigger_gc(&MathAbs));
        assert!(!builtin_may_trigger_gc(&NumberConstructor));
        assert!(!builtin_may_trigger_gc(&ArrayIsArray));
        assert!(!builtin_may_trigger_gc(&StringCharCodeAt));
        assert!(!builtin_may_trigger_gc(&IteratorDone));
        assert!(!builtin_may_trigger_gc(&IsCallable));
        assert!(!builtin_may_trigger_gc(&In));
        assert!(!builtin_may_trigger_gc(&RegExpTest));
    }

    #[test]
    fn test_builtin_may_trigger_gc_alloc_builtin() {
        // 分配对象的 builtin 触发 GC
        use Builtin::*;
        assert!(builtin_may_trigger_gc(&ConsoleLog));
        assert!(builtin_may_trigger_gc(&ArrayPush));
        assert!(builtin_may_trigger_gc(&ObjectKeys));
        assert!(builtin_may_trigger_gc(&StringSlice));
        assert!(builtin_may_trigger_gc(&MapConstructor));
        assert!(builtin_may_trigger_gc(&PromiseCreate));
    }
}
