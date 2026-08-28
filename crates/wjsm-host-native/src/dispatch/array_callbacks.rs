use std::cmp::Ordering;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::array_like::{ArrayLikeSource, element_get, element_has, element_slots_trusted};
use super::runtime::{
    allocate_object_or_out_of_memory, fail_dispatch, is_truthy, range_error, render_value,
    to_number, type_error,
};
use crate::NativeAgentState;

pub(super) fn dispatch_array_callback(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ArrayForEach => iterate(ctx, state, args, IterationKind::ForEach),
        Builtin::ArrayMap => iterate(ctx, state, args, IterationKind::Map),
        Builtin::ArrayFilter => iterate(ctx, state, args, IterationKind::Filter),
        Builtin::ArrayFind => iterate(ctx, state, args, IterationKind::Find),
        Builtin::ArrayFindIndex => iterate(ctx, state, args, IterationKind::FindIndex),
        Builtin::ArrayFindLast => iterate(ctx, state, args, IterationKind::FindLast),
        Builtin::ArrayFindLastIndex => iterate(ctx, state, args, IterationKind::FindLastIndex),
        Builtin::ArraySome => iterate(ctx, state, args, IterationKind::Some),
        Builtin::ArrayEvery => iterate(ctx, state, args, IterationKind::Every),
        Builtin::ArrayFlatMap => iterate(ctx, state, args, IterationKind::FlatMap),
        Builtin::ArrayReduce => reduce(ctx, state, args, false),
        Builtin::ArrayReduceRight => reduce(ctx, state, args, true),
        Builtin::ArraySort => sort(ctx, state, args, false),
        Builtin::ArrayToSorted => sort(ctx, state, args, true),
        _ => return None,
    })
}

pub(super) fn set_element_with_gc_retry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    index: u32,
    value: u64,
) -> Result<(), ()> {
    match state.gc.heap().set_element(handle, index, value) {
        Ok(()) => return Ok(()),
        Err(wjsm_gc::HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state.gc.flush_native_tlab(ctx).map_err(|_| ())?;
            if state.gc.heap().set_element(handle, index, value).is_ok() {
                return Ok(());
            }
        }
        Err(_) => {}
    }
    if state.collect_garbage(ctx).is_ok() {
        let _ = state.gc.heap().finish_relocation_epoch();
        let _ = state.gc.heap().advance_epoch_and_reclaim();
        if state.gc.heap().set_element(handle, index, value).is_ok() {
            return Ok(());
        }
    }
    Err(())
}

pub(super) fn push_element_with_gc_retry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    value: u64,
) -> Result<u32, ()> {
    match state.gc.heap().push_element(handle, value) {
        Ok(length) => return Ok(length),
        Err(wjsm_gc::HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state.gc.flush_native_tlab(ctx).map_err(|_| ())?;
            if let Ok(length) = state.gc.heap().push_element(handle, value) {
                return Ok(length);
            }
        }
        Err(_) => {}
    }
    if state.collect_garbage(ctx).is_ok() {
        let _ = state.gc.heap().finish_relocation_epoch();
        let _ = state.gc.heap().advance_epoch_and_reclaim();
        if let Ok(length) = state.gc.heap().push_element(handle, value) {
            return Ok(length);
        }
    }
    Err(())
}

fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    this_value: i64,
    args: &[i64],
) -> Result<i64, i64> {
    let result = state
        .invoke_callable(ctx, callback, this_value, args)
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        Err(result)
    } else {
        Ok(result)
    }
}

/// 回调实参非 callable 的 TypeError（§23.1.3 各方法 IsCallable 校验，
/// 文案对齐 V8）：原语带 typeof 词与值渲染（字符串加引号），非 callable
/// 对象 / symbol / bigint 只报类型词。
fn callback_type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
) -> i64 {
    let rendered = if value::is_undefined(callback) {
        "undefined".to_owned()
    } else if value::is_null(callback) {
        "object null".to_owned()
    } else if value::is_f64(callback) {
        format!("number {}", render_value(state, callback))
    } else if value::is_string(callback) {
        format!("string \"{}\"", render_value(state, callback))
    } else if value::is_bool(callback) {
        format!("boolean {}", render_value(state, callback))
    } else if value::is_symbol(callback) {
        "symbol".to_owned()
    } else if value::is_bigint(callback) {
        "bigint".to_owned()
    } else {
        "object".to_owned()
    };
    let message = format!("{rendered} is not a function");
    type_error(ctx, state, &message)
}

#[derive(Clone, Copy)]
enum IterationKind {
    Every,
    Filter,
    Find,
    FindIndex,
    FindLast,
    FindLastIndex,
    FlatMap,
    ForEach,
    Map,
    Some,
}

