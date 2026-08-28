//! Array.prototype 迭代方法族（map / forEach / filter / find* / some /
//! every / flatMap / reduce / reduceRight / sort / toSorted）的接收者抽象：
//! 真数组的自有非洞元素沿用堆槽直读快路径，洞、越界与字典 kind 退回完整
//! HasProperty / Get 协议（原型链继承索引、索引 accessor 与迭代中原型变更
//! 均可观察）；其余 array-like（arguments 对象、`{length, 索引}` 普通对象、
//! 盒装原语、Proxy 等）按规范 generic 语义（ToObject → LengthOfArrayLike →
//! HasProperty / Get / Set / DeletePropertyOrThrow）逐键访问，禁止落入
//! InternalInvariant。

use wjsm_host::RuntimeString;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::property_write::{SetCompletion, SetFailure, strict_set_failure_error};
use super::runtime::{
    delete_property, fail_dispatch, get_property, has_property, intern_string_with_gc_retry,
    primitive_string, set_element_completion, to_number_coerced, type_error,
};
use crate::NativeAgentState;

/// LengthOfArrayLike 的上界（§7.1.20 ToLength：2^53 − 1）。
const MAX_LENGTH: f64 = 9_007_199_254_740_991.0;

/// 迭代方法的元素访问源：快路径直读堆元素，generic 路径按属性协议访问。
#[derive(Clone, Copy)]
pub(super) enum ArrayLikeSource {
    /// 真数组接收者：`encoded` 为原始编码值（回传给回调第三实参）。
    Fast {
        encoded: i64,
        handle: u32,
        length: u32,
    },
    /// generic array-like：`object` 为 ToObject 后的对象，`length` 为
    /// LengthOfArrayLike 结果。
    Generic { object: i64, length: u64 },
}

impl ArrayLikeSource {
    /// 按 §23.1.3 各方法开头的 `ToObject(this)` + `LengthOfArrayLike(O)`
    /// 解析接收者。`method` 为 `Some(name)` 时 null/undefined 用 V8 的
    /// `Array.prototype.<name> called on null or undefined` 文案，`None`
    /// （sort / toSorted / flatMap）用 ToObject 通用文案。
    ///
    /// generic 分支把装箱对象压入 `temporary_roots`（读 length 可再入
    /// getter 触发 GC），调用方须在进入前记录根栈水位并在收尾统一截断。
    pub(super) fn resolve(
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        receiver: i64,
        method: Option<&str>,
    ) -> Result<Self, i64> {
        if value::is_array(receiver) {
            let handle = value::decode_handle(receiver);
            let Ok(length) = state.gc.heap().array_length(handle) else {
                return Err(fail_dispatch(ctx));
            };
            return Ok(Self::Fast {
                encoded: receiver,
                handle,
                length,
            });
        }
        if value::is_null(receiver) || value::is_undefined(receiver) {
            let message = match method {
                Some(name) => format!("Array.prototype.{name} called on null or undefined"),
                None => "Cannot convert undefined or null to object".to_owned(),
            };
            return Err(type_error(ctx, state, &message));
        }
        let object = super::with_env::to_object(ctx, state, receiver);
        if value::is_exception(object) {
            return Err(object);
        }
        state.temporary_roots.push(object);
        let length_key = intern_string_with_gc_retry(ctx, state, "length".into());
        if value::is_exception(length_key) {
            return Err(length_key);
        }
        let encoded_length =
            get_property(ctx, state, object, length_key).map_err(|()| fail_dispatch(ctx))?;
        if value::is_exception(encoded_length) {
            return Err(encoded_length);
        }
        let length = to_length(to_number_coerced(ctx, state, encoded_length)?);
        Ok(Self::Generic { object, length })
    }

    /// 回调第三实参 / sort 返回值使用的接收者对象（generic 为 ToObject 结果）。
    pub(super) fn receiver(&self) -> i64 {
        match self {
            Self::Fast { encoded, .. } => *encoded,
            Self::Generic { object, .. } => *object,
        }
    }

    pub(super) fn length(&self) -> u64 {
        match self {
            Self::Fast { length, .. } => u64::from(*length),
            Self::Generic { length, .. } => *length,
        }
    }

    /// filter / flatMap 结果数组的预分配容量提示：快路径沿用源长度；
    /// generic 长度可达 2^53 − 1，夹紧到小额提示避免病态 length 触发巨量
    /// 预分配（规范 ArraySpeciesCreate(O, 0) 本就不预分配）。
    pub(super) fn allocation_hint(&self) -> u32 {
        match self {
            Self::Fast { length, .. } => *length,
            Self::Generic { length, .. } => (*length).min(4096) as u32,
        }
    }

    /// `HasProperty(O, ToString(index))`（§7.3.11）：快路径自有非洞元素直判
    /// 存在，其余退回完整协议（原型链继承索引可观察）。
    pub(super) fn has(
        &self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        index: u64,
    ) -> Result<bool, i64> {
        match self {
            Self::Fast { encoded, .. } => element_has(ctx, state, *encoded, index),
            Self::Generic { object, .. } => {
                let key = index_key(ctx, state, index)?;
                has_property(ctx, state, *object, key)
            }
        }
    }

