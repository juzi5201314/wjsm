//! 立即消费型 Iterator Helper（reduce / toArray / forEach / some / every /
//! find，§27.1.4.6–27.1.4.12）：GetIteratorDirect 后循环 IteratorStepValue，
//! 回调抛出按 IfAbruptCloseIterator 关闭，命中提前退出按 normal 完成关闭。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::super::array_callbacks::push_element_with_gc_retry;
use super::super::runtime::{fail_dispatch, is_truthy, type_error};
use super::{IteratorProtoMethod, IteratorRecord, close_iterator, get_iterator_direct, step_value};
use crate::NativeAgentState;

fn call_callback(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    arguments: &[i64],
) -> Result<i64, i64> {
    let result = state
        .invoke_callable(ctx, callback, value::encode_undefined(), arguments)
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        Err(result)
    } else {
        Ok(result)
    }
}

/// 回调抛出后的 IfAbruptCloseIterator：throw 完成关闭底层迭代器并传播原异常。
fn close_throw(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
    exception: i64,
) -> i64 {
    close_iterator(ctx, state, record.iterator, exception, true)
}

/// 提前退出（some 命中 / every 失配 / find 命中）：normal 完成关闭，
/// close 期异常传播。
fn close_normal(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
    completion: i64,
) -> i64 {
    close_iterator(ctx, state, record.iterator, completion, false)
}

pub(crate) fn eager_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: IteratorProtoMethod,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let callback = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if method != IteratorProtoMethod::ToArray && !value::is_callable(callback) {
        let message = format!(
            "string \"Iterator.prototype.{}\" is not a function",
            method.name()
        );
        let error = type_error(ctx, state, &message);
        // 校验失败对临时 record（NextMethod=undefined）做 throw 完成的
        // IteratorClose：只读 return 不读 next（§27.1.4 各方法步骤 3–4）。
        return close_iterator(ctx, state, receiver, error, true);
    }
    let record = match get_iterator_direct(ctx, state, receiver) {
        Ok(record) => record,
        Err(exception) => return exception,
    };
    match method {
        IteratorProtoMethod::Reduce => reduce(ctx, state, &record, callback, args),
        IteratorProtoMethod::ToArray => to_array(ctx, state, &record),
        IteratorProtoMethod::ForEach => for_each(ctx, state, &record, callback),
        IteratorProtoMethod::Some | IteratorProtoMethod::Every | IteratorProtoMethod::Find => {
            search(ctx, state, &record, callback, method)
        }
        _ => fail_dispatch(ctx),
    }
}

/// Iterator.prototype.reduce（§27.1.4.9）。
fn reduce(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
    reducer: i64,
    args: &[i64],
) -> i64 {
    let (mut accumulator, mut counter) = if let Some(initial) = args.get(1).copied() {
        (initial, 0_u64)
    } else {
        match step_value(ctx, state, record) {
            Err(exception) => return exception,
            Ok(None) => {
                return type_error(
                    ctx,
                    state,
                    "Reduce of a done iterator with no initial value",
                );
            }
            Ok(Some(first)) => (first, 1),
        }
    };
    loop {
        let stepped = match step_value(ctx, state, record) {
            Err(exception) => return exception,
            Ok(None) => return accumulator,
            Ok(Some(stepped)) => stepped,
        };
        let arguments = [accumulator, stepped, value::encode_f64(counter as f64)];
        accumulator = match call_callback(ctx, state, reducer, &arguments) {
            Err(exception) => return close_throw(ctx, state, record, exception),
            Ok(result) => result,
        };
        counter += 1;
    }
}

/// Iterator.prototype.toArray（§27.1.4.12）。
fn to_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
) -> i64 {
    let Ok(array) = state.allocate_object_with_gc_retry(ctx, 0, true) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(array);
    loop {
        let stepped = match step_value(ctx, state, record) {
            Err(exception) => return exception,
            Ok(None) => return array,
            Ok(Some(stepped)) => stepped,
        };
        if push_element_with_gc_retry(ctx, state, handle, stepped as u64).is_err() {
            return fail_dispatch(ctx);
        }
    }
}

/// Iterator.prototype.forEach（§27.1.4.7）。
fn for_each(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
    callback: i64,
) -> i64 {
    let mut counter = 0_u64;
    loop {
        let stepped = match step_value(ctx, state, record) {
            Err(exception) => return exception,
            Ok(None) => return value::encode_undefined(),
            Ok(Some(stepped)) => stepped,
        };
        let arguments = [stepped, value::encode_f64(counter as f64)];
        if let Err(exception) = call_callback(ctx, state, callback, &arguments) {
            return close_throw(ctx, state, record, exception);
        }
        counter += 1;
    }
}

/// some / every / find 的共享循环（§27.1.4.10 / §27.1.4.6 / §27.1.4.2 顺序
/// 对应 Some / Every / Find）：命中即 normal 关闭并返回。
fn search(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
    predicate: i64,
    method: IteratorProtoMethod,
) -> i64 {
    let mut counter = 0_u64;
    loop {
        let stepped = match step_value(ctx, state, record) {
            Err(exception) => return exception,
            Ok(None) => {
                return match method {
                    IteratorProtoMethod::Some => value::encode_bool(false),
                    IteratorProtoMethod::Every => value::encode_bool(true),
                    _ => value::encode_undefined(),
                };
            }
            Ok(Some(stepped)) => stepped,
        };
        let arguments = [stepped, value::encode_f64(counter as f64)];
        let verdict = match call_callback(ctx, state, predicate, &arguments) {
            Err(exception) => return close_throw(ctx, state, record, exception),
            Ok(verdict) => is_truthy(state, verdict),
        };
        match method {
            IteratorProtoMethod::Some if verdict => {
                return close_normal(ctx, state, record, value::encode_bool(true));
            }
            IteratorProtoMethod::Every if !verdict => {
                return close_normal(ctx, state, record, value::encode_bool(false));
            }
            IteratorProtoMethod::Find if verdict => {
                return close_normal(ctx, state, record, stepped);
            }
            _ => {}
        }
        counter += 1;
    }
}