/// null/undefined 接收者的 TypeError 文案选择（V8 口径）：flatMap 走
/// ToObject 通用文案，其余为 `Array.prototype.<name> called on ...`。
fn null_receiver_method(kind: IterationKind) -> Option<&'static str> {
    match kind {
        IterationKind::Every => Some("every"),
        IterationKind::Filter => Some("filter"),
        IterationKind::Find => Some("find"),
        IterationKind::FindIndex => Some("findIndex"),
        IterationKind::FindLast => Some("findLast"),
        IterationKind::FindLastIndex => Some("findLastIndex"),
        IterationKind::FlatMap => None,
        IterationKind::ForEach => Some("forEach"),
        IterationKind::Map => Some("map"),
        IterationKind::Some => Some("some"),
    }
}

fn iterate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: IterationKind,
) -> i64 {
    let receiver = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let initial_temp_roots = state.temporary_roots.len();
    let res = (|| {
        let source =
            match ArrayLikeSource::resolve(ctx, state, receiver, null_receiver_method(kind)) {
                Ok(source) => source,
                Err(exception) => return exception,
            };
        // §23.1.3.21 等步骤 3：IsCallable 校验在 ToObject / LengthOfArrayLike
        // 之后（length getter 先于回调形态错误可观察）。
        let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
        if !value::is_callable(callback) {
            return callback_type_error(ctx, state, callback);
        }
        let this_value = args.get(2).copied().unwrap_or_else(value::encode_undefined);
        let length = source.length();
        let result_array = match kind {
            IterationKind::Map => {
                // ArraySpeciesCreate(O, len)（步骤 4）：非数组接收者退化为
                // ArrayCreate(len)。复用 Array(n) 构造以获得洞哨兵填充与
                // HOLEY kind（未回填的跳过索引必须读出洞，而非未初始化槽），
                // len 超过 2^32 − 1 由其抛 RangeError。
                let array =
                    super::array::construct(ctx, state, &[value::encode_f64(length as f64)]);
                if value::is_exception(array) {
                    return array;
                }
                Some(array)
            }
            IterationKind::Filter | IterationKind::FlatMap => {
                let array =
                    allocate_object_or_out_of_memory(ctx, state, source.allocation_hint(), true);
                if value::is_exception(array) {
                    return array;
                }
                Some(array)
            }
            _ => None,
        };
        let visits_holes = matches!(
            kind,
            IterationKind::Find
                | IterationKind::FindIndex
                | IterationKind::FindLast
                | IterationKind::FindLastIndex
        );
        let reverse = matches!(kind, IterationKind::FindLast | IterationKind::FindLastIndex);
        let indices: Box<dyn Iterator<Item = u64>> = if reverse {
            Box::new((0..length).rev())
        } else {
            Box::new(0..length)
        };
        if let Some(array) = result_array {
            state.temporary_roots.push(array);
        }
        state.temporary_roots.push(source.receiver());
        state.temporary_roots.push(callback);
        state.temporary_roots.push(this_value);

        for index in indices {
            // 跳洞方法先 HasProperty（步骤 6.b），find 族读穿洞只做 Get。
            if !visits_holes {
                match source.has(ctx, state, index) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(exception) => return exception,
                }
            }
            let element = match source.get(ctx, state, index) {
                Ok(element) => element,
                Err(exception) => return exception,
            };
            let callback_args = [element, value::encode_f64(index as f64), source.receiver()];
            let callback_result = match call(ctx, state, callback, this_value, &callback_args) {
                Ok(result) => result,
                Err(exception) => return exception,
            };
            match kind {
                IterationKind::ForEach => {}
                IterationKind::Map => {
                    let output = value::decode_handle(result_array.expect("map allocates output"));
                    if set_element_with_gc_retry(
                        ctx,
                        state,
                        output,
                        index as u32,
                        callback_result as u64,
                    )
                    .is_err()
                    {
                        return fail_dispatch(ctx);
                    }
                }
                IterationKind::Filter => {
                    if is_truthy(state, callback_result) {
                        let output =
                            value::decode_handle(result_array.expect("filter allocates output"));
                        if push_element_with_gc_retry(ctx, state, output, element as u64).is_err() {
                            return fail_dispatch(ctx);
                        }
                    }
                }
                IterationKind::FlatMap => {
                    let output =
                        value::decode_handle(result_array.expect("flatMap allocates output"));
                    if value::is_array(callback_result) {
                        // FlattenIntoArray（§23.1.3.13.1）depth=0 递归层：逐索引
                        // 先 HasProperty 跳过内层洞（原型链继承索引可观察），
                        // 存在才 Get 追加。内层数组与 Get 产出的值会途经 GC
                        // 安全点（原型 getter 分配 / 追加扩容），循环期间锚根；
                        // 提前 return 的异常路径由收尾 truncate 统一清根。
                        let inner = value::decode_handle(callback_result);
                        let Ok(inner_length) = state.gc.heap().array_length(inner) else {
                            return fail_dispatch(ctx);
                        };
                        state.temporary_roots.push(callback_result);
                        for inner_index in 0..u64::from(inner_length) {
                            match element_has(ctx, state, callback_result, inner_index) {
                                Ok(true) => {}
                                Ok(false) => continue,
                                Err(exception) => return exception,
                            }
                            let inner_value =
                                match element_get(ctx, state, callback_result, inner_index) {
                                    Ok(inner_value) => inner_value,
                                    Err(exception) => return exception,
                                };
                            state.temporary_roots.push(inner_value);
                            let pushed =
                                push_element_with_gc_retry(ctx, state, output, inner_value as u64);
                            state.temporary_roots.pop();
                            if pushed.is_err() {
                                return fail_dispatch(ctx);
                            }
                        }
                        state.temporary_roots.pop();
                    } else if push_element_with_gc_retry(ctx, state, output, callback_result as u64)
                        .is_err()
                    {
                        return fail_dispatch(ctx);
                    }
                }
                IterationKind::Find if is_truthy(state, callback_result) => return element,
                IterationKind::FindIndex if is_truthy(state, callback_result) => {
                    return value::encode_f64(index as f64);
                }
                IterationKind::FindLast if is_truthy(state, callback_result) => return element,
                IterationKind::FindLastIndex if is_truthy(state, callback_result) => {
                    return value::encode_f64(index as f64);
                }
                IterationKind::Some if is_truthy(state, callback_result) => {
                    return value::encode_bool(true);
                }
                IterationKind::Every if !is_truthy(state, callback_result) => {
                    return value::encode_bool(false);
                }
                IterationKind::Find
                | IterationKind::FindIndex
                | IterationKind::FindLast
                | IterationKind::FindLastIndex
                | IterationKind::Some
                | IterationKind::Every => {}
            }
        }
        match kind {
            IterationKind::Map | IterationKind::Filter | IterationKind::FlatMap => {
                result_array.expect("array-producing iteration allocates output")
            }
            IterationKind::Find | IterationKind::FindLast => value::encode_undefined(),
            IterationKind::FindIndex | IterationKind::FindLastIndex => value::encode_f64(-1.0),
            IterationKind::Some => value::encode_bool(false),
            IterationKind::Every => value::encode_bool(true),
            IterationKind::ForEach => value::encode_undefined(),
        }
    })();
    state.temporary_roots.truncate(initial_temp_roots);
    res
}

