//! `Set.prototype` 的集合运算方法（ES §24.2.4.5 difference / §24.2.4.9
//! intersection / §24.2.4.10 isSubsetOf / §24.2.4.11 isSupersetOf /
//! §24.2.4.12 isDisjointFrom / §24.2.4.15 symmetricDifference /
//! §24.2.4.16 union）。
//!
//! 参数侧按 GetSetRecord（§24.2.1.2）协议读取 size / has / keys（getter 与
//! 用户方法可重入执行 JS），接收者侧直接读宿主 `state.sets` 侧表；结果集
//! 按 OrdinaryObjectCreate(%Set.prototype%) 语义直接物化，不经构造器 /
//! @@species。错误文案对齐 Node v22（V8）。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::super::iterator_prototypes::render_incompatible_receiver;
use super::super::runtime::{
    fail_dispatch, get_property, is_truthy, range_error, render_value, to_number_coerced,
    type_error,
};
use super::{canonicalize_keyed_collection_key, collection_object, same_value_zero};
use crate::NativeAgentState;

/// 七个集合运算方法的宿主入口：按 builtin 选择算法，公共前置检查
/// （receiver brand + GetSetRecord）在各算法内完成。
pub(super) fn dispatch_set_ops(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::SetProtoUnion => set_union(ctx, state, args),
        Builtin::SetProtoIntersection => set_intersection(ctx, state, args),
        Builtin::SetProtoDifference => set_difference(ctx, state, args),
        Builtin::SetProtoSymmetricDifference => set_symmetric_difference(ctx, state, args),
        Builtin::SetProtoIsSubsetOf => set_is_subset_of(ctx, state, args),
        Builtin::SetProtoIsSupersetOf => set_is_superset_of(ctx, state, args),
        Builtin::SetProtoIsDisjointFrom => set_is_disjoint_from(ctx, state, args),
        _ => return None,
    })
}

/// GetSetRecord（§24.2.1.2）返回的 set-like 记录。
struct SetRecord {
    object: i64,
    /// ToIntegerOrInfinity 后的非负 size（可为 +∞）。
    size: f64,
    has: i64,
    keys: i64,
}

/// GetKeysIterator（§24.2.1.3）返回的迭代器记录：next 已按规范做过
/// 可调用性急检。
struct KeysIterator {
    iterator: i64,
    next: i64,
}

/// 公共前置：RequireInternalSlot(O, [[SetData]])（V8 incompatible receiver
/// 文案）+ GetSetRecord(other)。
fn set_op_prelude(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: &str,
    args: &[i64],
) -> Result<(u32, SetRecord), i64> {
    let receiver = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let handle = value::decode_handle(receiver);
    if !(value::is_js_object(receiver) && state.sets.contains_key(&handle)) {
        return Err(type_error(
            ctx,
            state,
            &format!(
                "Method Set.prototype.{method} called on incompatible receiver {}",
                render_incompatible_receiver(state, receiver)
            ),
        ));
    }
    let other = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let record = get_set_record(ctx, state, method, other)?;
    Ok((handle, record))
}

/// GetSetRecord（§24.2.1.2）：size 经 Get + ToNumber（NaN 抛 TypeError、
/// 负数抛 RangeError），has / keys 必须可调用（V8 按属性键渲染
/// `string "has" is not a function`）。
fn get_set_record(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: &str,
    other: i64,
) -> Result<SetRecord, i64> {
    if !value::is_js_object(other) {
        return Err(type_error(
            ctx,
            state,
            &format!("Set.prototype.{method} argument must be an object"),
        ));
    }
    let raw_size = get_named(ctx, state, other, "size")?;
    let num_size = to_number_coerced(ctx, state, raw_size)?;
    if num_size.is_nan() {
        return Err(type_error(ctx, state, "The .size property is NaN"));
    }
    let int_size = to_integer_or_infinity(num_size);
    if int_size < 0.0 {
        return Err(range_error(
            ctx,
            state,
            &format!(
                "'{}' is an invalid size",
                wjsm_builtins::number_format::format_number_js(int_size)
            ),
        ));
    }
    let has = get_named(ctx, state, other, "has")?;
    if !state.is_callable_value(has) {
        return Err(type_error(ctx, state, "string \"has\" is not a function"));
    }
    let keys = get_named(ctx, state, other, "keys")?;
    if !state.is_callable_value(keys) {
        return Err(type_error(ctx, state, "string \"keys\" is not a function"));
    }
    Ok(SetRecord {
        object: other,
        size: int_size,
        has,
        keys,
    })
}

