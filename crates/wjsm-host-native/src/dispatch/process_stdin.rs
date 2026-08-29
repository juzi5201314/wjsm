// process.stdin 宿主实现：管道输入的真实按块读取（TASK-10）。
//
// 语义模型（Node lib/internal/streams/readable.js 的最小可观测面）：
// - flowing 模式：注册 'data' 监听或 resume() 后，事件循环 pump 按块交付
//   'data'，读尽后依次发 'end' / 'close'。
// - paused 模式：注册 'readable' 后 pump 发一次 'readable' 通知，read()
//   同步取走缓冲；缓冲耗尽再经一次 pump 补发 'readable'（EOF 通知）与
//   'end' / 'close'（与 Node v22 实测次序一致）。
// - 源读取一次到 EOF（管道路径确定性交付）；TTY 交互输入不在当前范围，
//   按空输入处理（见 docs/book/src/user/runtime/limitations.md）。
//
// 测试替身：`WJSM_TEST_STDIN`（进程 env 表，configure_environment 注入）
// 提供确定性输入内容，`WJSM_TEST_STDIN_CHUNK` 控制 'data' 分块大小以测试
// 跨块行拼接；设置替身时不做任何真实 fd 读取。

use std::io::{IsTerminal, Read};

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::node_async_hooks::{self, AsyncContextSnapshot};
use super::{fail_dispatch, modules, node_buffer, promise, runtime};
use crate::{NativeAgentState, NativeCallableKind};

/// Node 管道 stdin 的默认交付块大小（highWaterMark 64KiB）。
const DEFAULT_CHUNK_SIZE: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StdinMethod {
    On,
    Once,
    Off,
    Resume,
    Pause,
    IsPaused,
    Read,
    SetEncoding,
    AsyncIterator,
    IterNext,
    IterReturn,
}

pub(crate) fn method_metadata(method: StdinMethod) -> (&'static str, u32) {
    match method {
        StdinMethod::On => ("on", 2),
        StdinMethod::Once => ("once", 2),
        StdinMethod::Off => ("removeListener", 2),
        StdinMethod::Resume => ("resume", 0),
        StdinMethod::Pause => ("pause", 0),
        StdinMethod::IsPaused => ("isPaused", 0),
        StdinMethod::Read => ("read", 1),
        StdinMethod::SetEncoding => ("setEncoding", 1),
        StdinMethod::AsyncIterator => ("[Symbol.asyncIterator]", 0),
        StdinMethod::IterNext => ("next", 0),
        StdinMethod::IterReturn => ("return", 0),
    }
}

struct StdinListener {
    event: String,
    callable: i64,
    once: bool,
    context: AsyncContextSnapshot,
}

#[derive(Default)]
pub(crate) struct ProcessStdinState {
    /// process.stdin 宿主对象（创建后缓存，pump 以其为 this 调用监听器）。
    object: Option<i64>,
    listeners: Vec<StdinListener>,
    /// 一次性读尽的源内容与已消费游标。
    buffer: Vec<u8>,
    position: usize,
    /// 源是否已读取（管道读取一次到 EOF，读后恒为 EOF）。
    filled: bool,
    /// setEncoding('utf8') 后按字符边界切块并交付字符串。
    utf8: bool,
    flowing: bool,
    /// 显式 pause() 后置位；此后新增 'data' 监听不再自动恢复流动。
    paused: bool,
    pump_scheduled: bool,
    /// 数据可读的 'readable' 通知是否已发（EOF 通知单独判定）。
    readable_notified: bool,
    ended: bool,
    closed: bool,
}

impl ProcessStdinState {
    fn available(&self) -> usize {
        self.buffer.len() - self.position
    }

    fn has_listener(&self, event: &str) -> bool {
        self.listeners.iter().any(|entry| entry.event == event)
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pump_scheduled
    }

    pub(crate) fn extend_gc_roots(&self, queue: &mut std::collections::VecDeque<i64>) {
        queue.extend(self.object);
        queue.extend(self.listeners.iter().map(|entry| entry.callable));
    }
}

