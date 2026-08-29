//! ArrayBuffer resizable / transfer 家族（ES2024，§25.1.6.2–.8）。
//!
//! - `resize`：resizable buffer 原地改长（grow 补零 / shrink 截断），既有
//!   视图经共享 `Rc<RefCell<Vec<u8>>>` 立即观察到新长度（length-tracking
//!   视图重算、固定视图可能越界）。
//! - `transfer` / `transferToFixedLength`：ArrayBufferCopyAndDetach
//!   （§25.1.3.2）——字节转移到新 buffer（grow 补零 / shrink 截断），
//!   原 buffer detach；`transfer` 保留 resizability，
//!   `transferToFixedLength` 收敛为固定长度。
//! - `resizable` / `maxByteLength` / `detached` 三个规范 accessor getter。
//!
//! 错误路径对齐 V8/Node：品牌检查失败按 incompatible receiver TypeError，
//! detach 后操作按 "Cannot perform ... on a detached ArrayBuffer" TypeError，
//! 长度非法按 V8 文案 RangeError。检查顺序按 Node 实测：两个方法都先跑
//! ToIndex 的用户转换（副作用可观察）；`resize` 随后 detached TypeError
//! 先于数值越界 RangeError，`transfer` 相反（数值越界 RangeError 先行）。

use std::cell::RefCell;
use std::rc::Rc;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::buffers::{ToIndexError, incompatible_receiver, to_index};
use super::runtime::{fail_dispatch, range_error, type_error};
use crate::NativeAgentState;

pub(super) fn dispatch(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    match builtin {
        Builtin::ArrayBufferProtoResize => resize(ctx, state, args),
        Builtin::ArrayBufferProtoTransfer => transfer(ctx, state, args, true),
        Builtin::ArrayBufferProtoTransferToFixedLength => transfer(ctx, state, args, false),
        Builtin::ArrayBufferProtoResizable
        | Builtin::ArrayBufferProtoMaxByteLength
        | Builtin::ArrayBufferProtoDetached => accessor(ctx, state, builtin, args),
        _ => fail_dispatch(ctx),
    }
}

/// `get resizable` / `maxByteLength` / `detached`（§25.1.6.4 / .3 / .2）：
/// 品牌检查只要求 [[ArrayBufferData]]；maxByteLength 对 detached buffer
/// 返回 +0，固定长度 buffer 返回 byteLength。
fn accessor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    let Some(buffer) = args
        .first()
        .and_then(|object| state.array_buffers.get(&value::decode_handle(*object)))
    else {
        let method = format!("get {}", builtin.as_str());
        return incompatible_receiver(ctx, state, &method, args);
    };
    match builtin {
        Builtin::ArrayBufferProtoResizable => value::encode_bool(buffer.max_byte_length.is_some()),
        Builtin::ArrayBufferProtoDetached => value::encode_bool(buffer.detached),
        Builtin::ArrayBufferProtoMaxByteLength => {
            let length = if buffer.detached {
                0
            } else {
                buffer
                    .max_byte_length
                    .unwrap_or_else(|| buffer.bytes.borrow().len())
            };
            u32::try_from(length)
                .ok()
                .map(|length| value::encode_f64(f64::from(length)))
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        _ => fail_dispatch(ctx),
    }
}

/// `ArrayBuffer.prototype.resize(newLength)`（§25.1.6.5）：品牌检查要求
/// [[ArrayBufferMaxByteLength]]（固定长度 buffer 与非 AB 同按 incompatible
/// receiver TypeError）。V8 实测顺序：ToNumber 的用户转换副作用先行，
/// detached TypeError 次之，数值越界的 RangeError 最后。
fn resize(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let receiver = args.first().copied().unwrap_or_else(value::encode_undefined);
    let handle = value::decode_handle(receiver);
    let Some(buffer) = state.array_buffers.get(&handle) else {
        return incompatible_receiver(ctx, state, "ArrayBuffer.prototype.resize", args);
    };
    let Some(max) = buffer.max_byte_length else {
        return incompatible_receiver(ctx, state, "ArrayBuffer.prototype.resize", args);
    };
    // §25.1.6.5 步骤 2 ToIndex(newLength)：可执行用户转换，Symbol / BigInt
    // 的 TypeError 原样上抛；数值越界暂存，待 detach 检查后统一渲染文案。
    let requested = match to_index(ctx, state, args.get(1).copied()) {
        Ok(new_length) => Some(new_length),
        Err(ToIndexError::Thrown(exception)) => return exception,
        Err(ToIndexError::OutOfRange(_)) => None,
    };
    // 用户转换可能中途 detach / transfer：重取条目再查（§25.1.6.5 步骤 4）。
    let Some((bytes, detached)) = state
        .array_buffers
        .get(&handle)
        .map(|buffer| (buffer.bytes.clone(), buffer.detached))
    else {
        return fail_dispatch(ctx);
    };
    if detached {
        return type_error(
            ctx,
            state,
            "Cannot perform ArrayBuffer.prototype.resize on a detached ArrayBuffer",
        );
    }
    // 负值 / ≥2^53 / 超出 maxByteLength 同按 V8 文案 RangeError。
    let invalid_length = "ArrayBuffer.prototype.resize: Invalid length parameter";
    let Some(new_length) = requested.filter(|new_length| *new_length <= max) else {
        return range_error(ctx, state, invalid_length);
    };
    // 原地改长：grow 补零、shrink 截断（§25.1.6.5 步骤 6–7 的宿主等价），
    // 共享同一 Rc 的视图立即观察到新长度。构造期已按 maxByteLength 预留
    // 容量；无预留容量的复原路径（structuredClone 等）经可恢复扩容兜底，
    // 失败按 V8 分配失败文案 RangeError，绝不 abort。
    {
        let mut bytes = bytes.borrow_mut();
        if let Some(additional) = new_length.checked_sub(bytes.len())
            && bytes.try_reserve_exact(additional).is_err()
        {
            drop(bytes);
            return range_error(ctx, state, "Array buffer allocation failed");
        }
        bytes.resize(new_length, 0);
    }
    value::encode_undefined()
}

