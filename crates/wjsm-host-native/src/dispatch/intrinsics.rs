//! intrinsic 快路径的宿主支撑：`IntrinsicPristine` 守卫与 `IntrinsicResolve`
//! 慢路径解析。两者只携带 `(family, wire_id)`，站点属性名经
//! `wjsm_ir::intrinsic_sites` 反查——名字不进制品常量池，守卫路径全程零
//! 新增字符串驻留（纯 pristine 执行不得改变宿主驻留表）。
//!
//! `IntrinsicPristine` 判定站点属性是否仍处于原始状态——未被赋值覆盖、未被
//! delete、未被换成访问器、容器全局名未被运行时遮蔽。守卫是纯查询，禁止
//! 触发 getter / Proxy trap 等任何可观察副作用（惰性合成与原型物化无用户
//! 可见效果，允许发生）。返回 false 只意味着语义层放弃快路径、改走通用
//! 属性查找 + 动态调用，因此所有拿不准的情形一律保守判 false：假阴性只
//! 损失速度，假阳性破坏语义。
//!
//! `IntrinsicResolve` 只在守卫判 false 后到达，按完整属性语义解析站点
//! callee / 容器（getter 生效、缺失全局名抛 ReferenceError），此处允许
//! 驻留名字——用户修改站点属性时其键名必然已经 `property_key` 驻留。

use wjsm_host::content_hash_units;
use wjsm_ir::{Builtin, constants, intrinsic_sites, value};
use wjsm_native_abi::NativeVmContext;

use super::node_perf_hooks::NodePerfHooksCallable;
use super::{global_env, runtime};
use crate::{NativeAgentState, NativeCallableKind, PropertyKey, intrinsic_builtin};

pub(super) fn dispatch_intrinsics(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    match builtin {
        Builtin::IntrinsicPristine => Some(intrinsic_pristine(ctx, state, args)),
        Builtin::IntrinsicResolve => Some(intrinsic_resolve(ctx, state, args)),
        _ => None,
    }
}

/// wire_id 实参 → 站点快路径 builtin。
fn site_builtin(wire: i64) -> Option<Builtin> {
    Builtin::from_wire_id(u16::try_from(value::decode_f64(wire) as i64).ok()?)
}

/// 站点名（ASCII）的既有编码字符串值：SSO 内联名直接编码；超长名仅当已
/// 驻留时返回现有句柄，不发布新字符串。None ⇒ 该名从未驻留——用户可达的
/// 属性写入、defineProperty、delete 与全局词法注入都经 `property_key`
/// 驻留键名（属性键名字符串被根集常驻，不会在仍被键引用时被 GC 剪除），
/// 从未驻留即证明该名下不存在任何用户修改。
fn existing_name_value(state: &NativeAgentState, name: &str) -> Option<i64> {
    if let Some(encoded) = value::encode_inline_ascii(name.as_bytes()) {
        return Some(encoded);
    }
    let units: Vec<u16> = name.encode_utf16().collect();
    let length = u32::try_from(units.len()).ok()?;
    state.dedup_string_handle(&(content_hash_units(&units), length))
}

/// 既有编码字符串值 → 属性键（零驻留、零分配）。
fn key_of(encoded: i64) -> Option<PropertyKey> {
    if value::is_inline_string(encoded) {
        return PropertyKey::inline_string(encoded);
    }
    Some(PropertyKey::from_name_id(
        value::decode_runtime_string_handle(encoded),
    ))
}

/// args: [family, wire_id, receiver?]，family 见 `constants::INTRINSIC_FAMILY_*`。
fn intrinsic_pristine(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [family, wire, rest @ ..] = args else {
        return runtime::fail_dispatch(ctx);
    };
    let Some(builtin) = site_builtin(*wire) else {
        return runtime::fail_dispatch(ctx);
    };
    let pristine = match (value::decode_f64(*family) as i64, rest) {
        (constants::INTRINSIC_FAMILY_GLOBAL_IDENT, []) => intrinsic_sites::global_ident_name(
            builtin,
        )
        .is_some_and(|name| global_ident_pristine(state, name)),
        (constants::INTRINSIC_FAMILY_STATIC_MEMBER, []) => {
            intrinsic_sites::static_member_names(builtin).is_some_and(|(container, prop)| {
                static_member_pristine(state, builtin, container, prop)
            })
        }
        (constants::INTRINSIC_FAMILY_STRING_PROTO, [receiver]) => {
            intrinsic_sites::string_proto_name(builtin)
                .is_some_and(|name| string_proto_pristine(state, *receiver, name, builtin))
        }
        (constants::INTRINSIC_FAMILY_ARRAY_PROTO, [receiver]) => {
            intrinsic_sites::array_proto_name(builtin)
                .is_some_and(|name| array_proto_pristine(state, *receiver, name, builtin))
        }
        _ => return runtime::fail_dispatch(ctx),
    };
    value::encode_bool(pristine)
}

