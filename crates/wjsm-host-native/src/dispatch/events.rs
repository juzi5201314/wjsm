//! WHATWG DOM 事件基础设施的宿主实现（已实现子集）：全局 `EventTarget` /
//! `Event` 构造器、`AbortSignal` 接口对象与 abort 事件派发。
//!
//! 身份模型与 fetch/streams 一致：实例经 `objects` 登记品牌，方法/访问器
//! 安装在共享 prototype 上按实际 this 分派。AbortSignal 实例由 fetch 侧表
//! 拥有（AbortController 创建），本模块提供其 EventTarget 行为（监听器
//! 登记、abort 事件一次性派发）与 `AbortSignal.prototype` 成员；原型链为
//! `signal → AbortSignal.prototype → EventTarget.prototype → Object.prototype`。
//! 可观察行为（错误文案、options 归一化、派发期列表变更可见性）与 Node
//! v22 实测逐项对齐。

use std::collections::HashMap;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, fetch, modules, runtime};
use crate::slot_table::SlotTable;
use crate::{NativeAgentState, NativeCallableKind};

mod event;
mod target;

pub(crate) use event::EventState;
pub(crate) use target::EventTargetData;
pub(super) use target::extend_target_edges;

/// 事件宿主侧表：通用 EventTarget 实例与 Event 实例（AbortSignal 的监听器
/// 表内嵌在 fetch 侧的 `AbortSignalState` 中，品牌仍由 fetch `objects` 登记）。
#[derive(Default)]
pub(crate) struct NativeEventsState {
    objects: HashMap<u32, EventsObjectKind>,
    targets: SlotTable<EventTargetState>,
    events: SlotTable<EventState>,
}

#[derive(Clone, Copy)]
enum EventsObjectKind {
    EventTarget(u32),
    Event(u32),
}

pub(crate) struct EventTargetState {
    object: i64,
    data: EventTargetData,
}

/// 事件目标品牌：通用 `new EventTarget()` 实例或 fetch 侧的 AbortSignal。
#[derive(Clone, Copy)]
pub(super) enum TargetRef {
    Generic(u32),
    Signal(u32),
}

/// EventTarget / Event / AbortSignal 家族的方法与访问器可调用值（不携带
/// 实例句柄，按实际 this 分派）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EventsCallable {
    AddEventListener,
    RemoveEventListener,
    DispatchEvent,
    EventGetter(EventGetter),
    EventMethod(EventMethod),
    SignalAborted,
    SignalReason,
    SignalOnabortGet,
    SignalOnabortSet,
    SignalThrowIfAborted,
    /// 监听器异常的 next-tick 重抛任务（Node event_target 的
    /// `process.nextTick(() => { throw err; })`）：args\[0\] 为错误值。
    RethrowListenerError,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EventGetter {
    Type,
    Target,
    CurrentTarget,
    EventPhase,
    Bubbles,
    Cancelable,
    DefaultPrevented,
    Composed,
    IsTrusted,
    TimeStamp,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EventMethod {
    StopPropagation,
    StopImmediatePropagation,
    PreventDefault,
}

pub(super) fn dispatch_events(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::EventTargetConstructor => construct_event_target(ctx, state),
        // WHATWG DOM §3.2：AbortSignal 无构造器步骤，[[Construct]] 恒抛
        // TypeError（Node 文案 "Illegal constructor"）。
        Builtin::AbortSignalConstructor => runtime::type_error(ctx, state, "Illegal constructor"),
        Builtin::EventConstructor => event::construct(ctx, state, args),
        _ => return None,
    })
}

/// `new EventTarget()`：创建空监听器列表的事件目标（实参忽略）。
fn construct_event_target(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .set_web_instance_prototype(object, Builtin::EventTargetConstructor)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    let Some(slot) = state.events.targets.insert(EventTargetState {
        object,
        data: EventTargetData::default(),
    }) else {
        return fail_dispatch(ctx);
    };
    state
        .events
        .objects
        .insert(value::decode_handle(object), EventsObjectKind::EventTarget(slot));
    object
}

/// 把 Event 实例登记进侧表并返回堆对象（event 子模块创建状态后调用）。
fn register_event(state: &mut NativeAgentState, event: EventState) -> Option<i64> {
    let object = event.object;
    let slot = state.events.events.insert(event)?;
    state
        .events
        .objects
        .insert(value::decode_handle(object), EventsObjectKind::Event(slot));
    Some(object)
}