    /// `Get(O, ToString(index))`（§7.3.2）：快路径自有非洞元素直读，其余
    /// 退回完整属性协议（原型链 getter / Proxy trap 异常原样传播）。
    pub(super) fn get(
        &self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        index: u64,
    ) -> Result<i64, i64> {
        match self {
            Self::Fast { encoded, .. } => element_get(ctx, state, *encoded, index),
            Self::Generic { object, .. } => {
                let key = index_key(ctx, state, index)?;
                let result =
                    get_property(ctx, state, *object, key).map_err(|()| fail_dispatch(ctx))?;
                if value::is_exception(result) {
                    return Err(result);
                }
                Ok(result)
            }
        }
    }
}

/// 真数组索引访问能否信任元素槽直读：PACKED / HOLEY 的非洞槽即自有数据
/// 属性；DICTIONARY 表示索引位置存在 accessor 等异质属性（槽值可能陈旧、
/// 被侧表遮蔽），必须走完整属性协议。逐次访问重读 kind，回调在迭代中把
/// 数组升为字典（defineProperty 装 getter）时立即可观察。
pub(super) fn element_slots_trusted(state: &NativeAgentState, handle: u32) -> bool {
    state.gc.heap().array_kind(handle).ok() != Some(wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
}

/// 真数组的 `HasProperty(O, ToString(index))`（§7.3.11）：可信槽内非洞元素
/// 直判存在；洞、越界与字典 kind 退回 `has_property`（侧表 accessor 与
/// 原型链上的继承索引可观察，Proxy 原型 trap 异常原样传播）。
pub(super) fn element_has(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
    index: u64,
) -> Result<bool, i64> {
    let handle = value::decode_handle(array);
    if element_slots_trusted(state, handle)
        && u32::try_from(index)
            .ok()
            .and_then(|index| raw(state, handle, index))
            .is_some_and(|stored| !value::is_array_hole(stored))
    {
        return Ok(true);
    }
    let key = index_key(ctx, state, index)?;
    has_property(ctx, state, array, key)
}

/// 真数组的 `Get(O, ToString(index))`（§7.3.2）：可信槽内非洞元素直读；
/// 洞、越界与字典 kind 退回 `get_property`（原型链 getter 以接收者为 this
/// 调用，异常原样传播）。
pub(super) fn element_get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
    index: u64,
) -> Result<i64, i64> {
    let handle = value::decode_handle(array);
    if element_slots_trusted(state, handle)
        && let Some(stored) = u32::try_from(index)
            .ok()
            .and_then(|index| raw(state, handle, index))
        && !value::is_array_hole(stored)
    {
        return Ok(stored);
    }
    let key = index_key(ctx, state, index)?;
    let result = get_property(ctx, state, array, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(result)
}

/// generic sort 写回的 `Set(O, ToString(index), value, true)`（§23.1.3.30
/// 步骤 7）：写失败按 throw=true 升级 TypeError（文案与赋值 strict 路径同源）。
/// String exotic 在界索引不可写（§10.4.3.2），ordinary 写路径不建模
/// [[StringData]]，此处先行按 ReadOnly 拒绝。
pub(super) fn set_index_or_throw(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    index: u64,
    stored: i64,
) -> Result<(), i64> {
    let key = index_key(ctx, state, index)?;
    if let Some(text) = primitive_string(state, object)
        && state
            .string_len(text)
            .is_some_and(|length| index < length as u64)
    {
        return Err(strict_set_failure_error(
            ctx,
            state,
            object,
            key,
            SetFailure::ReadOnly,
        ));
    }
    match set_element_completion(ctx, state, object, key, stored) {
        Err(exception) => Err(exception),
        Ok(SetCompletion::Written) => Ok(()),
        Ok(SetCompletion::Failed(failure)) => {
            Err(strict_set_failure_error(ctx, state, object, key, failure))
        }
    }
}

/// generic sort 收尾的 `DeletePropertyOrThrow(O, ToString(index))`
/// （§23.1.3.30 步骤 9）：Proxy 走 deleteProperty trap，其余走宿主
/// [[Delete]]；删除失败升级 TypeError。
pub(super) fn delete_index_or_throw(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    index: u64,
) -> Result<(), i64> {
    let key = index_key(ctx, state, index)?;
    let deleted = if value::is_proxy(object) {
        let result = super::proxy::dispatch_proxy(
            ctx,
            state,
            Builtin::ReflectDeleteProperty,
            &[object, key],
        )
        .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(result) {
            return Err(result);
        }
        super::runtime::is_truthy(state, result)
    } else {
        delete_property(state, object, key).map_err(|()| fail_dispatch(ctx))?
    };
    if deleted {
        Ok(())
    } else {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        Err(strict_set_failure_error(
            ctx,
            state,
            object,
            key,
            SetFailure::NonDeletableElement(index),
        ))
    }
}

/// 真数组元素的裸读取：越界 / 访问失败为 None（含洞哨兵原样返回）。
fn raw(state: &NativeAgentState, handle: u32, index: u32) -> Option<i64> {
    state
        .gc
        .heap()
        .get_element(handle, index)
        .ok()
        .flatten()
        .map(|stored| stored as i64)
}

/// ToLength（§7.1.20）：ToIntegerOrInfinity 后夹紧到 [0, 2^53 − 1]。
fn to_length(number: f64) -> u64 {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }
    number.trunc().min(MAX_LENGTH) as u64
}

/// 十进制索引串的属性键驻留（与 arguments / 对象属性创建同一驻留形态）。
fn index_key(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    index: u64,
) -> Result<i64, i64> {
    let key = intern_string_with_gc_retry(ctx, state, RuntimeString::from(index.to_string()));
    if value::is_exception(key) {
        return Err(key);
    }
    Ok(key)
}