/// 创建 process.stdin 宿主对象并接线原生方法（ensure_process_object 调用）。
pub(crate) fn create_stdin_object(state: &mut NativeAgentState) -> Option<i64> {
    let stdin = state.allocate_object(16, false).ok()?;
    for (name, method) in [
        ("on", StdinMethod::On),
        ("addListener", StdinMethod::On),
        ("once", StdinMethod::Once),
        ("off", StdinMethod::Off),
        ("removeListener", StdinMethod::Off),
        ("resume", StdinMethod::Resume),
        ("pause", StdinMethod::Pause),
        ("isPaused", StdinMethod::IsPaused),
        ("read", StdinMethod::Read),
        ("setEncoding", StdinMethod::SetEncoding),
    ] {
        let callable = state.native_callable(NativeCallableKind::ProcessStdin(method))?;
        modules::set_named_property(state, stdin, name, callable).ok()?;
    }
    let async_iterator =
        state.native_callable(NativeCallableKind::ProcessStdin(StdinMethod::AsyncIterator))?;
    let key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ASYNC_ITERATOR);
    let key = runtime::property_key(state, key)?;
    state
        .gc
        .heap()
        .set_property(value::decode_handle(stdin), key, async_iterator as u64)
        .ok()?;
    modules::set_named_property(state, stdin, "fd", value::encode_f64(0.0)).ok()?;
    modules::set_named_property(state, stdin, "readable", value::encode_bool(true)).ok()?;
    // Node 仅在 fd 0 是终端时挂 isTTY=true（管道时属性缺席）；
    // 测试替身生效时按管道语义处理。
    if test_stdin_content(state).is_none() && std::io::stdin().is_terminal() {
        modules::set_named_property(state, stdin, "isTTY", value::encode_bool(true)).ok()?;
    }
    state.process_stdin.object = Some(stdin);
    Some(stdin)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: StdinMethod,
    this_value: i64,
    args: &[i64],
) -> i64 {
    match method {
        StdinMethod::On => add_listener(ctx, state, this_value, args, false),
        StdinMethod::Once => add_listener(ctx, state, this_value, args, true),
        StdinMethod::Off => remove_listener(state, this_value, args),
        StdinMethod::Resume => {
            state.process_stdin.paused = false;
            state.process_stdin.flowing = true;
            schedule_pump(state);
            this_value
        }
        StdinMethod::Pause => {
            state.process_stdin.paused = true;
            state.process_stdin.flowing = false;
            this_value
        }
        StdinMethod::IsPaused => value::encode_bool(state.process_stdin.paused),
        StdinMethod::Read => read(ctx, state, args),
        StdinMethod::SetEncoding => set_encoding(ctx, state, this_value, args),
        StdinMethod::AsyncIterator => create_async_iterator(ctx, state),
        StdinMethod::IterNext => iterator_next(ctx, state),
        StdinMethod::IterReturn => iterator_return(ctx, state),
    }
}

fn add_listener(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
    once: bool,
) -> i64 {
    let event = args
        .first()
        .and_then(|event| state.string_owned(*event))
        .and_then(|text| text.to_utf8())
        .unwrap_or_default();
    let callable = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if !state.is_callable_value(callable) {
        return type_error(ctx, state, "listener must be a function");
    }
    let context = node_async_hooks::capture_context(state);
    state.process_stdin.listeners.push(StdinListener {
        event: event.clone(),
        callable,
        once,
        context,
    });
    match event.as_str() {
        // Node：新增 'data' 监听即切换 flowing（显式 pause 后除外）。
        "data" => {
            if !state.process_stdin.paused {
                state.process_stdin.flowing = true;
            }
            schedule_pump(state);
        }
        "readable" => schedule_pump(state),
        _ => {}
    }
    this_value
}

fn remove_listener(state: &mut NativeAgentState, this_value: i64, args: &[i64]) -> i64 {
    let event = args
        .first()
        .and_then(|event| state.string_owned(*event))
        .and_then(|text| text.to_utf8())
        .unwrap_or_default();
    let callable = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    // 与 EventEmitter 一致：移除最后一个匹配项。
    if let Some(index) = state
        .process_stdin
        .listeners
        .iter()
        .rposition(|entry| entry.event == event && entry.callable == callable)
    {
        state.process_stdin.listeners.remove(index);
    }
    this_value
}

fn set_encoding(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let encoding = args
        .first()
        .and_then(|encoding| state.string_owned(*encoding))
        .and_then(|text| text.to_utf8())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if encoding != "utf8" && encoding != "utf-8" {
        return type_error(ctx, state, &format!("Unknown encoding: {encoding}"));
    }
    state.process_stdin.utf8 = true;
    this_value
}