/// 按实际 this 解析事件目标品牌：通用 EventTarget 或 AbortSignal。
pub(super) fn resolve_event_target(
    state: &NativeAgentState,
    this_value: i64,
) -> Option<TargetRef> {
    if !value::is_js_object(this_value) {
        return None;
    }
    if let Some(EventsObjectKind::EventTarget(slot)) = state
        .events
        .objects
        .get(&value::decode_handle(this_value))
        .copied()
    {
        return Some(TargetRef::Generic(slot));
    }
    fetch::abort_signal_of(state, this_value).map(TargetRef::Signal)
}

/// 目标的监听器登记表（通用目标在本模块侧表，AbortSignal 在 fetch 侧表）。
pub(super) fn target_data_mut(
    state: &mut NativeAgentState,
    target: TargetRef,
) -> Option<&mut EventTargetData> {
    match target {
        TargetRef::Generic(slot) => state.events.targets.get_mut(slot).map(|entry| &mut entry.data),
        TargetRef::Signal(handle) => fetch::abort_signal_events_mut(state, handle),
    }
}

/// 目标的 JS 对象值。
pub(super) fn target_object(state: &NativeAgentState, target: TargetRef) -> Option<i64> {
    match target {
        TargetRef::Generic(slot) => state.events.targets.get(slot).map(|entry| entry.object),
        TargetRef::Signal(handle) => fetch::abort_signal_object(state, handle),
    }
}

