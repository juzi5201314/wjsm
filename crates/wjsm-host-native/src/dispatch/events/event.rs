//! Event 对象（WHATWG DOM §2.5 的已实现子集）：构造、只读属性访问器与
//! 传播控制方法。无节点树，派发期 eventPhase 恒为 AT_TARGET。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{EventGetter, EventMethod, EventsCallable, fail_dispatch, received_suffix, runtime};
use crate::{NativeAgentState, NativeCallableKind};

pub(super) const PHASE_NONE: u8 = 0;
pub(super) const PHASE_AT_TARGET: u8 = 2;

pub(crate) struct EventState {
    pub(super) object: i64,
    pub(super) event_type: String,
    pub(super) bubbles: bool,
    pub(super) cancelable: bool,
    pub(super) composed: bool,
    /// canceled 旗标（defaultPrevented）；派发结束后保留。
    pub(super) canceled: bool,
    pub(super) stop_immediate: bool,
    pub(super) is_trusted: bool,
    pub(super) event_phase: u8,
    pub(super) dispatching: bool,
    /// 派发过的最近目标（派发结束后保留）；null 编码表示尚未派发。
    pub(super) target: i64,
    pub(super) current_target: i64,
    pub(super) time_stamp: f64,
}

/// `new Event(type, options?)`：type 必选（ToString，symbol 抛标准转换
/// 错误）；options 须为 nullish 或对象，成员依次读 bubbles / cancelable /
/// composed（getter 可观察，真值化）。
pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(type_value) = args.first().copied() else {
        return runtime::type_error(ctx, state, "The \"type\" argument must be specified");
    };
    let event_type = match runtime::to_string_coerced(ctx, state, type_value) {
        Ok(event_type) => event_type,
        Err(exception) => return exception,
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let mut flags = [false; 3];
    if !value::is_nullish(options) {
        if !super::is_object_like(options) {
            let suffix = received_suffix(ctx, state, options);
            return runtime::type_error(
                ctx,
                state,
                &format!("The \"options\" argument must be of type object. {suffix}"),
            );
        }
        for (index, name) in ["bubbles", "cancelable", "composed"].iter().enumerate() {
            let Some(key) = state.intern_text((*name).into(), value::TAG_STRING) else {
                return fail_dispatch(ctx);
            };
            let stored = match runtime::get_property(ctx, state, options, key) {
                Ok(stored) => stored,
                Err(()) => return fail_dispatch(ctx),
            };
            if value::is_exception(stored) {
                return stored;
            }
            flags[index] = runtime::is_truthy(state, stored);
        }
    }
    create(ctx, state, event_type, flags[0], flags[1], flags[2], false)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// 创建并登记 Event 实例（用户构造与宿主 fire 共用）。
pub(super) fn create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    event_type: String,
    bubbles: bool,
    cancelable: bool,
    composed: bool,
    is_trusted: bool,
) -> Option<i64> {
    let object = state.allocate_object_with_gc_retry(ctx, 0, false).ok()?;
    state
        .set_web_instance_prototype(object, Builtin::EventConstructor)
        .ok()?;
    let time_stamp = value::decode_f64(super::super::node_perf_hooks::performance_now(state));
    super::register_event(
        state,
        EventState {
            object,
            event_type,
            bubbles,
            cancelable,
            composed,
            canceled: false,
            stop_immediate: false,
            is_trusted,
            event_phase: PHASE_NONE,
            dispatching: false,
            target: value::encode_null(),
            current_target: value::encode_null(),
            time_stamp,
        },
    )
}

