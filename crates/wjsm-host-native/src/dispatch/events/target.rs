//! EventTarget 监听器登记与派发（WHATWG DOM §2.7 + Node v22 实测语义）。
//!
//! 监听器按 Node 的「每事件类型一条链表」建模：派发时逐节点在回调调用前
//! 缓存"下一节点"指针；解链只更新相邻节点、被解链节点保留自身 next，
//! 因此回调期间移除后续节点再追加新节点时，遍历沿被移除节点冻结的 next
//! 走到链尾（与 WHATWG 的快照克隆不同，按 Node 对齐）。节点存储的物理
//! 回收延后到派发深度归零。

use std::collections::HashMap;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{
    EventsCallable, TargetRef, event, fail_dispatch, invalid_this, is_object_like, modules,
    received_suffix, resolve_event_target, runtime, target_data_mut, target_object,
};
use crate::{NativeAgentState, NativeCallableKind};

/// 单个目标的事件监听器登记表 + `onabort` 事件处理器槽。
#[derive(Default)]
pub(crate) struct EventTargetData {
    /// 监听器节点存储：`None` 为空闲槽（下标进 `free` 复用）。
    nodes: Vec<Option<ListenerNode>>,
    free: Vec<u32>,
    /// 每个事件类型一条注册序链（Node `kEvents` 的链表形态）。
    chains: HashMap<String, Chain>,
    /// 派发期解链的节点：缓存的"下一节点"指针仍可能引用其 next，
    /// 深度归零后统一回收。
    retired: Vec<u32>,
    /// `onabort` 处理器当前值；`None` 表示从未赋值（包装监听器未挂接）。
    /// 首次赋值时在 abort 链当前尾部挂接包装槽位，之后仅替换值、位置
    /// 不变（HTML event handler 的 Node 形态，置 null 亦不摘除包装）。
    abort_handler: Option<i64>,
    /// 派发嵌套深度：>0 时解链节点先进 retired，物理回收延后。
    dispatch_depth: u32,
}

#[derive(Clone, Copy, Default)]
struct Chain {
    head: Option<u32>,
    tail: Option<u32>,
}

struct ListenerNode {
    /// 回调：函数或携带 handleEvent 的对象；handler 槽位不使用本字段。
    callback: i64,
    capture: bool,
    once: bool,
    /// onabort 事件处理器的包装槽位：调用时读 `abort_handler` 当前值。
    handler_slot: bool,
    removed: bool,
    /// 链内后继；解链时不清除（Node 语义：removed 节点冻结自身 next）。
    next: Option<u32>,
}

impl EventTargetData {
    /// 分配节点槽位（复用空闲下标）。
    fn alloc_node(&mut self, node: ListenerNode) -> Option<u32> {
        if let Some(index) = self.free.pop() {
            self.nodes[index as usize] = Some(node);
            return Some(index);
        }
        let index = u32::try_from(self.nodes.len()).ok()?;
        self.nodes.push(Some(node));
        Some(index)
    }

    /// 在事件类型链尾追加节点（§2.7.2 add an event listener 步骤 5）。
    fn append(&mut self, event_type: &str, node: ListenerNode) -> Option<()> {
        let index = self.alloc_node(node)?;
        let chain = self.chains.entry(event_type.to_owned()).or_default();
        match chain.tail {
            Some(tail) => {
                if let Some(Some(node)) = self.nodes.get_mut(tail as usize) {
                    node.next = Some(index);
                }
            }
            None => chain.head = Some(index),
        }
        chain.tail = Some(index);
        Some(())
    }

    /// 沿链查找 (callback, capture) 匹配的在链监听器（handler 槽位除外）。
    fn find_listener(&self, event_type: &str, callback_key: i64, capture: bool) -> Option<u32> {
        let mut cursor = self.chains.get(event_type)?.head;
        while let Some(index) = cursor {
            let node = self.nodes.get(index as usize)?.as_ref()?;
            if !node.handler_slot
                && node.capture == capture
                && value::strip_gc_color(node.callback) == callback_key
            {
                return Some(index);
            }
            cursor = node.next;
        }
        None
    }