/// paused 模式 read()：从缓冲同步取数据；未就绪/耗尽返回 null。
/// 首次调用（源未读）与耗尽后各调度一次 pump，使 'readable' / 'end'
/// 经事件循环按 Node 次序补发。
fn read(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if !state.process_stdin.filled {
        schedule_pump(state);
        return value::encode_null();
    }
    if state.process_stdin.available() == 0 {
        if !state.process_stdin.ended {
            schedule_pump(state);
        }
        return value::encode_null();
    }
    let requested = args
        .first()
        .and_then(|size| runtime::to_number(state, *size))
        .filter(|size| size.is_finite() && *size >= 0.0)
        .map(|size| size as usize);
    if state.process_stdin.utf8 {
        let remaining = decode_remaining(state);
        let text: String = match requested {
            // setEncoding 后 size 按字符计（Node string 模式缓冲语义）。
            Some(size) => remaining.chars().take(size).collect(),
            None => remaining,
        };
        // 非法 UTF-8 经替换符解码后字节数可能与源不一致，钳制游标防越界。
        state.process_stdin.position = state
            .process_stdin
            .buffer
            .len()
            .min(state.process_stdin.position + text.len());
        finish_read(state);
        return state
            .intern_text(text, value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let take = requested
        .unwrap_or(usize::MAX)
        .min(state.process_stdin.available());
    let start = state.process_stdin.position;
    let bytes = state.process_stdin.buffer[start..start + take].to_vec();
    state.process_stdin.position += take;
    finish_read(state);
    node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
}

fn decode_remaining(state: &NativeAgentState) -> String {
    let stdin = &state.process_stdin;
    String::from_utf8_lossy(&stdin.buffer[stdin.position..]).into_owned()
}

fn finish_read(state: &mut NativeAgentState) {
    if state.process_stdin.available() == 0 && !state.process_stdin.ended {
        schedule_pump(state);
    }
}

fn schedule_pump(state: &mut NativeAgentState) {
    if !state.process_stdin.ended || !state.process_stdin.closed {
        state.process_stdin.pump_scheduled = true;
    }
}

/// 测试替身内容：从进程环境表读 `WJSM_TEST_STDIN`（CLI 继承 env、
/// 进程内测试经 configure_environment 注入，两条路径同一钩子）。
fn test_stdin_content(state: &NativeAgentState) -> Option<Vec<u8>> {
    state
        .environment
        .get("WJSM_TEST_STDIN")
        .map(|content| content.clone().into_bytes())
}

fn chunk_size(state: &NativeAgentState) -> usize {
    state
        .environment
        .get("WJSM_TEST_STDIN_CHUNK")
        .and_then(|size| size.parse::<usize>().ok())
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_CHUNK_SIZE)
}

/// 一次性读尽输入源：测试替身优先；真实路径读管道到 EOF。
/// TTY 交互输入不在当前范围，按空输入（立即 EOF）处理。
fn fill_source(state: &mut NativeAgentState) {
    if state.process_stdin.filled {
        return;
    }
    state.process_stdin.filled = true;
    if let Some(bytes) = test_stdin_content(state) {
        state.process_stdin.buffer = bytes;
        return;
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return;
    }
    let mut bytes = Vec::new();
    if stdin.lock().read_to_end(&mut bytes).is_ok() {
        state.process_stdin.buffer = bytes;
    }
}

/// 事件循环外部事件阶段的交付泵：填充源、按模式交付数据与终结事件。
pub(crate) fn pump(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    if !state.process_stdin.pump_scheduled {
        return value::encode_undefined();
    }
    state.process_stdin.pump_scheduled = false;
    fill_source(state);
    // flowing 交付：有监听器则按块发 'data'；无监听器时数据按 Node 语义丢弃。
    while state.process_stdin.flowing
        && !state.process_stdin.ended
        && state.process_stdin.available() > 0
    {
        if !state.process_stdin.has_listener("data") {
            state.process_stdin.position = state.process_stdin.buffer.len();
            break;
        }
        let Some(chunk) = take_chunk(state) else {
            break;
        };
        let Some(encoded) = encode_chunk(state, chunk) else {
            return fail_dispatch(ctx);
        };
        if let Err(exception) = emit(ctx, state, "data", &[encoded]) {
            return exception;
        }
    }
    // paused 模式的数据可读通知。
    if !state.process_stdin.flowing
        && !state.process_stdin.ended
        && state.process_stdin.available() > 0
        && !state.process_stdin.readable_notified
        && state.process_stdin.has_listener("readable")
    {
        state.process_stdin.readable_notified = true;
        if let Err(exception) = emit(ctx, state, "readable", &[]) {
            return exception;
        }
    }
    // EOF 终结：缓冲耗尽后按 Node 次序发（paused 先补一次 'readable'）
    // 'end' / 'close'，并把 readable 属性翻为 false。
    if state.process_stdin.filled
        && state.process_stdin.available() == 0
        && !state.process_stdin.ended
    {
        if !state.process_stdin.flowing && state.process_stdin.has_listener("readable") {
            if let Err(exception) = emit(ctx, state, "readable", &[]) {
                return exception;
            }
        }
        state.process_stdin.ended = true;
        if let Some(stdin) = state.process_stdin.object {
            let _ = modules::set_named_property(state, stdin, "readable", value::encode_bool(false));
        }
        if let Err(exception) = emit(ctx, state, "end", &[]) {
            return exception;
        }
        if !state.process_stdin.closed {
            state.process_stdin.closed = true;
            if let Err(exception) = emit(ctx, state, "close", &[]) {
                return exception;
            }
        }
    }
    value::encode_undefined()
}

/// 从缓冲取一块；utf8 模式向前回退到字符边界，避免劈开多字节字符。
fn take_chunk(state: &mut NativeAgentState) -> Option<Vec<u8>> {
    let size = chunk_size(state);
    let stdin = &mut state.process_stdin;
    let available = stdin.buffer.len() - stdin.position;
    if available == 0 {
        return None;
    }
    let mut take = size.min(available);
    if stdin.utf8 && take < available {
        let mut adjusted = take;
        while adjusted > 0 && (stdin.buffer[stdin.position + adjusted] & 0xC0) == 0x80 {
            adjusted -= 1;
        }
        // 全是延续字节（非法 UTF-8）时按原样切，交付层做替换符解码。
        if adjusted > 0 {
            take = adjusted;
        }
    }
    let chunk = stdin.buffer[stdin.position..stdin.position + take].to_vec();
    stdin.position += take;
    Some(chunk)
}

fn encode_chunk(state: &mut NativeAgentState, chunk: Vec<u8>) -> Option<i64> {
    if state.process_stdin.utf8 {
        let text = String::from_utf8_lossy(&chunk).into_owned();
        state.intern_text(text, value::TAG_STRING)
    } else {
        node_buffer::from_bytes(state, chunk)
    }
}

/// 以 stdin 对象为 this 依次调用监听器；once 项先移除再调用（Node 语义）。
/// 监听器抛异常时立即上抛给事件循环。
fn emit(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    event: &str,
    args: &[i64],
) -> Result<(), i64> {
    let this_value = state
        .process_stdin
        .object
        .unwrap_or_else(value::encode_undefined);
    let invocations: Vec<(i64, AsyncContextSnapshot)> = state
        .process_stdin
        .listeners
        .iter()
        .filter(|entry| entry.event == event)
        .map(|entry| (entry.callable, entry.context.clone()))
        .collect();
    state
        .process_stdin
        .listeners
        .retain(|entry| !(entry.once && entry.event == event));
    for (callable, context) in invocations {
        let previous = node_async_hooks::enter_context(state, context);
        let result = state
            .invoke_callable(ctx, callable, this_value, args)
            .unwrap_or_else(|| fail_dispatch(ctx));
        node_async_hooks::restore_context(state, previous);
        if value::is_exception(result) {
            return Err(result);
        }
    }
    Ok(())
}

/// stdin[Symbol.asyncIterator]()：`for await (const chunk of process.stdin)`。
/// 迭代器共享底层流状态；next() 同步填源并以 resolved promise 交付。
fn create_async_iterator(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Ok(iterator) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    for (name, method) in [
        ("next", StdinMethod::IterNext),
        ("return", StdinMethod::IterReturn),
    ] {
        let Some(callable) = state.native_callable(NativeCallableKind::ProcessStdin(method)) else {
            return fail_dispatch(ctx);
        };
        if modules::set_named_property(state, iterator, name, callable).is_err() {
            return fail_dispatch(ctx);
        }
    }
    iterator
}

fn iterator_next(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    fill_source(state);
    if state.process_stdin.available() > 0 && !state.process_stdin.ended {
        let Some(chunk) = take_chunk(state) else {
            return fail_dispatch(ctx);
        };
        let Some(encoded) = encode_chunk(state, chunk) else {
            return fail_dispatch(ctx);
        };
        let Some(result) = iter_result(state, encoded, false) else {
            return fail_dispatch(ctx);
        };
        return promise::resolved_promise(ctx, state, result);
    }
    // 读尽：经 pump 按正常路径补发 'end' / 'close'。
    if !state.process_stdin.ended {
        schedule_pump(state);
    }
    let Some(result) = iter_result(state, value::encode_undefined(), true) else {
        return fail_dispatch(ctx);
    };
    promise::resolved_promise(ctx, state, result)
}

/// for-await break 触发 return()：丢弃剩余缓冲（Node destroy 的最小对应），
/// 不补发 'end'。
fn iterator_return(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    state.process_stdin.filled = true;
    state.process_stdin.position = state.process_stdin.buffer.len();
    state.process_stdin.ended = true;
    let Some(result) = iter_result(state, value::encode_undefined(), true) else {
        return fail_dispatch(ctx);
    };
    promise::resolved_promise(ctx, state, result)
}

fn iter_result(state: &mut NativeAgentState, stored: i64, done: bool) -> Option<i64> {
    let result = state.allocate_object(2, false).ok()?;
    modules::set_named_property(state, result, "value", stored).ok()?;
    modules::set_named_property(state, result, "done", value::encode_bool(done)).ok()?;
    Some(result)
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
