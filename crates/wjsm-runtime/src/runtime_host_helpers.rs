use super::*;
use crate::wasm_env::WasmEnv;
use std::sync::atomic::Ordering;

/// 将诊断行写入 RuntimeState.diagnostics（供 in-process fixture 捕获）。
pub(crate) fn append_runtime_diagnostic<C: RuntimeStateAccess>(ctx: &mut C, line: &str) {
    let mut buf = ctx
        .state_mut()
        .diagnostics
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    buf.extend_from_slice(line.as_bytes());
    if !line.ends_with('\n') {
        buf.push(b'\n');
    }
}

/// 构造一个 TAG_EXCEPTION 值（携带 TypeError 对象），供属性/调用等返回路径
/// 直接返回给被编译代码，从而经语义层插入的 IsException 分叉被 try/catch 捕获。
/// 这与延迟、不可捕获的 `set_runtime_error` 不同：后者只在程序结束时作为顶层
/// "Runtime error:" 暴露。Proxy 不变量违反 / 撤销代理访问 / private 品牌检查失败
/// 等规范要求“同步抛出 TypeError”的场景应使用本函数。
pub(crate) fn make_type_error_exception(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
    make_error_exception(caller, "TypeError", msg)
}

/// 构造可捕获的 RangeError（如数组长度超过 2^32 - 1）。
pub(crate) fn make_range_error_exception(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
    make_error_exception(caller, "RangeError", msg)
}

fn make_error_exception(caller: &mut Caller<'_, RuntimeState>, error_name: &str, msg: &str) -> i64 {
    let msg_val = store_runtime_string(caller, msg.to_string());
    let error_obj = create_error_object(caller, error_name, msg_val, value::encode_undefined());
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(crate::ErrorEntry {
        name: error_name.to_string(),
        message: msg.to_string(),
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}

/// 从 TAG_EXCEPTION 中提取 error_table 里的真实错误对象值。
/// 用于需要 reject promise 或传播真实错误值的场景（如 async 迭代器异常、array spread）。
pub(crate) fn exception_reason_from_state(state: &RuntimeState, exception: i64) -> i64 {
    let idx = value::decode_handle(exception) as usize;
    let errors = state.error_table.lock().unwrap_or_else(|e| e.into_inner());
    errors
        .get(idx)
        .map(|entry| entry.value)
        .unwrap_or_else(value::encode_undefined)
}

pub(crate) fn exception_reason(caller: &mut Caller<'_, RuntimeState>, exception: i64) -> i64 {
    exception_reason_from_state(caller.data(), exception)
}

pub(crate) fn read_shadow_arg_with_env<C: AsContext>(
    ctx: &C,
    env: &WasmEnv,
    args_base: i32,
    index: u32,
) -> i64 {
    let data = env.memory.data(ctx);
    let offset = args_base as usize + (index as usize) * 8;
    if offset + 8 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

// ── call_wasm_callback 共享核心（sync/async 复用）─────────────────
//
// sync 与 async 版本曾各持一份 ~150 行的影子栈写入 + 函数解析逻辑，仅在
// 三个 dispatch 点（native callable / bound 递归 / WASM invocation）有 sync vs
// async 差异。下面把无差异部分抽成两个 helper，两个版本退化为薄 dispatcher。

/// 解析后的回调目标。proxy 链已走完，剩下终态（含 Proxy apply trap）。
pub(crate) enum CallbackTarget {
    /// native callable —— 不走 WASM，直接 host 调用。
    Native(i64),
    /// bound function —— 需合并 bound_args 后递归。
    Bound {
        target_func: i64,
        bound_this: i64,
        bound_args: Vec<i64>,
    },
    /// Proxy `handler.apply(target, thisArg, argumentsList)`（不可当作单参数函数）。
    ApplyTrap {
        trap: i64,
        handler: i64,
        proxy_target: i64,
    },
    /// WASM 函数表调用：func table 索引 + 闭包环境对象。
    Wasm { func_idx: u32, env_obj: i64 },
}

/// 将 args 写入影子栈（`WasmEnv` + 任意 `AsContextMut`）。返回调用前的 `shadow_sp`。
pub(crate) fn push_args_to_shadow_stack<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    args: &[i64],
) -> Option<i32> {
    let saved_sp = env.shadow_sp.get(&mut *ctx).i32().unwrap_or(0);
    let args_bytes = args.len().checked_mul(8)?;
    {
        let data = env.memory.data_mut(&mut *ctx);
        let offset = saved_sp as usize;
        if offset + args_bytes > data.len() {
            return None;
        }
        for (index, arg) in args.iter().enumerate() {
            let write_offset = offset + index * 8;
            data[write_offset..write_offset + 8].copy_from_slice(&arg.to_le_bytes());
        }
    }
    let new_sp = saved_sp + (args.len() as i32) * 8;
    let _ = env.shadow_sp.set(&mut *ctx, Val::I32(new_sp));
    Some(saved_sp)
}

pub(crate) fn restore_shadow_sp<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    saved_sp: i32,
) {
    let _ = env.shadow_sp.set(&mut *ctx, Val::I32(saved_sp));
}

mod host_helpers_alloc;
/// 将 args 写入影子栈并推进 `__shadow_sp`。返回 `(shadow_sp_global, 原始 shadow_sp)`，
/// 调用方在 dispatch 后须用原始值恢复 `__shadow_sp`。
mod host_helpers_callback;
mod host_helpers_descriptor;
mod host_helpers_property;
mod host_helpers_proxy;

pub(crate) use host_helpers_alloc::*;
pub(crate) use host_helpers_callback::*;
pub(crate) use host_helpers_descriptor::*;
pub(crate) use host_helpers_property::*;
pub(crate) use host_helpers_proxy::*;