fn reduce(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let receiver = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let initial_temp_roots = state.temporary_roots.len();
    let res = (|| {
        let method = if reverse { "reduceRight" } else { "reduce" };
        let source = match ArrayLikeSource::resolve(ctx, state, receiver, Some(method)) {
            Ok(source) => source,
            Err(exception) => return exception,
        };
        let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
        if !value::is_callable(callback) {
            return callback_type_error(ctx, state, callback);
        }
        let length = source.length();
        let mut indices: Box<dyn Iterator<Item = u64>> = if reverse {
            Box::new((0..length).rev())
        } else {
            Box::new(0..length)
        };
        state.temporary_roots.push(source.receiver());
        state.temporary_roots.push(callback);
        // 无初始值：按 §23.1.3.24 步骤 8 用第一个存在的索引播种 accumulator。
        let mut accumulator = if let Some(initial) = args.get(2).copied() {
            initial
        } else {
            loop {
                let Some(index) = indices.next() else {
                    return type_error(ctx, state, "Reduce of empty array with no initial value");
                };
                match source.has(ctx, state, index) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(exception) => return exception,
                }
                match source.get(ctx, state, index) {
                    Ok(element) => break element,
                    Err(exception) => return exception,
                }
            }
        };
        state.temporary_roots.push(accumulator);

        for index in indices {
            match source.has(ctx, state, index) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(exception) => return exception,
            }
            let element = match source.get(ctx, state, index) {
                Ok(element) => element,
                Err(exception) => return exception,
            };
            let callback_args = [
                accumulator,
                element,
                value::encode_f64(index as f64),
                source.receiver(),
            ];
            accumulator = match call(
                ctx,
                state,
                callback,
                value::encode_undefined(),
                &callback_args,
            ) {
                Ok(result) => result,
                Err(exception) => return exception,
            };
            if let Some(last) = state.temporary_roots.last_mut() {
                *last = accumulator;
            }
        }
        accumulator
    })();
    state.temporary_roots.truncate(initial_temp_roots);
    res
}

