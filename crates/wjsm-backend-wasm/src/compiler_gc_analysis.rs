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
use wjsm_ir::{Builtin, FunctionId, Instruction, Module, ValueId};

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
    /// 仅记录可追溯到已知函数声明（known_callee_vars）的 Call。
    /// 如果 callee 函数 may_gc == false，该 Call 可省 safepoint spill。
    call_targets: HashMap<(FunctionId, ValueId), FunctionId>,
}

impl GcAnalysis {
    /// 执行模块级 GC 分析。
    ///
    /// 返回 `GcAnalysis { may_gc: Vec<bool> }`，每个函数一个 bool。
    pub fn analyze(module: &Module) -> Self {
        let num_functions = module.functions().len();
        if num_functions == 0 {
            return Self {
                may_gc: Vec::new(),
                call_targets: HashMap::new(),
            };
        }

        // ── 阶段 1：扫描每个函数体，收集 direct_may_gc + call_edges + unknown_callee ──
        let mut direct_may_gc = vec![false; num_functions];
        let mut call_edges: Vec<Vec<FunctionId>> = vec![Vec::new(); num_functions];
        let mut unknown_callee = vec![false; num_functions];
        let mut call_targets: HashMap<(FunctionId, ValueId), FunctionId> = HashMap::new();

        for (func_idx, function) in module.functions().iter().enumerate() {
            let func_id = FunctionId(func_idx as u32);
            let known_callees = function.known_callee_vars();

            // 构建该函数体内 ValueId → LoadVar name 的映射（用于追溯 Call 的 callee 来源）
            let mut loadvar_map: HashMap<ValueId, String> = HashMap::new();

            for bb in function.blocks() {
                for ins in bb.instructions() {
                    // 记录所有 LoadVar 的 dest → name
                    if let Instruction::LoadVar { dest, name } = ins {
                        loadvar_map.insert(*dest, name.clone());
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
                        Instruction::CallBuiltin { builtin, .. } => {
                            if builtin_may_trigger_gc(builtin) {
                                direct_may_gc[func_idx] = true;
                            }
                        }

                        // ── Call：追溯 callee ──
                        Instruction::Call { callee, .. } => {
                            // 检查 callee ValueId 是否来自 LoadVar，且 name 在 known_callee_vars 中
                            if let Some(var_name) = loadvar_map.get(callee) {
                                if let Some(&callee_fn_id) = known_callees.get(var_name) {
                                    // 精确追溯：callee 是已知函数声明
                                    call_edges[func_idx].push(callee_fn_id);
                                    call_targets.insert((func_id, *callee), callee_fn_id);
                                } else {
                                    // LoadVar 但不在 known_callee_vars → unknown callee
                                    unknown_callee[func_idx] = true;
                                }
                            } else {
                                // callee 不来自 LoadVar → unknown callee
                                // (可能来自 GetProp/GetElem/Phi/Call result 等)
                                unknown_callee[func_idx] = true;
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