pub(super) fn getter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    getter: EventGetter,
    this_value: i64,
) -> i64 {
    let Some(slot) = super::event_slot_of(state, this_value) else {
        return super::invalid_this(ctx, state, "Event");
    };
    let Some(event) = state.events.events.get(slot) else {
        return fail_dispatch(ctx);
    };
    match getter {
        EventGetter::Type => {
            let event_type = event.event_type.clone();
            state
                .intern_text(event_type, value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        EventGetter::Target => event.target,
        EventGetter::CurrentTarget => event.current_target,
        EventGetter::EventPhase => value::encode_f64(f64::from(event.event_phase)),
        EventGetter::Bubbles => value::encode_bool(event.bubbles),
        EventGetter::Cancelable => value::encode_bool(event.cancelable),
        EventGetter::DefaultPrevented => value::encode_bool(event.canceled),
        EventGetter::Composed => value::encode_bool(event.composed),
        EventGetter::IsTrusted => value::encode_bool(event.is_trusted),
        EventGetter::TimeStamp => value::encode_f64(event.time_stamp),
    }
}

pub(super) fn method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: EventMethod,
    this_value: i64,
) -> i64 {
    let Some(slot) = super::event_slot_of(state, this_value) else {
        return super::invalid_this(ctx, state, "Event");
    };
    let Some(event) = state.events.events.get_mut(slot) else {
        return fail_dispatch(ctx);
    };
    match method {
        // 无节点树时传播路径长度为 1，stop propagation 旗标不可观察。
        EventMethod::StopPropagation => {}
        EventMethod::StopImmediatePropagation => event.stop_immediate = true,
        // §2.5：仅 cancelable 事件可置 canceled 旗标。
        EventMethod::PreventDefault => {
            if event.cancelable {
                event.canceled = true;
            }
        }
    }
    value::encode_undefined()
}

pub(super) fn getter_name(getter: EventGetter) -> &'static str {
    match getter {
        EventGetter::Type => "get type",
        EventGetter::Target => "get target",
        EventGetter::CurrentTarget => "get currentTarget",
        EventGetter::EventPhase => "get eventPhase",
        EventGetter::Bubbles => "get bubbles",
        EventGetter::Cancelable => "get cancelable",
        EventGetter::DefaultPrevented => "get defaultPrevented",
        EventGetter::Composed => "get composed",
        EventGetter::IsTrusted => "get isTrusted",
        EventGetter::TimeStamp => "get timeStamp",
    }
}

/// `Event.prototype` 成员（Node v22 的成员次序子集；isTrusted 为
/// { enumerable: true, configurable: false }，其余访问器/方法按 Web IDL
/// 缺省）。
pub(super) fn install_prototype_members(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Option<()> {
    for method in [
        EventMethod::StopImmediatePropagation,
        EventMethod::PreventDefault,
    ] {
        state.install_web_prototype_method(
            prototype,
            metadata_name(method),
            NativeCallableKind::Events(EventsCallable::EventMethod(method)),
        )?;
    }
    for getter in [
        EventGetter::Target,
        EventGetter::CurrentTarget,
        EventGetter::Type,
        EventGetter::Cancelable,
        EventGetter::DefaultPrevented,
        EventGetter::TimeStamp,
        EventGetter::Bubbles,
        EventGetter::Composed,
        EventGetter::EventPhase,
    ] {
        state.install_web_prototype_getter(
            prototype,
            getter_attribute_name(getter),
            NativeCallableKind::Events(EventsCallable::EventGetter(getter)),
        )?;
    }
    state.install_web_prototype_method(
        prototype,
        "stopPropagation",
        NativeCallableKind::Events(EventsCallable::EventMethod(EventMethod::StopPropagation)),
    )?;
    state.install_web_prototype_accessor_with_flags(
        prototype,
        "isTrusted",
        NativeCallableKind::Events(EventsCallable::EventGetter(EventGetter::IsTrusted)),
        None,
        wjsm_ir::constants::FLAG_ENUMERABLE as u32,
    )?;
    Some(())
}

fn metadata_name(method: EventMethod) -> &'static str {
    match method {
        EventMethod::StopPropagation => "stopPropagation",
        EventMethod::StopImmediatePropagation => "stopImmediatePropagation",
        EventMethod::PreventDefault => "preventDefault",
    }
}

fn getter_attribute_name(getter: EventGetter) -> &'static str {
    match getter {
        EventGetter::Type => "type",
        EventGetter::Target => "target",
        EventGetter::CurrentTarget => "currentTarget",
        EventGetter::EventPhase => "eventPhase",
        EventGetter::Bubbles => "bubbles",
        EventGetter::Cancelable => "cancelable",
        EventGetter::DefaultPrevented => "defaultPrevented",
        EventGetter::Composed => "composed",
        EventGetter::IsTrusted => "isTrusted",
        EventGetter::TimeStamp => "timeStamp",
    }
}