    /// 解链：更新相邻节点与链首尾，节点保留自身 next 并打 removed 标记；
    /// 派发期外立即回收槽位，派发期内延后到深度归零。
    fn unlink(&mut self, event_type: &str, index: u32) {
        let Some(mut chain) = self.chains.get(event_type).copied() else {
            return;
        };
        let next = self
            .nodes
            .get(index as usize)
            .and_then(|node| node.as_ref())
            .and_then(|node| node.next);
        if chain.head == Some(index) {
            chain.head = next;
        } else {
            let mut cursor = chain.head;
            while let Some(current) = cursor {
                let Some(Some(node)) = self.nodes.get_mut(current as usize) else {
                    break;
                };
                if node.next == Some(index) {
                    node.next = next;
                    break;
                }
                cursor = node.next;
            }
        }
        if chain.tail == Some(index) {
            chain.tail = self.find_tail(chain.head);
        }
        self.chains.insert(event_type.to_owned(), chain);
        if let Some(Some(node)) = self.nodes.get_mut(index as usize) {
            node.removed = true;
        }
        self.retired.push(index);
        if self.dispatch_depth == 0 {
            self.reclaim_retired();
        }
    }

    fn find_tail(&self, head: Option<u32>) -> Option<u32> {
        let mut tail = None;
        let mut cursor = head;
        while let Some(index) = cursor {
            tail = Some(index);
            cursor = self
                .nodes
                .get(index as usize)
                .and_then(|node| node.as_ref())
                .and_then(|node| node.next);
        }
        tail
    }

    /// 回收全部已解链节点的槽位（仅在派发深度归零后调用）。
    fn reclaim_retired(&mut self) {
        for index in self.retired.drain(..) {
            if self.nodes.get(index as usize).is_some_and(Option::is_some) {
                self.nodes[index as usize] = None;
                self.free.push(index);
            }
        }
    }
}

/// 目标对象持有的 JS 值（监听器回调与 onabort 处理器）并入 GC 边图。
/// 已解链未回收的节点回调不再可调用，但缓存指针存续期内保持边无害。
pub(crate) fn extend_target_edges(
    data: &EventTargetData,
    owner: i64,
    add: &mut impl FnMut(i64, i64),
) {
    for node in data.nodes.iter().flatten() {
        if !node.handler_slot {
            add(owner, node.callback);
        }
    }
    if let Some(handler) = data.abort_handler {
        add(owner, handler);
    }
}

/// `EventTarget.prototype.addEventListener`（校验次序按 Node：实参数 →
/// options → listener → type 转换；nullish listener 静默忽略）。
pub(super) fn add_event_listener(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let Some(target) = resolve_event_target(state, this_value) else {
        return invalid_this(ctx, state, "EventTarget");
    };
    if args.len() < 2 {
        return missing_type_listener(ctx, state);
    }
    let (once, capture) = match add_options(ctx, state, args.get(2).copied()) {
        Ok(flags) => flags,
        Err(exception) => return exception,
    };
    let listener = args[1];
    match validate_listener(ctx, state, listener) {
        Ok(true) => {}
        Ok(false) => return value::encode_undefined(),
        Err(exception) => return exception,
    }
    let event_type = match event_type_string(ctx, state, args[0]) {
        Ok(event_type) => event_type,
        Err(exception) => return exception,
    };
    let callback_key = value::strip_gc_color(listener);
    let Some(data) = target_data_mut(state, target) else {
        return fail_dispatch(ctx);
    };
    // 去重键 (type, callback, capture)：已在链上则忽略本次注册
    //（once 等旗标不参与去重，§2.7.2 add an event listener 步骤 4）。
    if data
        .find_listener(&event_type, callback_key, capture)
        .is_none()
        && data
            .append(
                &event_type,
                ListenerNode {
                    callback: listener,
                    capture,
                    once,
                    handler_slot: false,
                    removed: false,
                    next: None,
                },
            )
            .is_none()
    {
        return fail_dispatch(ctx);
    }
    value::encode_undefined()
}

/// `EventTarget.prototype.removeEventListener`。capture 只认
/// `options.capture === true`（布尔捷径被忽略，Node 实测行为）。
pub(super) fn remove_event_listener(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let Some(target) = resolve_event_target(state, this_value) else {
        return invalid_this(ctx, state, "EventTarget");
    };
    if args.len() < 2 {
        return missing_type_listener(ctx, state);
    }
    let listener = args[1];
    match validate_listener(ctx, state, listener) {
        Ok(true) => {}
        Ok(false) => return value::encode_undefined(),
        Err(exception) => return exception,
    }
    let event_type = match event_type_string(ctx, state, args[0]) {
        Ok(event_type) => event_type,
        Err(exception) => return exception,
    };
    let capture = match remove_capture(ctx, state, args.get(2).copied()) {
        Ok(capture) => capture,
        Err(exception) => return exception,
    };
    let callback_key = value::strip_gc_color(listener);
    let Some(data) = target_data_mut(state, target) else {
        return fail_dispatch(ctx);
    };
    if let Some(index) = data.find_listener(&event_type, callback_key, capture) {
        data.unlink(&event_type, index);
    }
    value::encode_undefined()
}