/// GetKeysIterator（§24.2.1.3）：Call(keys) 结果必须为对象，next 必须
/// 可调用（急检，先于任何 next 调用）。
fn get_keys_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &SetRecord,
) -> Result<KeysIterator, i64> {
    let iterator = state
        .invoke_callable(ctx, record.keys, record.object, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(iterator) {
        return Err(iterator);
    }
    if !value::is_js_object(iterator) {
        return Err(type_error(
            ctx,
            state,
            "Result of the keys method is not an object",
        ));
    }
    let next = get_named(ctx, state, iterator, "next")?;
    if !state.is_callable_value(next) {
        return Err(type_error(ctx, state, "string \"next\" is not a function"));
    }
    Ok(KeysIterator { iterator, next })
}

/// IteratorStepValue（§7.4.8）：next() 结果必须为对象，done 为真返回
/// `None`，否则读出 value。
fn step_keys_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: &KeysIterator,
) -> Result<Option<i64>, i64> {
    let result = state
        .invoke_callable(ctx, iterator.next, iterator.iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        return Err(result);
    }
    if !value::is_js_object(result) {
        return Err(type_error(
            ctx,
            state,
            &format!(
                "Iterator result {} is not an object",
                render_value(state, result)
            ),
        ));
    }
    let done = get_named(ctx, state, result, "done")?;
    if is_truthy(state, done) {
        return Ok(None);
    }
    Ok(Some(get_named(ctx, state, result, "value")?))
}

/// isSupersetOf / isDisjointFrom 早退 false 前的 IteratorClose
/// （§7.4.6，normal completion）：return 缺失静默跳过，调用异常与
/// 非对象结果（V8 文案 `return called on non-object`）向外传播。
fn close_keys_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: &KeysIterator,
) -> Result<(), i64> {
    let method = get_named(ctx, state, iterator.iterator, "return")?;
    if value::is_undefined(method) || value::is_null(method) {
        return Ok(());
    }
    if !state.is_callable_value(method) {
        return Err(type_error(
            ctx,
            state,
            &format!(
                "{} is not a function",
                super::super::runtime::default_call_site(state, method)
            ),
        ));
    }
    let result = state
        .invoke_callable(ctx, method, iterator.iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        return Err(result);
    }
    if !value::is_js_object(result) {
        return Err(type_error(ctx, state, "return called on non-object"));
    }
    Ok(())
}

fn get_named(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<i64, i64> {
    let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    match get_property(ctx, state, object, key) {
        Ok(result) if value::is_exception(result) => Err(result),
        Ok(result) => Ok(result),
        Err(()) => Err(fail_dispatch(ctx)),
    }
}

/// ToIntegerOrInfinity（§7.1.5）：NaN 由调用方先行拒绝，±0 归 0，±∞
/// 保留，其余向零截断。
fn to_integer_or_infinity(number: f64) -> f64 {
    if number == 0.0 {
        0.0
    } else if number.is_infinite() {
        number
    } else {
        number.trunc()
    }
}

/// SetDataHas（§24.2.1.6）：SameValueZero 线性查找（宿主 Vec 模型无
/// EMPTY 墓碑）。
fn list_has(state: &NativeAgentState, values: &[i64], candidate: i64) -> bool {
    values
        .iter()
        .any(|stored| same_value_zero(state, *stored, candidate))
}

fn list_position(state: &NativeAgentState, values: &[i64], candidate: i64) -> Option<usize> {
    values
        .iter()
        .position(|stored| same_value_zero(state, *stored, candidate))
}

/// 接收者 [[SetData]] 的活视图 SetDataHas：用户回调可能已增删元素。
fn receiver_has(state: &NativeAgentState, handle: u32, candidate: i64) -> bool {
    state
        .sets
        .get(&handle)
        .is_some_and(|values| list_has(state, values, candidate))
}

/// ToBoolean(? Call(record.[[Has]], record.[[SetObject]], « value »))。
fn call_has(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &SetRecord,
    candidate: i64,
) -> Result<bool, i64> {
    let result = state
        .invoke_callable(ctx, record.has, record.object, &[candidate])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(is_truthy(state, result))
}

/// 以给定元素列表物化 %Set.prototype% 结果集（OrdinaryObjectCreate 语义）。
fn make_result_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    values: Vec<i64>,
) -> i64 {
    let Some(object) = collection_object(ctx, state, Builtin::SetConstructor) else {
        return fail_dispatch(ctx);
    };
    state.sets.insert(value::decode_handle(object), values);
    object
}