/// 按实际 this 解析 Event 品牌。
pub(super) fn event_slot_of(state: &NativeAgentState, this_value: i64) -> Option<u32> {
    if !value::is_js_object(this_value) {
        return None;
    }
    match state
        .events
        .objects
        .get(&value::decode_handle(this_value))
        .copied()
    {
        Some(EventsObjectKind::Event(slot)) => Some(slot),
        _ => None,
    }
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: EventsCallable,
    this_value: i64,
    args: &[i64],
) -> i64 {
    match callable {
        EventsCallable::AddEventListener => target::add_event_listener(ctx, state, this_value, args),
        EventsCallable::RemoveEventListener => {
            target::remove_event_listener(ctx, state, this_value, args)
        }
        EventsCallable::DispatchEvent => target::dispatch_event(ctx, state, this_value, args),
        EventsCallable::EventGetter(getter) => event::getter(ctx, state, getter, this_value),
        EventsCallable::EventMethod(method) => event::method(ctx, state, method, this_value),
        EventsCallable::SignalAborted | EventsCallable::SignalReason => {
            let Some(handle) = fetch::abort_signal_of(state, this_value) else {
                return invalid_this(ctx, state, "AbortSignal");
            };
            let Some((aborted, reason)) = fetch::abort_signal_flags(state, handle) else {
                return fail_dispatch(ctx);
            };
            match callable {
                EventsCallable::SignalAborted => value::encode_bool(aborted),
                _ => reason,
            }
        }
        EventsCallable::SignalOnabortGet => target::onabort_get(ctx, state, this_value),
        EventsCallable::SignalOnabortSet => target::onabort_set(ctx, state, this_value, args),
        EventsCallable::SignalThrowIfAborted => throw_if_aborted(ctx, state, this_value),
        EventsCallable::RethrowListenerError => {
            let error = args.first().copied().unwrap_or_else(value::encode_undefined);
            state
                .create_exception(error)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
    }
}

/// `AbortSignal.prototype.throwIfAborted`（WHATWG DOM §3.2）：aborted 时把
/// reason 作为异常值原样抛出。
fn throw_if_aborted(ctx: &mut NativeVmContext, state: &mut NativeAgentState, this_value: i64) -> i64 {
    let Some(handle) = fetch::abort_signal_of(state, this_value) else {
        return invalid_this(ctx, state, "AbortSignal");
    };
    let Some((aborted, reason)) = fetch::abort_signal_flags(state, handle) else {
        return fail_dispatch(ctx);
    };
    if !aborted {
        return value::encode_undefined();
    }
    state
        .create_exception(reason)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// AbortController.prototype.abort 的 "signal abort" 事件步骤：以 isTrusted
/// 事件对象在 signal 上派发一次 `abort`（WHATWG DOM "fire an event"）。
/// 监听器异常经 next-tick 重抛（不中断 abort），返回 undefined；仅宿主
/// 内部失败返回异常。
pub(super) fn fire_abort_event(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    signal: u32,
) -> i64 {
    let Some(event_object) = event::create(ctx, state, "abort".into(), false, false, false, true)
    else {
        return fail_dispatch(ctx);
    };
    let Some(slot) = event_slot_of(state, event_object) else {
        return fail_dispatch(ctx);
    };
    match target::fire_event(ctx, state, TargetRef::Signal(signal), slot) {
        Ok(_) => value::encode_undefined(),
        Err(exception) => exception,
    }
}

/// 值是否可作字典/监听器对象参与属性读取（对象、数组、Proxy 或函数）。
pub(super) fn is_object_like(encoded: i64) -> bool {
    value::is_js_object(encoded)
        || value::is_array(encoded)
        || value::is_proxy(encoded)
        || value::is_callable(encoded)
}

/// Web IDL brand check 失败（Node ERR_INVALID_THIS 惯用格式）。
pub(super) fn invalid_this(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    interface: &str,
) -> i64 {
    runtime::type_error(
        ctx,
        state,
        &format!("Value of \"this\" must be of type {interface}"),
    )
}

/// Node `determineSpecificType` 的收据文案："Received undefined" /
/// "Received type number (42)" / "Received an instance of Object" 等，
/// 供 ERR_INVALID_ARG_TYPE 形态的错误消息复用。
pub(super) fn received_suffix(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> String {
    if value::is_undefined(encoded) {
        return "Received undefined".into();
    }
    if value::is_null(encoded) {
        return "Received null".into();
    }
    if value::is_callable(encoded) {
        let name = state.callable_js_name(encoded).unwrap_or_default();
        return format!("Received function {name}");
    }
    if value::is_js_object(encoded) || value::is_array(encoded) || value::is_proxy(encoded) {
        return format!(
            "Received an instance of {}",
            instance_label(ctx, state, encoded)
        );
    }
    if value::is_string(encoded) {
        // Node v22 对超过 28 字符的字符串内容截断为前 25 字符 + "..."
        //（引号内截断，实测 determineSpecificType 行为）。
        let text = runtime::render_value(state, encoded);
        let inspected = if text.chars().count() > 28 {
            format!("'{}...'", text.chars().take(25).collect::<String>())
        } else {
            format!("'{text}'")
        };
        return format!("Received type string ({inspected})");
    }
    let type_of = if value::is_bool(encoded) {
        "boolean"
    } else if value::is_f64(encoded) {
        "number"
    } else if value::is_bigint(encoded) {
        "bigint"
    } else {
        "symbol"
    };
    let mut rendered = runtime::render_value(state, encoded);
    if value::is_bigint(encoded) {
        rendered.push('n');
    }
    format!("Received type {type_of} ({rendered})")
}

/// 对象的构造器名（`value.constructor.name`，Node determineSpecificType 同款
/// 查找），读取失败时回落 "Object"。
fn instance_label(ctx: &mut NativeVmContext, state: &mut NativeAgentState, encoded: i64) -> String {
    let label = (|| {
        let constructor_key = state.intern_text("constructor".into(), value::TAG_STRING)?;
        let constructor = runtime::get_property(ctx, state, encoded, constructor_key).ok()?;
        if value::is_exception(constructor) || !value::is_callable(constructor) {
            return None;
        }
        let name_key = state.intern_text("name".into(), value::TAG_STRING)?;
        let name = runtime::get_property(ctx, state, constructor, name_key).ok()?;
        if value::is_exception(name) || !value::is_string(name) {
            return None;
        }
        state.string_owned(name)?.to_utf8()
    })();
    label.filter(|name| !name.is_empty()).unwrap_or_else(|| "Object".into())
}

/// 把已实现成员安装为共享 prototype 的自有属性；描述符逐项对齐 Node v22
/// 实测（AbortSignal 的 reason/throwIfAborted 不可枚举是 Node 实现细节，
/// 与 Web IDL 缺省不同，按 Node 对齐）。
pub(crate) fn install_prototype_members(
    state: &mut NativeAgentState,
    prototype: i64,
    builtin: Builtin,
) -> Option<()> {
    match builtin {
        Builtin::EventTargetConstructor => {
            for (name, callable) in [
                ("addEventListener", EventsCallable::AddEventListener),
                ("removeEventListener", EventsCallable::RemoveEventListener),
                ("dispatchEvent", EventsCallable::DispatchEvent),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    NativeCallableKind::Events(callable),
                )?;
            }
            install_to_string_tag(state, prototype, "EventTarget")?;
        }
        Builtin::AbortSignalConstructor => {
            state.install_web_prototype_getter(
                prototype,
                "aborted",
                NativeCallableKind::Events(EventsCallable::SignalAborted),
            )?;
            state.install_web_prototype_accessor_with_flags(
                prototype,
                "reason",
                NativeCallableKind::Events(EventsCallable::SignalReason),
                None,
                wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
            )?;
            state.install_web_prototype_method_with_flags(
                prototype,
                "throwIfAborted",
                NativeCallableKind::Events(EventsCallable::SignalThrowIfAborted),
                (wjsm_ir::constants::FLAG_WRITABLE | wjsm_ir::constants::FLAG_CONFIGURABLE) as u32,
            )?;
            state.install_web_prototype_accessor_with_flags(
                prototype,
                "onabort",
                NativeCallableKind::Events(EventsCallable::SignalOnabortGet),
                Some(NativeCallableKind::Events(EventsCallable::SignalOnabortSet)),
                (wjsm_ir::constants::FLAG_ENUMERABLE | wjsm_ir::constants::FLAG_CONFIGURABLE)
                    as u32,
            )?;
            install_to_string_tag(state, prototype, "AbortSignal")?;
        }
        Builtin::EventConstructor => {
            event::install_prototype_members(state, prototype)?;
            install_to_string_tag(state, prototype, "Event")?;
        }
        _ => {}
    }
    Some(())
}

/// `Symbol.toStringTag` 数据属性：{ writable: false, enumerable: false,
/// configurable: true }（Node/Web IDL 一致）。
fn install_to_string_tag(state: &mut NativeAgentState, prototype: i64, tag: &str) -> Option<()> {
    let key_value = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::TO_STRING_TAG);
    let key = runtime::property_key(state, key_value)?;
    let tag_value = state.intern_text(tag.into(), value::TAG_STRING)?;
    state
        .gc
        .heap()
        .define_data_property(
            value::decode_handle(prototype),
            key,
            tag_value as u64,
            wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
        )
        .ok()
}

/// 事件家族可调用值的 JS 可见 `(name, length)`（与 Node 实测一致）。
pub(crate) fn metadata(callable: EventsCallable) -> Option<(&'static str, u32)> {
    Some(match callable {
        EventsCallable::AddEventListener => ("addEventListener", 2),
        EventsCallable::RemoveEventListener => ("removeEventListener", 2),
        EventsCallable::DispatchEvent => ("dispatchEvent", 1),
        EventsCallable::EventGetter(getter) => (event::getter_name(getter), 0),
        EventsCallable::EventMethod(EventMethod::StopPropagation) => ("stopPropagation", 0),
        EventsCallable::EventMethod(EventMethod::StopImmediatePropagation) => {
            ("stopImmediatePropagation", 0)
        }
        EventsCallable::EventMethod(EventMethod::PreventDefault) => ("preventDefault", 0),
        EventsCallable::SignalAborted => ("get aborted", 0),
        EventsCallable::SignalReason => ("get reason", 0),
        EventsCallable::SignalOnabortGet => ("get onabort", 0),
        EventsCallable::SignalOnabortSet => ("set onabort", 1),
        EventsCallable::SignalThrowIfAborted => ("throwIfAborted", 0),
        // 内部 next-tick 重抛任务，对 JS 不可见（匿名箭头函数形态）。
        EventsCallable::RethrowListenerError => ("", 0),
    })
}

/// 把事件侧表持有的 JS 值按「owner 存活 ⇒ 内部引用存活」并入 GC 边图：
/// 目标对象持监听器回调与 onabort 处理器值，Event 对象持 target /
/// currentTarget（派发期间另有 temporary_roots 钉扎）。
pub(crate) fn extend_gc_edges(events: &NativeEventsState, mut add: impl FnMut(i64, i64)) {
    for (_, entry) in events.targets.iter() {
        target::extend_target_edges(&entry.data, entry.object, &mut add);
    }
    for (_, event) in events.events.iter() {
        if !value::is_null(event.target) {
            add(event.object, event.target);
        }
        if !value::is_null(event.current_target) {
            add(event.object, event.current_target);
        }
    }
}

/// GC 完成后按 retired 句柄清扫事件侧表：死包装对象的登记项与槽位一并
/// 释放，防止句柄复用后新对象继承旧品牌。
pub(crate) fn sweep_retired(events: &mut NativeEventsState, retired: &[u32]) {
    let NativeEventsState {
        objects,
        targets,
        events,
    } = events;
    objects.retain(|handle, kind| {
        if retired.binary_search(handle).is_err() {
            return true;
        }
        match kind {
            EventsObjectKind::EventTarget(slot) => {
                targets.remove(*slot);
            }
            EventsObjectKind::Event(slot) => {
                events.remove(*slot);
            }
        }
        false
    });
}
