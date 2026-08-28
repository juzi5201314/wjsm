//! `IntrinsicPristine` 守卫：判定 intrinsic 调用站点对应的属性是否仍处于
//! 原始（pristine）状态——未被赋值覆盖、未被 delete、未被换成访问器、
//! 容器全局名未被运行时遮蔽。守卫是纯查询，禁止触发 getter / Proxy trap
//! 等任何可观察副作用（惰性合成与原型物化无用户可见效果，允许发生）。
//! 返回 false 只意味着语义层放弃快路径、改走通用属性查找 + 动态调用，
//! 因此所有拿不准的情形一律保守判 false：假阴性只损失速度，假阳性破坏语义。

use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::NativeVmContext;

use super::node_perf_hooks::NodePerfHooksCallable;
use super::{global_env, runtime};
use crate::{NativeAgentState, NativeCallableKind, intrinsic_builtin};

pub(super) fn dispatch_intrinsics(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    match builtin {
        Builtin::IntrinsicPristine => Some(intrinsic_pristine(ctx, state, args)),
        _ => None,
    }
}

/// args[0] 为家族编码（`constants::INTRINSIC_FAMILY_*`），其余实参按家族解释。
fn intrinsic_pristine(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some((&family, rest)) = args.split_first() else {
        return runtime::fail_dispatch(ctx);
    };
    let pristine = match value::decode_f64(family) as i64 {
        constants::INTRINSIC_FAMILY_GLOBAL_IDENT => match rest {
            [name] => global_ident_pristine(state, *name),
            _ => return runtime::fail_dispatch(ctx),
        },
        constants::INTRINSIC_FAMILY_STATIC_MEMBER => match rest {
            [container, prop, wire] => static_member_pristine(state, *container, *prop, *wire),
            _ => return runtime::fail_dispatch(ctx),
        },
        constants::INTRINSIC_FAMILY_STRING_PROTO => match rest {
            [receiver, prop, wire] => string_proto_pristine(state, *receiver, *prop, *wire),
            _ => return runtime::fail_dispatch(ctx),
        },
        constants::INTRINSIC_FAMILY_ARRAY_PROTO => match rest {
            [receiver, prop, wire] => array_proto_pristine(state, *receiver, *prop, *wire),
            _ => return runtime::fail_dispatch(ctx),
        },
        _ => return runtime::fail_dispatch(ctx),
    };
    value::encode_bool(pristine)
}

/// 站点期望 builtin 的规范 callable 值。`native_callable` 按 kind 记忆化，
/// 值身份稳定，可与存储槽做同一性比较。个别名字的规范值不以
/// `NativeCallableKind::Builtin` 形态存储，需按同一 kind 取值。
fn expected_canonical(state: &mut NativeAgentState, wire: i64, method: bool) -> Option<i64> {
    let builtin = Builtin::from_wire_id(u16::try_from(value::decode_f64(wire) as i64).ok()?)?;
    let kind = match builtin {
        Builtin::PerformanceNow => {
            NativeCallableKind::NodePerfHooks(NodePerfHooksCallable::PerformanceNow)
        }
        _ => NativeCallableKind::Builtin(builtin, method),
    };
    state.native_callable(kind)
}

/// 全局名未被运行时触碰：无全局词法绑定（间接 eval 注入的 let/const）、
/// 无全局对象自有槽（赋值 / defineProperty 数据或访问器）、无删除墓碑。
/// 全局对象尚未创建时不存在任何用户可达的修改通道，恒 pristine。
fn global_ident_pristine(state: &mut NativeAgentState, name: i64) -> bool {
    let Some(global) = state.global_object else {
        return true;
    };
    let Some(key) = runtime::property_key(state, name) else {
        return false;
    };
    if global_env::lexical_has(state, global, key) {
        return false;
    }
    if state
        .gc
        .heap()
        .get_property_slot(value::decode_handle(global), key)
        .ok()
        .flatten()
        .is_some()
    {
        return false;
    }
    !state
        .intrinsic_tombstones
        .contains(&(value::strip_gc_color(global), key))
}

