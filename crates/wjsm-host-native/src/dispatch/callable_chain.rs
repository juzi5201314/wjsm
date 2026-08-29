//! callable 接收者的原型链属性解析：OrdinaryGet / OrdinarySet / HasProperty
//! 共享的链行走底座。
//!
//! callable（闭包 / bound / native callable）的自有属性存宿主侧表
//! （`callable_properties` / `callable_accessors` / `callable_property_flags`），
//! [[Prototype]] 存 `callable_prototypes`；表中无条目表示隐式
//! %Function.prototype%，其内建成员由 `primitive_property` 惰性合成。本模块
//! 沿链找到最近一级自有属性，由调用方按 Get / Set / Has 语义处置。

use wjsm_ir::{constants, value};
use wjsm_native_abi::NativeVmContext;

use super::property_write::{SetCompletion, SetFailure, SetResult};
use super::runtime::{
    assign_data_property_to_receiver, encoded_property_key, fail_dispatch, ordinary_set_key,
};
use crate::{NativeAgentState, PropertyKey};

/// callable 原型链上属性 `key` 的最近命中。
pub(super) enum CallableChainHit {
    /// 某层自有访问器属性（getter / setter 可能为非 callable 占位）。
    Accessor { getter: i64, setter: i64 },
    /// 某层自有数据属性；`writable` 取该层 flags，无 flags 条目视为可写。
    Data { stored: i64, writable: bool },
    /// 链上出现非 callable 原型（堆对象 / proxy 等），沿对象语义继续。
    Object { prototype: i64 },
    /// 显式 null 原型：链终止，属性缺失。
    Null,
    /// 链尾 callable 无显式原型：隐式 Function.prototype。内建成员（含
    /// extends 内建构造器时的静态成员）由 `primitive_property(tail)` 合成。
    Implicit { tail: i64 },
}

/// 沿 callable 原型链逐层查找：先查该层自有访问器，再查自有数据（含
/// name / length / prototype 的惰性物化），未命中经 `callable_prototypes`
/// 上行。链无环由 `set_prototype` 的环检测保证。
pub(super) fn resolve(
    state: &mut NativeAgentState,
    callable: i64,
    key: PropertyKey,
) -> CallableChainHit {
    let mut current = value::strip_gc_color(callable);
    loop {
        if let Some((getter, setter)) = state.callable_accessors.get(&(current, key)).copied() {
            return CallableChainHit::Accessor { getter, setter };
        }
        if let Some(stored) = state.callable_property(current, key) {
            let writable = state
                .callable_property_flags
                .get(&(current, key))
                .is_none_or(|flags| flags & constants::FLAG_WRITABLE as u32 != 0);
            return CallableChainHit::Data { stored, writable };
        }
        match state.callable_prototypes.get(&current).copied() {
            None => return CallableChainHit::Implicit { tail: current },
            Some(prototype) if value::is_null(prototype) => return CallableChainHit::Null,
            Some(prototype) if value::is_callable(prototype) => {
                current = value::strip_gc_color(prototype);
            }
            Some(prototype) => return CallableChainHit::Object { prototype },
        }
    }
}

/// callable 目标的 [[Set]]（OrdinarySetWithOwnDescriptor）：链上最近命中决定
/// 调 setter、按可写性拒绝、或在 receiver 上建自有数据属性。写失败返回
/// `Ok(Failed(..))` 携带规范原因（strict 抛错与否由调用方决定），Err 为异常值。
///
/// 隐式 Function.prototype 的内建成员均为可写数据属性，链尾未命中与显式
/// null 同样落在 receiver 上建自有属性（primitive_property 合成的属性无
/// flags 模型，如 extends 内建构造器时其不可写静态成员无法拒绝写入）。
pub(super) fn set_with_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: PropertyKey,
    stored: i64,
    receiver: i64,
) -> SetResult {
    match resolve(state, target, key) {
        CallableChainHit::Accessor { setter, .. } => {
            if !value::is_callable(setter) {
                return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
            }
            let result = state
                .invoke_callable(ctx, setter, receiver, &[stored])
                .ok_or_else(|| fail_dispatch(ctx))?;
            if value::is_exception(result) {
                Err(result)
            } else {
                Ok(SetCompletion::Written)
            }
        }
        CallableChainHit::Data { writable, .. } => {
            if !writable {
                return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
            }
            assign_data_property_to_receiver(ctx, state, receiver, key, stored)
        }
        CallableChainHit::Object { prototype } => {
            if value::is_proxy(prototype) {
                return super::proxy::set(
                    ctx,
                    state,
                    prototype,
                    encoded_property_key(key),
                    stored,
                    receiver,
                );
            }
            ordinary_set_key(ctx, state, prototype, key, stored, receiver)
        }
        CallableChainHit::Implicit { tail } => {
            // 隐式父层 %Function.prototype% 的自有 name/length 不可写
            // （§20.2.3）：own 层删除（墓碑）后写入按继承数据属性语义在
            // 原型层拒绝，而不是在 receiver 上重建自有属性。
            if (state.text_matches(key.to_value(), "name")
                || state.text_matches(key.to_value(), "length"))
                && let Some(prototype) =
                    state.native_callable(crate::NativeCallableKind::FunctionPrototype)
                && value::strip_gc_color(prototype) != tail
            {
                return set_with_receiver(ctx, state, prototype, key, stored, receiver);
            }
            assign_data_property_to_receiver(ctx, state, receiver, key, stored)
        }
        CallableChainHit::Null => {
            assign_data_property_to_receiver(ctx, state, receiver, key, stored)
        }
    }
}