/// `EventTarget.prototype.dispatchEvent`：返回 `!defaultPrevented`；
/// 同一事件派发中再次派发抛 ERR_EVENT_RECURSION（name 为 Error）。
pub(super) fn dispatch_event(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let Some(target) = resolve_event_target(state, this_value) else {
        return invalid_this(ctx, state, "EventTarget");
    };
    let Some(event_value) = args.first().copied() else {
        return runtime::type_error(ctx, state, "The \"event\" argument must be specified");
    };
    let Some(slot) = super::event_slot_of(state, event_value) else {
        let suffix = received_suffix(ctx, state, event_value);
        return runtime::type_error(
            ctx,
            state,
            &format!("The \"event\" argument must be an instance of Event. {suffix}"),
        );
    };
    let Some(event) = state.events.events.get_mut(slot) else {
        return fail_dispatch(ctx);
    };
    if event.dispatching {
        let message = format!(
            "The event \"{}\" is already being dispatched",
            event.event_type
        );
        // Node ERR_EVENT_RECURSION：name 为 Error，携带 code 自有属性。
        let exception = (|| {
            let error = modules::named_error_object(state, "Error", message)?;
            let code = state.intern_text("ERR_EVENT_RECURSION".into(), value::TAG_STRING)?;
            modules::set_named_property(state, error, "code", code).ok()?;
            state.create_exception(error)
        })();
        return exception.unwrap_or_else(|| fail_dispatch(ctx));
    }
    // §2.7.3 dispatchEvent 步骤 2：isTrusted 置 false。
    event.is_trusted = false;
    match fire_event(ctx, state, target, slot) {
        Ok(not_canceled) => value::encode_bool(not_canceled),
        Err(exception) => exception,
    }
}

/// `get onabort`：从未赋值或值为 null/undefined 时返回 null，否则原样返回
/// （Node 存任意值，仅派发时要求 callable）。
pub(super) fn onabort_get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
) -> i64 {
    let Some(target) = resolve_event_target(state, this_value) else {
        return onabort_invalid_this(ctx, state, this_value);
    };
    let Some(data) = target_data_mut(state, target) else {
        return fail_dispatch(ctx);
    };
    match data.abort_handler {
        Some(handler) if !value::is_nullish(handler) => handler,
        _ => value::encode_null(),
    }
}

/// `set onabort`：首次赋值时在监听器列表当前位置挂接包装槽位。
pub(super) fn onabort_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let Some(target) = resolve_event_target(state, this_value) else {
        return onabort_invalid_this(ctx, state, this_value);
    };
    let handler = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(data) = target_data_mut(state, target) else {
        return fail_dispatch(ctx);
    };
    if data.abort_handler.is_none()
        && data
            .append(
                "abort",
                ListenerNode {
                    callback: value::encode_undefined(),
                    capture: false,
                    once: false,
                    handler_slot: true,
                    removed: false,
                    next: None,
                },
            )
            .is_none()
    {
        return fail_dispatch(ctx);
    }
    data.abort_handler = Some(handler);
    value::encode_undefined()
}

/// onabort 访问器的品牌失败（Node ERR_INVALID_THIS 的 EventTarget 形态，
/// 消息含收到值的类型收据）。
fn onabort_invalid_this(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
) -> i64 {
    let suffix = received_suffix(ctx, state, this_value);
    runtime::type_error(
        ctx,
        state,
        &format!("The \"this\" argument must be an instance of EventTarget. {suffix}"),
    )
}