/// 接收者数据快照进 `temporary_roots`：用户回调 / 迭代器可能触发 GC 与
/// 对接收者的删除，副本中的值必须钉扎到结果集发布为止。
fn snapshot_receiver(state: &mut NativeAgentState, handle: u32) -> Vec<i64> {
    let values = state.sets.get(&handle).cloned().unwrap_or_default();
    state.temporary_roots.extend(values.iter().copied());
    values
}

/// `Set.prototype.union`（§24.2.4.16）。
fn set_union(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let roots_base = state.temporary_roots.len();
    let result = (|| -> Result<Vec<i64>, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "union", args)?;
        let iterator = get_keys_iterator(ctx, state, &record)?;
        let mut result = snapshot_receiver(state, handle);
        while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
            let next = canonicalize_keyed_collection_key(next);
            if !list_has(state, &result, next) {
                state.temporary_roots.push(next);
                result.push(next);
            }
        }
        Ok(result)
    })();
    let encoded = match result {
        Ok(values) => make_result_set(ctx, state, values),
        Err(exception) => exception,
    };
    state.temporary_roots.truncate(roots_base);
    encoded
}

/// `Set.prototype.intersection`（§24.2.4.9）：thisSize ≤ otherSize 时逐元素
/// 调 other.has（keys 不取用），否则消费 other 的 keys 迭代器。
fn set_intersection(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let roots_base = state.temporary_roots.len();
    let result = (|| -> Result<Vec<i64>, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "intersection", args)?;
        let this_size = state.sets.get(&handle).map_or(0, Vec::len);
        let mut result = Vec::new();
        if this_size as f64 <= record.size {
            let mut index = 0usize;
            loop {
                let Some(element) = state
                    .sets
                    .get(&handle)
                    .and_then(|values| values.get(index).copied())
                else {
                    break;
                };
                index += 1;
                if call_has(ctx, state, &record, element)? && !list_has(state, &result, element) {
                    state.temporary_roots.push(element);
                    result.push(element);
                }
            }
        } else {
            let iterator = get_keys_iterator(ctx, state, &record)?;
            while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
                let next = canonicalize_keyed_collection_key(next);
                if receiver_has(state, handle, next) && !list_has(state, &result, next) {
                    state.temporary_roots.push(next);
                    result.push(next);
                }
            }
        }
        Ok(result)
    })();
    let encoded = match result {
        Ok(values) => make_result_set(ctx, state, values),
        Err(exception) => exception,
    };
    state.temporary_roots.truncate(roots_base);
    encoded
}

/// `Set.prototype.difference`（§24.2.4.5）。
fn set_difference(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let roots_base = state.temporary_roots.len();
    let result = (|| -> Result<Vec<i64>, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "difference", args)?;
        let this_size = state.sets.get(&handle).map_or(0, Vec::len);
        let mut result = snapshot_receiver(state, handle);
        if this_size as f64 <= record.size {
            let mut index = 0usize;
            while index < result.len() {
                let element = result[index];
                if call_has(ctx, state, &record, element)? {
                    result.remove(index);
                } else {
                    index += 1;
                }
            }
        } else {
            let iterator = get_keys_iterator(ctx, state, &record)?;
            while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
                let next = canonicalize_keyed_collection_key(next);
                if let Some(position) = list_position(state, &result, next) {
                    result.remove(position);
                }
            }
        }
        Ok(result)
    })();
    let encoded = match result {
        Ok(values) => make_result_set(ctx, state, values),
        Err(exception) => exception,
    };
    state.temporary_roots.truncate(roots_base);
    encoded
}

