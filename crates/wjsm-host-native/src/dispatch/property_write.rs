//! [[Set]] 完成结果与 PutValue 严格模式失败策略。
//!
//! OrdinarySet / proxy [[Set]] / callable 链 [[Set]] 统一返回
//! [`SetCompletion`]：成功写入或携带规范失败原因。赋值点（SetProp /
//! SetElem / SetPropIc 及其 strict 变体）经 [`finish_property_set`] 收口：
//! sloppy 静默返回赋值值（PutValue 步骤 6.b），strict 升级 TypeError
//! （步骤 6.c），消息按失败原因取 V8 口径。Reflect.set 与 Object.assign
//! 等非赋值入口只关心成败布尔，不受本策略影响。

use wjsm_ir::{constants, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, render_value, type_error};
use crate::{ASSIGNED_PROPERTY_FLAGS, NativeAgentState, PropertyKey};

/// [[Set]] 的完成结果：`Err` 为已构造的异常值（含 setter/trap 抛出）。
pub(super) type SetResult = Result<SetCompletion, i64>;

/// [[Set]] 返回 true / false 的结构化表示。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SetCompletion {
    /// 写入成功（[[Set]] 返回 true）。
    Written,
    /// 写入失败（[[Set]] 返回 false），携带规范上的失败原因。
    Failed(SetFailure),
}

/// [[Set]] 返回 false 的原因，决定 strict 升级 TypeError 时的消息。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SetFailure {
    /// 命中不可写数据属性（自有或原型链上）。
    ReadOnly,
    /// 命中无 setter 的访问器属性。
    GetterOnly,
    /// 属性不存在且 receiver 不可扩展。
    NotExtensible,
    /// proxy 写路径的 trap（set / defineProperty）返回 falsish。
    ProxyFalsish,
    /// receiver 无法承载数据属性（OrdinarySetWithOwnDescriptor 步骤
    /// 3.d.iv：Receiver 为基元等非对象值）。
    Receiver,
}

impl SetCompletion {
    pub(super) fn succeeded(self) -> bool {
        matches!(self, Self::Written)
    }
}

/// PutValue 步骤 6.b–6.c：写失败时 sloppy 静默返回赋值值，strict 抛
/// TypeError。`key` 为编码后的属性键（消息渲染用），`receiver` 为赋值基座。
pub(super) fn finish_property_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: i64,
    stored: i64,
    strict: bool,
    completion: SetResult,
) -> i64 {
    match completion {
        Err(exception) => exception,
        Ok(SetCompletion::Written) => stored,
        Ok(SetCompletion::Failed(_)) if !strict => stored,
        Ok(SetCompletion::Failed(failure)) => {
            strict_set_failure_error(ctx, state, receiver, key, failure)
        }
    }
}

/// strict 写失败的 TypeError，消息与 V8 对齐（Node 同口径）。
pub(super) fn strict_set_failure_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: i64,
    failure: SetFailure,
) -> i64 {
    let key_text = render_value(state, key);
    let message = match failure {
        SetFailure::ReadOnly => format!(
            "Cannot assign to read only property '{key_text}' of {}",
            render_receiver_owner(state, receiver)
        ),
        SetFailure::GetterOnly => format!(
            "Cannot set property {key_text} of {} which has only a getter",
            render_receiver_brief(state, receiver)
        ),
        SetFailure::NotExtensible => {
            format!("Cannot add property {key_text}, object is not extensible")
        }
        SetFailure::ProxyFalsish => {
            format!("'set' on proxy: trap returned falsish for property '{key_text}'")
        }
        SetFailure::Receiver => format!(
            "Cannot create property '{key_text}' on {}",
            render_receiver_brief(state, receiver)
        ),
    };
    type_error(ctx, state, &message)
}

/// ReadOnly 消息中的属主渲染：`object '#<Object>'`、`object '[object
/// Array]'`；callable 按引擎统一的函数 toString 文本。
fn render_receiver_owner(state: &NativeAgentState, receiver: i64) -> String {
    if value::is_callable(receiver) {
        format!("function '{}'", callable_source_text(state, receiver))
    } else {
        format!("object '{}'", render_receiver_brief(state, receiver))
    }
}

/// GetterOnly / Receiver 消息中的简短渲染：数组 → `[object Array]`，
/// callable → 函数 toString 文本，其余对象（含 proxy）→ `#<Object>`。
fn render_receiver_brief(state: &NativeAgentState, receiver: i64) -> String {
    if value::is_array(receiver) {
        "[object Array]".into()
    } else if value::is_callable(receiver) {
        callable_source_text(state, receiver)
    } else {
        "#<Object>".into()
    }
}

/// 引擎对 callable 的统一 toString 表示（与 Function.prototype.toString
/// 的 native-code 形态一致；不追踪源码文本）。
fn callable_source_text(_state: &NativeAgentState, _receiver: i64) -> String {
    "function() { [native code] }".into()
}

/// 数组接收者的命名（非下标）属性 [[Set]]：自有访问器（含无 setter 拒绝）、
/// 自有数据属性可写性、不可扩展拒绝新属性，其余按缺省特性写入宿主侧表。
pub(super) fn set_array_named_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
    key: PropertyKey,
    stored: i64,
) -> SetResult {
    let handle = value::decode_handle(array);
    if let Some((_, setter, _)) = state.array_accessors.get(&(handle, key)).copied() {
        if !value::is_callable(setter) {
            return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
        }
        let result = state
            .invoke_callable(ctx, setter, array, &[stored])
            .ok_or_else(|| fail_dispatch(ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        return Ok(SetCompletion::Written);
    }
    if state.array_properties.contains_key(&(handle, key)) {
        if state
            .array_property_flags
            .get(&(handle, key))
            .is_some_and(|flags| flags & constants::FLAG_WRITABLE as u32 == 0)
        {
            return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
        }
        // 更新既有属性保留原特性（flags 不重置）。
        state.array_properties.insert((handle, key), stored);
        return Ok(SetCompletion::Written);
    }
    if state.non_extensible_objects.contains(&handle) {
        return Ok(SetCompletion::Failed(SetFailure::NotExtensible));
    }
    state.note_array_property(handle, key);
    state.array_properties.insert((handle, key), stored);
    state
        .array_property_flags
        .insert((handle, key), ASSIGNED_PROPERTY_FLAGS);
    Ok(SetCompletion::Written)
}