/// 在目标上派发事件：设置 target/currentTarget/AT_TARGET，按注册顺序调用
/// 匹配监听器（once 先摘除再调用，异常经 next-tick 重抛并继续），收尾复位
/// 派发期字段。返回 `!canceled`；Err 仅出现在宿主内部失败路径。
pub(super) fn fire_event(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetRef,
    event_slot: u32,
) -> Result<bool, i64> {
    let Some(target_value) = target_object(state, target) else {
        return Err(fail_dispatch(ctx));
    };
    let (event_object, event_type) = {
        let Some(event) = state.events.events.get_mut(event_slot) else {
            return Err(fail_dispatch(ctx));
        };
        event.dispatching = true;
        event.stop_immediate = false;
        event.event_phase = event::PHASE_AT_TARGET;
        event.target = target_value;
        event.current_target = target_value;
        (event.object, event.event_type.clone())
    };
    // 事件对象与目标仅由 Rust 局部持有，监听器执行可触发 GC，必须钉扎；
    // 待重抛的监听器错误值在入队前同样只由局部持有，也挂在本段根上。
    let roots_mark = state.temporary_roots.len();
    state.temporary_roots.push(event_object);
    state.temporary_roots.push(target_value);
    if let Some(data) = target_data_mut(state, target) {
        data.dispatch_depth += 1;
    }
    let listener_run = run_listeners(ctx, state, target, event_slot, event_object, &event_type);
    if let Some(data) = target_data_mut(state, target) {
        data.dispatch_depth -= 1;
        if data.dispatch_depth == 0 {
            data.reclaim_retired();
        }
    }
    let canceled = if let Some(event) = state.events.events.get_mut(event_slot) {
        event.dispatching = false;
        event.event_phase = event::PHASE_NONE;
        event.current_target = value::encode_null();
        event.stop_immediate = false;
        event.canceled
    } else {
        false
    };
    state.temporary_roots.truncate(roots_mark);
    listener_run?;
    Ok(!canceled)
}

/// 一次监听器遍历步骤的动作（借用期内快照，调用在借用外进行）。
enum StepAction {
    Skip,
    Invoke {
        callback: i64,
        handler_slot: bool,
        once: bool,
    },
}

/// 遍历匹配监听器并逐个调用；监听器异常按 Node 语义就地入队 next-tick
/// 重抛任务后继续遍历。Err 仅出现在重抛任务入队失败（如 async_hooks init
/// 钩子抛异常）时。
fn run_listeners(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetRef,
    event_slot: u32,
    event_object: i64,
    event_type: &str,
) -> Result<(), i64> {
    let mut cursor =
        target_data_mut(state, target).and_then(|data| data.chains.get(event_type)?.head);
    while let Some(index) = cursor {
        // Node 语义：每个 handler 调用前检查 stopImmediatePropagation 旗标。
        if state
            .events
            .events
            .get(event_slot)
            .is_none_or(|event| event.stop_immediate)
        {
            break;
        }
        // 调用回调前缓存"下一节点"：解链节点冻结自身 next，回调期间的链表
        // 变更按 Node 链表语义可见/不可见。
        let (cached_next, action) = {
            let Some(node) = target_data_mut(state, target)
                .and_then(|data| data.nodes.get(index as usize)?.as_ref())
            else {
                break;
            };
            let cached_next = node.next;
            let action = if node.removed {
                StepAction::Skip
            } else {
                StepAction::Invoke {
                    callback: node.callback,
                    handler_slot: node.handler_slot,
                    once: node.once,
                }
            };
            (cached_next, action)
        };
        if let StepAction::Invoke {
            callback,
            handler_slot,
            once,
        } = action
        {
            // once 监听器先摘除再调用（回调内可重新注册，§2.9 inner invoke）。
            if once && let Some(data) = target_data_mut(state, target) {
                data.unlink(event_type, index);
            }
            let result = invoke_listener(ctx, state, target, callback, handler_slot, event_object);
            if let Some(result) = result
                && value::is_exception(result)
            {
                schedule_listener_rethrow(ctx, state, result)?;
            }
        }
        cursor = cached_next;
    }
    Ok(())
}

/// 把监听器抛出的错误值入队为 next-tick 重抛任务（Node event_target 的
/// `process.nextTick(() => { throw err; })`，入队时机在监听器抛出当下，
/// 与用户后续 nextTick 的相对次序一致）。
fn schedule_listener_rethrow(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    exception: i64,
) -> Result<(), i64> {
    let error = state.exception_value(exception).unwrap_or(exception);
    // 入队過程有分配（TickObject 资源），错误值先钉扎（fire_event 收尾统一
    // 截断；入队后由 next_ticks 队列作为宿主根持有）。
    state.temporary_roots.push(error);
    let Some(callable) = state.native_callable(NativeCallableKind::Events(
        EventsCallable::RethrowListenerError,
    )) else {
        return Err(fail_dispatch(ctx));
    };
    let scheduled = super::super::promise::enqueue_next_tick(ctx, state, callable, vec![error]);
    if value::is_exception(scheduled) {
        return Err(scheduled);
    }
    Ok(())
}