/// `Set.prototype.symmetricDifference`（§24.2.4.15）：对 other 每个键，
/// 依接收者活数据判定去留（用户迭代器可能重入变更接收者）。
fn set_symmetric_difference(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let roots_base = state.temporary_roots.len();
    let result = (|| -> Result<Vec<i64>, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "symmetricDifference", args)?;
        let iterator = get_keys_iterator(ctx, state, &record)?;
        let mut result = snapshot_receiver(state, handle);
        while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
            let next = canonicalize_keyed_collection_key(next);
            let position = list_position(state, &result, next);
            if receiver_has(state, handle, next) {
                if let Some(position) = position {
                    result.remove(position);
                }
            } else if position.is_none() {
                state.temporary_roots.push(next);
                result.push(next);
            }
        }
        Ok(result)
    })();
    let encoded = match result {
        Ok(values) => make_result_set(ctx, state, values),
        Err(exception) => exception,
    };
    state.temporary_roots.truncate(roots_base);
    encoded
}

/// `Set.prototype.isSubsetOf`（§24.2.4.10）：thisSize > otherSize 直接
/// false；否则逐元素调 other.has，全部为真才是子集。
fn set_is_subset_of(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let result = (|| -> Result<bool, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "isSubsetOf", args)?;
        let this_size = state.sets.get(&handle).map_or(0, Vec::len);
        if this_size as f64 > record.size {
            return Ok(false);
        }
        let mut index = 0usize;
        loop {
            let Some(element) = state
                .sets
                .get(&handle)
                .and_then(|values| values.get(index).copied())
            else {
                break;
            };
            index += 1;
            if !call_has(ctx, state, &record, element)? {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    match result {
        Ok(answer) => value::encode_bool(answer),
        Err(exception) => exception,
    }
}

/// `Set.prototype.isSupersetOf`（§24.2.4.11）：thisSize < otherSize 直接
/// false；否则消费 keys 迭代器，遇缺失键 IteratorClose 后返回 false。
fn set_is_superset_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let result = (|| -> Result<bool, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "isSupersetOf", args)?;
        let this_size = state.sets.get(&handle).map_or(0, Vec::len);
        if (this_size as f64) < record.size {
            return Ok(false);
        }
        let iterator = get_keys_iterator(ctx, state, &record)?;
        while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
            if !receiver_has(state, handle, next) {
                close_keys_iterator(ctx, state, &iterator)?;
                return Ok(false);
            }
        }
        Ok(true)
    })();
    match result {
        Ok(answer) => value::encode_bool(answer),
        Err(exception) => exception,
    }
}

/// `Set.prototype.isDisjointFrom`（§24.2.4.12）：小集合侧逐元素探测，
/// 遇交集即 false（keys 路径需 IteratorClose）。
fn set_is_disjoint_from(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let result = (|| -> Result<bool, i64> {
        let (handle, record) = set_op_prelude(ctx, state, "isDisjointFrom", args)?;
        let this_size = state.sets.get(&handle).map_or(0, Vec::len);
        if this_size as f64 <= record.size {
            let mut index = 0usize;
            loop {
                let Some(element) = state
                    .sets
                    .get(&handle)
                    .and_then(|values| values.get(index).copied())
                else {
                    break;
                };
                index += 1;
                if call_has(ctx, state, &record, element)? {
                    return Ok(false);
                }
            }
        } else {
            let iterator = get_keys_iterator(ctx, state, &record)?;
            while let Some(next) = step_keys_iterator(ctx, state, &iterator)? {
                if receiver_has(state, handle, next) {
                    close_keys_iterator(ctx, state, &iterator)?;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    })();
    match result {
        Ok(answer) => value::encode_bool(answer),
        Err(exception) => exception,
    }
}
