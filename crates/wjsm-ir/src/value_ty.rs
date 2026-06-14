//! Per-ValueId 类型推断：区分 Handle（需 GC root）与 Scalar。
//!
//! 供 GC safepoint spill 用（spec §11.2）。polymorphic ops（GetProp/GetElem/
//! Call/Phi 等）的结果类型静态不定，保守判 Handle。
use crate::{Constant, Function, Instruction, Module, ValueId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueTy {
    /// 持有 GC handle（对象/数组/闭包/...）。spill 时需暴露给 shadow stack 扫描。
    Handle,
    /// 纯标量（number/bool/undefined/null）。spill 时跳过。
    Scalar,
}

/// 推断函数内每个 producing instruction 的 dest 类型。
///
/// 返回 `HashMap<ValueId, ValueTy>`。未出现在 map 中的 ValueId（理论上不应发生）
/// 在调用方保守视为 Handle。
pub fn infer_value_ty(module: &Module, function: &Function) -> HashMap<ValueId, ValueTy> {
    let mut ty: HashMap<ValueId, ValueTy> = HashMap::new();
    let constants = module.constants();
    for bb in function.blocks() {
        for ins in bb.instructions() {
            if let Some((dest, kind)) = dest_and_kind(ins, constants) {
                ty.insert(dest, kind);
            }
        }
    }
    ty
}

/// 返回 producing instruction 的 `(dest, ValueTy)`；非 producing 返回 None。
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
                Some(Constant::Number(_)) | Some(Constant::Bool(_))
                | Some(Constant::Null) | Some(Constant::Undefined) => ValueTy::Scalar,
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
        //   IsNullish/Delete -> bool
        Unary { dest, .. } => (*dest, ValueTy::Scalar),

        // ── 比较 -> bool (Scalar) ──
        Compare { dest, .. } => (*dest, ValueTy::Scalar),

        // ── polymorphic -> Handle 保守 ──
        //   GetProp/GetElem 可能返回对象或标量
        //   LoadVar 加载的变量类型不定
        //   Phi 合并多路值
        //   StringConcatVa 产 runtime string handle
        //   DeleteProp -> bool? 实际语义返回 bool，但保守 Handle 无害（多 spill 不会漏）
        //     注：DeleteProp 返回 bool 可判 Scalar；此处保守 Handle 以求简单一致。
        //     如果未来 profile 显示它是热点，可精确化。
        GetProp { dest, .. }
        | GetElem { dest, .. }
        | OptionalGetProp { dest, .. }
        | OptionalGetElem { dest, .. }
        | OptionalCall { dest, .. }
        | DeleteProp { dest, .. }
        | LoadVar { dest, .. }
        | CollectRestArgs { dest, .. }
        | IsException { dest, .. }
        | EncodeException { dest, .. }
        | Phi { dest, .. }
        | StringConcatVa { dest, .. } => (*dest, ValueTy::Handle),

        // ── 条件 dest（Option<ValueId>）──
        Call { dest, .. } | CallBuiltin { dest, .. } | SuperCall { dest, .. } => {
            // dest=Some 时结果类型不定 -> Handle；dest=None 无值
            match dest {
                Some(d) => (*d, ValueTy::Handle),
                None => return None,
            }
        }

        // ── 非 producing（无 dest 或 void）──
        StoreVar { .. }
        | SetProp { .. }
        | SetProto { .. }
        | SetElem { .. }
        | PromiseResolve { .. }
        | PromiseReject { .. }
        | Suspend { .. }
        | ConstructCall { .. } => return None,
    })
}