fn sort(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64], copy: bool) -> i64 {
    let initial_temp_roots = state.temporary_roots.len();
    let res = (|| {
        // §23.1.3.30 步骤 1：comparator 形态校验先于接收者 ToObject。
        let comparator = args
            .get(1)
            .copied()
            .filter(|value| !value::is_undefined(*value));
        if let Some(comparator) = comparator
            && !value::is_callable(comparator)
        {
            let message = format!(
                "The comparison function must be either a function or undefined: {}",
                render_value(state, comparator)
            );
            return type_error(ctx, state, &message);
        }
        let receiver = args
            .first()
            .copied()
            .unwrap_or_else(value::encode_undefined);
        let source = match ArrayLikeSource::resolve(ctx, state, receiver, None) {
            Ok(source) => source,
            Err(exception) => return exception,
        };
        let length = source.length();
        // toSorted 的 ArrayCreate(len)（§23.1.3.34 步骤 3）先于元素读取。
        if copy && u32::try_from(length).is_err() {
            return range_error(ctx, state, "Invalid array length");
        }
        state.temporary_roots.push(source.receiver());
        if let Some(comparator) = comparator {
            state.temporary_roots.push(comparator);
        }
        // SortIndexedProperties（§23.1.3.30.1）：sort 跳洞收集，toSorted
        // 读穿洞（缺失索引按 undefined 参与排序）。generic 读取可再入
        // getter 触发 GC，值随读随锚根。
        let mut values = Vec::with_capacity(source.allocation_hint() as usize);
        for index in 0..length {
            if !copy {
                match source.has(ctx, state, index) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(exception) => return exception,
                }
            }
            match source.get(ctx, state, index) {
                Ok(element) => {
                    state.temporary_roots.push(element);
                    values.push(element);
                }
                Err(exception) => return exception,
            }
        }
        let sorted = super::array_sort::stable_sort_by(&mut values, |left, right| {
            compare(ctx, state, comparator, left, right)
        });
        if let Err(exception) = sorted {
            return exception;
        }
        if copy {
            return state
                .allocate_array_values_with_gc_retry(ctx, &values)
                .unwrap_or_else(|_| fail_dispatch(ctx));
        }
        match source {
            // 元素槽可信（非字典 kind）才允许直写写回；比较器可能在排序中
            // 给数组索引装 accessor（升为字典 kind），此时直写会绕过 setter
            // 与不可写特性，退回下方规范 Set / DeletePropertyOrThrow 路径。
            ArrayLikeSource::Fast {
                encoded,
                handle,
                length,
            } if element_slots_trusted(state, handle) => {
                let present = values.len() as u32;
                for (index, stored) in values.into_iter().enumerate() {
                    if set_element_with_gc_retry(ctx, state, handle, index as u32, stored as u64)
                        .is_err()
                    {
                        return fail_dispatch(ctx);
                    }
                }
                for index in present..length {
                    if set_element_with_gc_retry(
                        ctx,
                        state,
                        handle,
                        index,
                        value::encode_array_hole() as u64,
                    )
                    .is_err()
                    {
                        return fail_dispatch(ctx);
                    }
                }
                encoded
            }
            source => {
                // §23.1.3.30 步骤 7–9：Set 逐键写回，收集数少于 length 的
                // 尾部索引 DeletePropertyOrThrow。
                let object = source.receiver();
                let length = source.length();
                let present = values.len() as u64;
                for (index, stored) in values.into_iter().enumerate() {
                    if let Err(exception) = super::array_like::set_index_or_throw(
                        ctx,
                        state,
                        object,
                        index as u64,
                        stored,
                    ) {
                        return exception;
                    }
                }
                for index in present..length {
                    if let Err(exception) =
                        super::array_like::delete_index_or_throw(ctx, state, object, index)
                    {
                        return exception;
                    }
                }
                object
            }
        }
    })();
    state.temporary_roots.truncate(initial_temp_roots);
    res
}

fn compare(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    comparator: Option<i64>,
    left: i64,
    right: i64,
) -> Result<Ordering, i64> {
    if value::is_undefined(left) || value::is_undefined(right) {
        return Ok(
            match (value::is_undefined(left), value::is_undefined(right)) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => unreachable!(),
            },
        );
    }
    if let Some(comparator) = comparator {
        let result = call(
            ctx,
            state,
            comparator,
            value::encode_undefined(),
            &[left, right],
        )?;
        let number = to_number(state, result).ok_or_else(|| fail_dispatch(ctx))?;
        Ok(if number.is_nan() || number == 0.0 {
            Ordering::Equal
        } else if number < 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        })
    } else {
        Ok(render_value(state, left).cmp(&render_value(state, right)))
    }
}
