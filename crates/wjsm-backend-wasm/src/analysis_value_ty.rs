//! Per-ValueId 类型推断：区分 Handle（需 GC root）与 Scalar。
//!
//! 供 GC safepoint spill 用（spec §11.2）。
//!
//! 算法分两阶段：
//! 1. **初始化遍**（`dest_and_kind`）：对每个 producing 指令做确定性分类——
//!    算术/比较/已知标量 builtin → Scalar；对象分配/多态 → Handle。
//! 2. **固定点迭代**：处理"级联污染"——StoreVar→LoadVar 类型传播 + Phi 折叠。
//!    典型 JS 中大量变量是 number（循环计数器、中间结果），但单遍分析只能把
//!    LoadVar 判 Handle；固定点迭代让"所有 StoreVar 源都是 Scalar 的变量"的
//!    LoadVar 也降为 Scalar，进而让消费它的 Phi 降级，链式传播直至不动点。
//!
//! **安全性（soundness）**：算法只能 Handle→Scalar（减少 spill），绝不反向。
//! 每次降级要求所有上游源都是 Scalar；未被 StoreVar 的变量（函数参数、捕获变量）
//! 不降级；Phi 任一入边 Handle 则不降级。误判（把 Handle 当 Scalar）会导致 GC
//! 漏 root → 悬垂指针，故一律保守。
use std::collections::HashMap;
use wjsm_ir::{Builtin, Constant, Function, Instruction, Module, ValueId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueTy {
    /// 持有 GC handle（对象/数组/闭包/...）。spill 时需暴露给 shadow stack 扫描。
    Handle,
    /// 纯标量（number/bool/undefined/null）。spill 时跳过。
    Scalar,
}

/// 推断函数内每个 producing instruction 的 dest 类型（含固定点传播）。
///
/// 返回 `HashMap<ValueId, ValueTy>`。未出现在 map 中的 ValueId（理论上不应发生）
/// 在调用方保守视为 Handle。
pub fn infer_value_ty(module: &Module, function: &Function) -> HashMap<ValueId, ValueTy> {
    infer_value_and_var_ty(module, function).0
}

/// 同 `infer_value_ty`，并额外返回**每个变量**的类型（按 `StoreVar` 源折叠）。
///
/// 变量类型供 GC safepoint 的变量 spill 过滤：某变量所有 `StoreVar` 源都是 Scalar
/// → Scalar（safepoint 不 spill，避免热循环里多 spill 标量内建如 `Math.E`）；否则
/// Handle（保守，含 handle 源或未分类源）。从未被 `StoreVar` 的变量（函数参数 / 捕获
/// 变量）不在返回 map 中，由调用方默认按 Handle 处理（保守 spill）。
pub fn infer_value_and_var_ty(
    module: &Module,
    function: &Function,
) -> (HashMap<ValueId, ValueTy>, HashMap<String, ValueTy>) {
    let constants = module.constants();

    // ── 阶段 1：初始化遍，确定性分类 ──
    let mut ty: HashMap<ValueId, ValueTy> = HashMap::new();
    // 结构信息（供阶段 2 固定点迭代用）
    // var_stores[name] = 所有 StoreVar 到该变量的源 ValueId 列表（跨所有 block）
    let mut var_stores: HashMap<String, Vec<ValueId>> = HashMap::new();
    // load_vars = 所有 LoadVar 的 (dest, name)
    let mut load_vars: Vec<(ValueId, String)> = Vec::new();
    // phis = 所有 Phi 的 (dest, source_values)
    let mut phis: Vec<(ValueId, Vec<ValueId>)> = Vec::new();

    for bb in function.blocks() {
        for ins in bb.instructions() {
            match ins {
                Instruction::StoreVar { name, value } => {
                    var_stores.entry(name.clone()).or_default().push(*value);
                }
                Instruction::LoadVar { dest, name } => {
                    load_vars.push((*dest, name.clone()));
                }
                Instruction::Phi { dest, sources } => {
                    phis.push((*dest, sources.iter().map(|s| s.value).collect()));
                }
                _ => {}
            }
            if let Some((dest, kind)) = dest_and_kind(ins, constants) {
                ty.insert(dest, kind);
            }
        }
    }

    // ── 阶段 2：固定点迭代（StoreVar→LoadVar 传播 + Phi 折叠）──
    // 终止性：每次迭代至少把一个 Handle 降为 Scalar，map 大小有限，故必然终止。
    // 复杂度：O((|LoadVar|+|Phi|) × 迭代轮数 × 平均入度)，实践中 2-4 轮收敛。
    let mut changed = true;
    while changed {
        changed = false;

        // StoreVar→LoadVar 传播：若某变量所有 StoreVar 源值当前都是 Scalar，
        // 则该变量的 LoadVar 也降为 Scalar。
        for (dest, name) in &load_vars {
            if ty.get(dest) != Some(&ValueTy::Handle) {
                continue; // 已是 Scalar 或未分类（未分类不在此处理）
            }
            let Some(sources) = var_stores.get(name) else {
                continue; // 从未被 StoreVar（函数参数/捕获变量）→ 保守不降级
            };
            if sources.is_empty() {
                continue;
            }
            // 要求所有源都已分类为 Scalar（缺失视为 Handle，保守不降级）
            let all_scalar = sources.iter().all(|s| ty.get(s) == Some(&ValueTy::Scalar));
            if all_scalar {
                ty.insert(*dest, ValueTy::Scalar);
                changed = true;
            }
        }

        // Phi 折叠：若 Phi 所有入边的源值当前都是 Scalar，则 Phi dest 降为 Scalar。
        for (dest, sources) in &phis {
            if ty.get(dest) != Some(&ValueTy::Handle) {
                continue;
            }
            if sources.is_empty() {
                continue;
            }
            let all_scalar = sources.iter().all(|s| ty.get(s) == Some(&ValueTy::Scalar));
            if all_scalar {
                ty.insert(*dest, ValueTy::Scalar);
                changed = true;
            }
        }
    }

    // ── 阶段 3：折叠每个变量的类型（按其全部 StoreVar 源）──
    let mut var_ty: HashMap<String, ValueTy> = HashMap::new();
    for (name, sources) in &var_stores {
        let all_scalar =
            !sources.is_empty() && sources.iter().all(|s| ty.get(s) == Some(&ValueTy::Scalar));
        var_ty.insert(
            name.clone(),
            if all_scalar {
                ValueTy::Scalar
            } else {
                ValueTy::Handle
            },
        );
    }

    (ty, var_ty)
}

/// 该 builtin 的返回值是否**规范保证**总是标量（number/bool/undefined）。
///
/// 列入此白名单的前提（逐个审计 host 实现确认）：
/// ①返回值类型静态确定为 number/bool/undefined（非 string/object/array/handle）；
/// ②host 实现不分配堆对象（无 alloc_host_object/obj_new/arr_new/GC safepoint poll）；
/// ③host 实现不 reentrant 回用户 JS（无 native_call/ProxyTrap/callback 调用）。
///
/// **GC 正确性红线**：误判（把会分配/reentrant 的 builtin 当标量）→ GC 漏 root
/// → 悬垂指针 → 内存损坏。有疑问的 builtin 一律不入白名单（保守 Handle）。
///
/// 特别剔除说明（曾误列入候选，但审计发现不安全）：
/// - ArraySome/Every/Find/FindIndex/ForEach/Map/Filter/Reduce/ReduceRight/FlatMap：
///   host 经 `wrap_array_callback_async!` 宏调用用户 callback → reentrant。
/// - StringFromCharCode/FromCodePoint/At/Concat/Slice/...：返回 runtime string handle。
///
/// 此函数同时被 Layer 1（value_ty 类型推断）和 Layer 3（GC 分析 may-trigger-gc 判定）
/// 使用。Layer 3 的 `builtin_may_trigger_gc` 为本函数的逻辑补集（`!builtin_returns_scalar`）。
pub fn builtin_returns_scalar(b: &Builtin) -> bool {
    use Builtin::*;
    matches!(
        b,
        // ── 纯数值运算（WASM host 辅助，f64 算术）──
        F64Mod | F64Exp
        // ── Math.*：全部纯 f64 运算，host 实现无 alloc/reentrant ──
        // 审计：math_number_error.rs 中 Math 函数体均为 value_to_number→数学→encode_f64，
        //       文件内 alloc 仅在 Error 构造器/native_callable（独立 builtin）。
        | MathAbs | MathAcos | MathAcosh | MathAsin | MathAsinh | MathAtan | MathAtanh
        | MathAtan2 | MathCbrt | MathCeil | MathClz32 | MathCos | MathCosh | MathExp
        | MathExpm1 | MathFloor | MathFround | MathHypot | MathImul | MathLog | MathLog1p
        | MathLog10 | MathLog2 | MathMax | MathMin | MathPow | MathRandom | MathRound
        | MathSign | MathSin | MathSinh | MathSqrt | MathTan | MathTanh | MathTrunc
        // ── Number：构造/转换/parse 均返回 number ──
        | NumberConstructor | NumberParseInt | NumberParseFloat
        | NumberIsNaN | NumberIsFinite | NumberIsInteger | NumberIsSafeInteger
        | NumberProtoValueOf
        // ── Boolean：构造/valueOf 返回 bool ──
        | BooleanConstructor | BooleanProtoValueOf
        // ── Array 纯查询（无 callback，纯内存扫描/tag 检查）──
        // 审计：array_object.rs 中 includes/index_of/is_array 为 Func::wrap 纯扫描；
        //       get_length 在 timers_arrays.rs 纯读长度。均无 alloc/reentrant。
        // 注意：some/every/find/find_index/for_each/map/filter/reduce 等带 callback 的
        //       方法经 reentrant_async.rs 的宏调用用户 JS → 不在此列。
        | ArrayIsArray | ArrayIncludes | ArrayIndexOf | ArrayGetLength
        // ── String 纯查询（返回索引/码点/bool）──
        // 审计：string_methods.rs 整文件零 alloc；这些函数返回 number/bool。
        //       注意：slice/concat/replace/trim/at 等返回 runtime string handle → 不在此列。
        | StringCharCodeAt | StringCodePointAt | StringIndexOf | StringLastIndexOf
        | StringIncludes | StringStartsWith | StringEndsWith
        // ── BigInt 比较（返回 bool）──
        | BigIntEq | BigIntCmp
        // ── Map/Set 查询（返回 bool/number）──
        // 审计：has/delete 返回 bool；size 返回 number。
        | MapSetHas | MapSetDelete | MapSetGetSize
        // ── Date 静态方法（返回 number）──
        | DateNow | DateParse | DateUTC
        // ── TypedArray 长度/索引查询（返回 number）──
        | TypedArrayProtoLength | TypedArrayProtoByteLength | TypedArrayProtoByteOffset
        | TypedArrayProtoIndexOf | TypedArrayProtoLastIndexOf | TypedArrayProtoIncludes
        // ── SharedArrayBuffer 长度查询（返回 number/bool）──
        | SharedArrayBufferProtoByteLength | SharedArrayBufferProtoMaxByteLength
        | SharedArrayBufferProtoGrowable
        // ── 迭代器完成态查询（返回 bool）──
        | IteratorDone | EnumeratorDone
        // ── 类型/存在性判断（返回 bool）──
        | IsCallable | IsJsObject | IsPromise | ObjectIs
        // ── 运算符（返回 bool）──
        | In | InstanceOf | AtomicsIsLockFree
        // ── RegExp.test（返回 bool）──
        // 审计：regexp host 实现返回 encode_bool(test 结果)，不分配 match 数组
        //       （分配 match 数组的是 RegExpExec，不在此列）。
        | RegExpTest
        // ── Reflect 查询型（返回 bool）──
        // 注意：ReflectHas/DeleteProperty/IsExtensible/PreventExtensions 返回 bool；
        //       但 ReflectGet/OwnKeys 等返回值/数组 → 不在此列。
        | ReflectHas | ReflectDeleteProperty | ReflectIsExtensible | ReflectPreventExtensions
    )
}

/// 返回 producing instruction 的 `(dest, ValueTy)`；非 producing 返回 None。
///
/// 这是**初始化遍**的分类——仅做单指令的确定性判断。LoadVar/Phi 的精确化
/// 由 `infer_value_ty` 的固定点迭代阶段处理（它们在此初始化为 Handle）。
fn dest_and_kind(ins: &Instruction, constants: &[Constant]) -> Option<(ValueId, ValueTy)> {
    use Instruction::*;
    Some(match ins {
        // ── 确定 Handle ──
        NewObject { dest, .. }
        | NewArray { dest, .. }
        | GetSuperBase { dest }
        | GetSuperConstructor { dest }
        | ObjectSpread { dest, .. }
        | NewPromise { dest }
        | ExceptionToObject { dest, .. } => (*dest, ValueTy::Handle),

        // ── Const：看 Constant variant ──
        Const { dest, constant } => {
            let kind = match constants.get(constant.0 as usize) {
                Some(Constant::Number(_))
                | Some(Constant::Bool(_))
                | Some(Constant::Null)
                | Some(Constant::Undefined) => ValueTy::Scalar,
                // String / FunctionRef / NativeCallableEval / BigInt / RegExp / ModuleId
                // 都涉及 handle 或运行时对象 -> Handle
                _ => ValueTy::Handle,
            };
            (*dest, kind)
        }

        // ── 算术 / 位运算 -> number (Scalar) ──
        Binary { dest, .. } => (*dest, ValueTy::Scalar),

        // ── 一元：全部产标量 ──
        //   Not/Neg/Pos/BitNot -> number
        //   Void -> undefined
        //   IsNullish -> bool
        Unary { dest, .. } => (*dest, ValueTy::Scalar),

        // ── 比较 -> bool (Scalar) ──
        Compare { dest, .. } => (*dest, ValueTy::Scalar),

        // ── DeleteProp -> bool (Scalar) ──
        // 规范保证 delete 返回 true/false（ECMA 262 §13.5.1.2）。
        // $obj_delete 是纯内存操作，不触发 GC。曾误判 Handle（注释自承 bug），现修正。
        DeleteProp { dest, .. } => (*dest, ValueTy::Scalar),

        // ── IsException -> bool (Scalar) ──
        // 检查值是否携带 TAG_EXCEPTION，结果恒为 encode_bool(true/false)。
        // 纯 tag 检查，不触发 GC。曾误判 Handle，现修正。
        IsException { dest, .. } => (*dest, ValueTy::Scalar),

        // ── EncodeException -> Handle（保持保守）──
        // 结果 = BOX_BASE | (TAG_EXCEPTION<<32) | 对象handle。
        // TAG_EXCEPTION 在 tag_needs_root 中为 true，且 low32 携带真实对象 handle，
        // 省 spill 会导致 GC 漏 root。故保持 Handle（修正 report.md 的风险判断）。
        EncodeException { dest, .. } => (*dest, ValueTy::Handle),

        // ── polymorphic -> Handle 保守（固定点迭代阶段可能降级）──
        //   GetProp/GetElem/Optional*：属性值类型运行时依赖。
        //   LoadVar：变量类型不定（迭代阶段按 StoreVar 源传播）。
        //   Phi：合并多路值（迭代阶段按入边折叠）。
        //   StringConcatVa：产 runtime string handle。
        //   CollectRestArgs：产 array handle。
        GetProp { dest, .. }
        | GetElem { dest, .. }
        | OptionalGetProp { dest, .. }
        | OptionalGetElem { dest, .. }
        | OptionalCall { dest, .. }
        | LoadVar { dest, .. }
        | CollectRestArgs { dest, .. }
        | Phi { dest, .. }
        | StringConcatVa { dest, .. } => (*dest, ValueTy::Handle),

        // ── CallBuiltin：按白名单判定 ──
        // builtin_returns_scalar 命中 → Scalar（规范保证返回标量且 host 不分配/reentrant）；
        // 否则 → Handle（保守）。
        CallBuiltin { dest, builtin, .. } => {
            let kind = if builtin_returns_scalar(builtin) {
                ValueTy::Scalar
            } else {
                ValueTy::Handle
            };
            ((*dest)?, kind)
        }

        // ── Call / SuperCall（用户函数）：返回值类型不定 -> Handle ──
        // 层 3 的 callee no-GC 分析可进一步省掉 Call 的 safepoint spill，
        // 但那是基于 callee "是否触发 GC" 的分析，与 "返回值是否标量" 是不同维度。
        // 返回值类型仍取决于被调函数实际 return 什么，保守 Handle。
        Call { dest, .. } | SuperCall { dest, .. } | ConstructCall { dest, .. } => {
            ((*dest)?, ValueTy::Handle)
        }

        // ── 非 producing（无 dest 或 void）──
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