/// `transfer` / `transferToFixedLength`（§25.1.6.6 / .7，共用
/// ArrayBufferCopyAndDetach §25.1.3.2）：newLength 缺省取当前 byteLength；
/// preserve_resizability 时 resizable 源的新 buffer 继承 maxByteLength
/// （newLength 超出即 RangeError），否则收敛为固定长度。字节直接转移
/// （grow 补零 / shrink 截断），随后 detach 源 buffer。
fn transfer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    preserve_resizability: bool,
) -> i64 {
    let method = if preserve_resizability {
        "ArrayBuffer.prototype.transfer"
    } else {
        "ArrayBuffer.prototype.transferToFixedLength"
    };
    let receiver = args.first().copied().unwrap_or_else(value::encode_undefined);
    let handle = value::decode_handle(receiver);
    if !state.array_buffers.contains_key(&handle) {
        return incompatible_receiver(ctx, state, method, args);
    }
    // §25.1.3.2 步骤 2：ToIndex(newLength) 先于 detached 检查（V8 同序），
    // 可执行用户转换；V8 实测文案分叉：负值按 kInvalidArrayBufferLength，
    // ≥2^53 按方法名 kInvalidLength。
    let new_length = match args.get(1) {
        None => None,
        Some(encoded) if value::is_undefined(*encoded) => None,
        Some(encoded) => match to_index(ctx, state, Some(*encoded)) {
            Ok(new_length) => Some(new_length),
            Err(ToIndexError::Thrown(exception)) => return exception,
            Err(ToIndexError::OutOfRange(invalid)) if invalid < 0.0 => {
                return range_error(ctx, state, "Invalid array buffer length");
            }
            Err(ToIndexError::OutOfRange(_)) => {
                let message = format!("{method}: Invalid length parameter");
                return range_error(ctx, state, &message);
            }
        },
    };
    // 用户转换可能中途 detach：重取条目再查（Node 实测 detach TypeError
    // 后于 ToIndex、先于 maxByteLength 与分配检查）。
    let Some(buffer) = state.array_buffers.get(&handle).cloned() else {
        return fail_dispatch(ctx);
    };
    if buffer.detached {
        let message = format!("Cannot perform {method} on a detached ArrayBuffer");
        return type_error(ctx, state, &message);
    }
    let new_length = new_length.unwrap_or_else(|| buffer.bytes.borrow().len());
    // §25.1.3.2 步骤 4–6：preserve-resizability 时新 buffer 继承源
    // maxByteLength，newLength 超出即 AllocateArrayBuffer 的 RangeError。
    let new_max = match buffer.max_byte_length {
        Some(max) if preserve_resizability => {
            if new_length > max {
                return range_error(ctx, state, "Invalid array buffer length");
            }
            Some(max)
        }
        _ => None,
    };
    // 字节转移（ArrayBufferCopyAndDetach）：先备好新 backing 再 detach 源，
    // 分配失败时源保持原状（Node 实测 transfer 分配失败后源未 detach）。
    // 现有容量足够则直接窃取源字节原地改长（grow 补零 / shrink 截断），
    // 否则可恢复分配后拷贝前缀，失败按 V8 分配失败文案 RangeError。
    let capacity = new_max.unwrap_or(new_length);
    let bytes = if capacity <= buffer.bytes.borrow().capacity() {
        let mut bytes = std::mem::take(&mut *buffer.bytes.borrow_mut());
        bytes.resize(new_length, 0);
        bytes
    } else {
        let Some(mut bytes) = super::buffers::try_allocate_bytes(new_length, capacity) else {
            return range_error(ctx, state, "Array buffer allocation failed");
        };
        {
            let source = buffer.bytes.borrow();
            let copied = new_length.min(source.len());
            bytes[..copied].copy_from_slice(&source[..copied]);
        }
        buffer.bytes.borrow_mut().clear();
        bytes
    };
    if let Some(entry) = state.array_buffers.get_mut(&handle) {
        entry.detached = true;
    }
    super::buffers::from_shared_bytes_with_max(state, Rc::new(RefCell::new(bytes)), new_max)
        .unwrap_or_else(|| fail_dispatch(ctx))
}