/// 内建容器静态成员（`String.raw` / `Math.floor` / `console.log`）：
/// 容器全局名未被遮蔽，且容器上该属性未被覆盖 / 删除 / 换成访问器。
fn static_member_pristine(
    state: &mut NativeAgentState,
    container_name: i64,
    prop_name: i64,
    wire: i64,
) -> bool {
    if !global_ident_pristine(state, container_name) {
        return false;
    }
    let Some(global) = state.global_object else {
        return true;
    };
    let Some(container) = state.global_property(global, container_name) else {
        // 容器名不经全局惰性合成（如 Atomics）：既然全局名未被触碰，
        // 也就不存在任何用户可达的属性修改通道，快路径保持引擎规范行为。
        return true;
    };
    let Some(key) = runtime::property_key(state, prop_name) else {
        return false;
    };
    let container = value::strip_gc_color(container);
    if value::is_native_callable(container) {
        // native callable 容器的静态成员按需惰性合成、不落 side table；
        // side table / 墓碑出现该键即为用户改动。
        return !state.callable_accessors.contains_key(&(container, key))
            && !state.callable_properties.contains_key(&(container, key))
            && !state.intrinsic_tombstones.contains(&(container, key));
    }
    if value::is_js_object(container) {
        // console / performance 等真实堆对象容器：方法在物化时写为自有
        // 数据属性，槽位必须仍持有规范值（删除→缺失，覆盖→值不同，
        // defineProperty 访问器→标志位）。
        let Some(slot) = state
            .gc
            .heap()
            .get_property_slot(value::decode_handle(container), key)
            .ok()
            .flatten()
        else {
            return false;
        };
        if slot.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
            return false;
        }
        let Some(expected) = expected_canonical(state, wire, false) else {
            return false;
        };
        return value::strip_gc_color(slot.value as i64) == value::strip_gc_color(expected);
    }
    false
}

/// %String.prototype% 方法（`"x".slice(...)`）：receiver 必须是字符串基元，
/// 且原型对象上该方法槽仍为规范内建（原型未物化时不可能被改动）。
fn string_proto_pristine(
    state: &mut NativeAgentState,
    receiver: i64,
    prop_name: i64,
    wire: i64,
) -> bool {
    if !value::is_runtime_string_handle(receiver) {
        return false;
    }
    let Some(prototype) = state.intl.string_prototype else {
        return true;
    };
    let Some(key) = runtime::property_key(state, prop_name) else {
        return false;
    };
    let Some(slot) = state
        .gc
        .heap()
        .get_property_slot(value::decode_handle(prototype), key)
        .ok()
        .flatten()
    else {
        return false;
    };
    if slot.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
        return false;
    }
    let Some(expected) = expected_canonical(state, wire, true) else {
        return false;
    };
    value::strip_gc_color(slot.value as i64) == value::strip_gc_color(expected)
}

/// %Array.prototype% 方法（`[1].map(...)`）：receiver 必须是数组、无同名
/// 自有覆盖、堆原型槽仍指向 %Array.prototype%，且原型层无覆盖 / 访问器 /
/// 删除墓碑；名字到 builtin 的惰性合成映射须与站点期望一致（自校验）。
fn array_proto_pristine(
    state: &mut NativeAgentState,
    receiver: i64,
    prop_name: i64,
    wire: i64,
) -> bool {
    let receiver = value::strip_gc_color(receiver);
    if !value::is_array(receiver) {
        return false;
    }
    let Some(key) = runtime::property_key(state, prop_name) else {
        return false;
    };
    let handle = value::decode_handle(receiver);
    if state.array_accessors.contains_key(&(handle, key))
        || state.array_properties.contains_key(&(handle, key))
    {
        return false;
    }
    if let Some(prototype) = state.array_prototype {
        let proto_handle = value::decode_handle(prototype);
        if handle != proto_handle
            && state.gc.heap().prototype(handle).ok() != Some(proto_handle)
        {
            // Object.setPrototypeOf 替换过原型（或原型物化前分配的孤儿数组）：
            // 继承来源不再可证明为 %Array.prototype%。
            return false;
        }
        if state.array_accessors.contains_key(&(proto_handle, key))
            || state.array_properties.contains_key(&(proto_handle, key))
            || state
                .intrinsic_tombstones
                .contains(&(value::strip_gc_color(prototype), key))
        {
            return false;
        }
    }
    let Some(name) = state.string_owned(prop_name).and_then(|text| text.to_utf8()) else {
        return false;
    };
    let Some(synthesized) = intrinsic_builtin(receiver, &name) else {
        return false;
    };
    i64::from(synthesized.wire_id()) == value::decode_f64(wire) as i64
}