/// 站点期望 builtin 的规范 callable 值。`native_callable` 按 kind 记忆化，
/// 值身份稳定，可与存储槽做同一性比较。个别名字的规范值不以
/// `NativeCallableKind::Builtin` 形态存储，需按同一 kind 取值。
fn expected_canonical(state: &mut NativeAgentState, builtin: Builtin, method: bool) -> Option<i64> {
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
/// 全局对象尚未创建或名字从未驻留时不存在任何用户可达的修改通道，恒
/// pristine。
fn global_ident_pristine(state: &mut NativeAgentState, name: &str) -> bool {
    let Some(global) = state.global_object else {
        return true;
    };
    let Some(encoded) = existing_name_value(state, name) else {
        return true;
    };
    let Some(key) = key_of(encoded) else {
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
    builtin: Builtin,
    container_name: &str,
    prop_name: &str,
) -> bool {
    if !global_ident_pristine(state, container_name) {
        return false;
    }
    let Some(global) = state.global_object else {
        return true;
    };
    // 容器名从未驻留 ⇒ 用户从未经名字触达容器 ⇒ 容器属性无修改通道。
    let Some(container_encoded) = existing_name_value(state, container_name) else {
        return true;
    };
    let Some(container) = state.global_property(global, container_encoded) else {
        // 容器名不经全局惰性合成（如 Atomics）：既然全局名未被触碰，
        // 也就不存在任何用户可达的属性修改通道，快路径保持引擎规范行为。
        return true;
    };
    // 属性名从未驻留 ⇒ 该键下无 side table 覆盖 / 自有槽 / 墓碑（堆对象
    // 容器物化时即驻留全部方法名，名字缺失同样证明无修改）。
    let Some(prop_encoded) = existing_name_value(state, prop_name) else {
        return true;
    };
    let Some(key) = key_of(prop_encoded) else {
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
        let Some(expected) = expected_canonical(state, builtin, false) else {
            return false;
        };
        return value::strip_gc_color(slot.value as i64) == value::strip_gc_color(expected);
    }
    false
}

/// %String.prototype% 方法（`"x".slice(...)`）：receiver 必须是字符串基元
/// （含 SSO 内联），且原型对象上该方法槽仍为规范内建（原型未物化时不可能
/// 被改动）。原型物化时全部方法名已驻留，名字缺失说明状态不一致，保守
/// 判 false。
fn string_proto_pristine(
    state: &mut NativeAgentState,
    receiver: i64,
    name: &str,
    builtin: Builtin,
) -> bool {
    if !value::is_string(receiver) {
        return false;
    }
    let Some(prototype) = state.intl.string_prototype else {
        return true;
    };
    let Some(key) = existing_name_value(state, name).and_then(key_of) else {
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
    let Some(expected) = expected_canonical(state, builtin, true) else {
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
    name: &str,
    builtin: Builtin,
) -> bool {
    let receiver = value::strip_gc_color(receiver);
    if !value::is_array(receiver) {
        return false;
    }
    let handle = value::decode_handle(receiver);
    if let Some(prototype) = state.array_prototype {
        let proto_handle = value::decode_handle(prototype);
        if handle != proto_handle && state.gc.heap().prototype(handle).ok() != Some(proto_handle) {
            // Object.setPrototypeOf 替换过原型（或原型物化前分配的孤儿数组）：
            // 继承来源不再可证明为 %Array.prototype%。
            return false;
        }
    }
    // 键从未驻留 ⇒ 自有覆盖 / 原型覆盖 / 墓碑都不可能存在，跳过键探测。
    if let Some(key) = existing_name_value(state, name).and_then(key_of) {
        if state.array_accessors.contains_key(&(handle, key))
            || state.array_properties.contains_key(&(handle, key))
        {
            return false;
        }
        if let Some(prototype) = state.array_prototype {
            let proto_handle = value::decode_handle(prototype);
            if state.array_accessors.contains_key(&(proto_handle, key))
                || state.array_properties.contains_key(&(proto_handle, key))
                || state
                    .intrinsic_tombstones
                    .contains(&(value::strip_gc_color(prototype), key))
            {
                return false;
            }
            // 原型对象自有堆槽（对象写路径落槽时）：存在即须为持有规范值
            // 的数据属性，否则视为用户覆盖。
            if let Ok(Some(slot)) = state.gc.heap().get_property_slot(proto_handle, key) {
                if slot.flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
                    return false;
                }
                let Some(expected) = expected_canonical(state, builtin, true) else {
                    return false;
                };
                if value::strip_gc_color(slot.value as i64) != value::strip_gc_color(expected) {
                    return false;
                }
            }
        }
    }
    intrinsic_builtin(receiver, name) == Some(builtin)
}

/// args: [family, wire_id] 解析站点全局名（GLOBAL_IDENT 为站点名、
/// STATIC_MEMBER 为容器名）；[family, wire_id, receiver] 解析 receiver 上
/// 的站点属性成员。
fn intrinsic_resolve(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [family, wire, rest @ ..] = args else {
        return runtime::fail_dispatch(ctx);
    };
    let Some(builtin) = site_builtin(*wire) else {
        return runtime::fail_dispatch(ctx);
    };
    let resolved_name = match (value::decode_f64(*family) as i64, rest) {
        (constants::INTRINSIC_FAMILY_GLOBAL_IDENT, []) => {
            intrinsic_sites::global_ident_name(builtin).map(|name| (name, None))
        }
        (constants::INTRINSIC_FAMILY_STATIC_MEMBER, []) => {
            intrinsic_sites::static_member_names(builtin).map(|(container, _)| (container, None))
        }
        (constants::INTRINSIC_FAMILY_STATIC_MEMBER, [receiver]) => {
            intrinsic_sites::static_member_names(builtin).map(|(_, prop)| (prop, Some(*receiver)))
        }
        (constants::INTRINSIC_FAMILY_STRING_PROTO, [receiver]) => {
            intrinsic_sites::string_proto_name(builtin).map(|name| (name, Some(*receiver)))
        }
        (constants::INTRINSIC_FAMILY_ARRAY_PROTO, [receiver]) => {
            intrinsic_sites::array_proto_name(builtin).map(|name| (name, Some(*receiver)))
        }
        _ => None,
    };
    match resolved_name {
        Some((name, Some(receiver))) => resolve_member(ctx, state, receiver, name),
        Some((name, None)) => resolve_global(ctx, state, name),
        None => runtime::fail_dispatch(ctx),
    }
}

/// GlobalEnvGet（ResolveBinding + GetValue）：全局词法记录 → 全局对象属性
/// （getter 生效、含惰性内建合成），缺失名抛 ReferenceError。
fn resolve_global(ctx: &mut NativeVmContext, state: &mut NativeAgentState, name: &str) -> i64 {
    let Some(global) = state.global_object else {
        return runtime::fail_dispatch(ctx);
    };
    let Some(name_value) = state.intern_text(name.to_owned(), value::TAG_STRING) else {
        return runtime::fail_dispatch(ctx);
    };
    let flags = value::encode_f64(0.0);
    global_env::dispatch_global_env(
        ctx,
        state,
        Builtin::GlobalEnvGet,
        &[global, name_value, flags],
    )
    .unwrap_or_else(|| runtime::fail_dispatch(ctx))
}

/// 通用 [[Get]]（与 `Instruction::GetProp` 同一宿主入口：getter 以 receiver
/// 为 this 生效，基元 receiver 经包装原型链解析），返回属性值或异常哨兵。
fn resolve_member(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    name: &str,
) -> i64 {
    let Some(name_value) = state.intern_text(name.to_owned(), value::TAG_STRING) else {
        return runtime::fail_dispatch(ctx);
    };
    runtime::get_property(ctx, state, receiver, name_value)
        .unwrap_or_else(|()| runtime::fail_dispatch(ctx))
}