/// 调用单个监听器：函数回调 this 为 currentTarget；对象监听器派发时查找
/// `handleEvent`（getter 可观察，非 callable 静默跳过）；handler 槽位读
/// onabort 当前值。返回 None 表示本步无调用。
fn invoke_listener(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetRef,
    callback: i64,
    handler_slot: bool,
    event_object: i64,
) -> Option<i64> {
    let target_value = target_object(state, target)?;
    if handler_slot {
        let handler = target_data_mut(state, target)?.abort_handler?;
        if !value::is_callable(handler) {
            return None;
        }
        return Some(
            state
                .invoke_callable(ctx, handler, target_value, &[event_object])
                .unwrap_or_else(|| fail_dispatch(ctx)),
        );
    }
    if value::is_callable(callback) {
        return Some(
            state
                .invoke_callable(ctx, callback, target_value, &[event_object])
                .unwrap_or_else(|| fail_dispatch(ctx)),
        );
    }
    let key = state.intern_text("handleEvent".into(), value::TAG_STRING)?;
    let handle_event = match runtime::get_property(ctx, state, callback, key) {
        Ok(handle_event) => handle_event,
        Err(()) => return Some(fail_dispatch(ctx)),
    };
    if value::is_exception(handle_event) {
        return Some(handle_event);
    }
    if !value::is_callable(handle_event) {
        return None;
    }
    Some(
        state
            .invoke_callable(ctx, handle_event, callback, &[event_object])
            .unwrap_or_else(|| fail_dispatch(ctx)),
    )
}

/// add/remove 缺参错误（Node ERR_MISSING_ARGS 形态）。
fn missing_type_listener(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    runtime::type_error(
        ctx,
        state,
        "The \"type\" and \"listener\" arguments must be specified",
    )
}

/// 监听器校验：函数/对象可注册（Ok(true)），null/undefined 静默忽略
/// （Ok(false)），其余抛 ERR_INVALID_ARG_TYPE。
fn validate_listener(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    listener: i64,
) -> Result<bool, i64> {
    if value::is_callable(listener)
        || value::is_js_object(listener)
        || value::is_array(listener)
        || value::is_proxy(listener)
    {
        return Ok(true);
    }
    if value::is_nullish(listener) {
        return Ok(false);
    }
    let suffix = received_suffix(ctx, state, listener);
    Err(runtime::type_error(
        ctx,
        state,
        &format!("The \"listener\" argument must be an instance of EventListener. {suffix}"),
    ))
}

/// 事件类型实参 → DOMString（Node webidl 转换器：symbol 抛
/// ERR_INVALID_ARG_VALUE，其余走 ToString）。
fn event_type_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<String, i64> {
    if value::is_symbol(encoded) {
        let rendered = runtime::render_value(state, encoded);
        return Err(runtime::type_error(
            ctx,
            state,
            &format!("The argument 'value' is invalid. Received {rendered}"),
        ));
    }
    runtime::to_string_coerced(ctx, state, encoded)
}

/// addEventListener 的 options 归一化：boolean → capture 捷径；
/// null/undefined → 缺省；对象/函数 → 依次读 once、capture（getter 可
/// 观察，真值化）；其余抛 ERR_INVALID_ARG_TYPE。返回 (once, capture)。
fn add_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: Option<i64>,
) -> Result<(bool, bool), i64> {
    let Some(options) = options.filter(|options| !value::is_nullish(*options)) else {
        return Ok((false, false));
    };
    if value::is_bool(options) {
        return Ok((false, value::decode_bool(options)));
    }
    if !is_object_like(options) {
        let suffix = received_suffix(ctx, state, options);
        return Err(runtime::type_error(
            ctx,
            state,
            &format!("The \"options\" argument must be of type object. {suffix}"),
        ));
    }
    let once = read_truthy_option(ctx, state, options, "once")?;
    let capture = read_truthy_option(ctx, state, options, "capture")?;
    Ok((once, capture))
}

/// removeEventListener 的 capture 提取：仅 `options.capture === true` 记
/// capture（布尔捷径与真值化都不生效，Node 实测）。
fn remove_capture(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: Option<i64>,
) -> Result<bool, i64> {
    let Some(options) = options.filter(|options| is_object_like(*options)) else {
        return Ok(false);
    };
    let key = state
        .intern_text("capture".into(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let capture =
        runtime::get_property(ctx, state, options, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(capture) {
        return Err(capture);
    }
    Ok(value::is_bool(capture) && value::decode_bool(capture))
}

fn read_truthy_option(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<bool, i64> {
    let key = state
        .intern_text(name.to_owned(), value::TAG_STRING)
        .ok_or_else(|| fail_dispatch(ctx))?;
    let stored =
        runtime::get_property(ctx, state, options, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(stored) {
        return Err(stored);
    }
    Ok(runtime::is_truthy(state, stored))
}
