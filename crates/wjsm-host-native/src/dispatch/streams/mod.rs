use std::collections::{HashMap, VecDeque};

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use crate::slot_table::SlotTable;
use crate::{NativeAgentState, NativeCallableKind};

mod gc;
mod readable;
mod writable;

pub(crate) use gc::{extend_gc_edges, extend_gc_roots, extend_reaction_roots, extend_task_roots};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ReadableMethod {
    Cancel,
    GetReader,
    PipeThrough,
    PipeTo,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ReaderMethod {
    Read,
    ReleaseLock,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ControllerMethod {
    Close,
    Enqueue,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ByobMethod {
    Respond,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WritableMethod {
    Abort,
    Close,
    GetWriter,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WriterMethod {
    Abort,
    Close,
    ReleaseLock,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WritableControllerMethod {
    Error,
}

/// Streams 家族的方法/访问器可调用值。不携带实例句柄：调用时按实际
/// `this` 经 `objects` 登记表解析品牌，借用按 this 工作、同名方法在
/// 实例间身份相同，品牌不符抛 TypeError（reader/writer/controller 等
/// 无共享 prototype 的接口同样按 this 分派）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StreamCallable {
    AsyncIterator,
    AsyncIteratorNext,
    AsyncIteratorReturn,
    AsyncIteratorSelf,
    Byob(ByobMethod),
    Controller(ControllerMethod),
    QueuingSize(bool),
    Readable(ReadableMethod),
    ReadableLockedGetter,
    Reader(ReaderMethod),
    TransformReadableGetter,
    TransformWritableGetter,
    Writable(WritableMethod),
    WritableController(WritableControllerMethod),
    WritableLockedGetter,
    Writer(WriterMethod),
}

#[derive(Clone)]
pub(crate) enum StreamTask {
    CloseWritable {
        stream: u32,
        promise: u32,
    },
    Pull {
        controller: u32,
    },
    Pump {
        readable: u32,
    },
    Write {
        stream: u32,
        chunk: i64,
        promise: u32,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum StreamReaction {
    FinishClose { stream: u32, promise: u32 },
    Pump { readable: u32 },
}

#[derive(Clone, Copy)]
pub(crate) enum StreamProperty {
    Callable(StreamCallable),
    Value(i64),
}

#[derive(Clone, Copy)]
pub(super) enum ObjectKind {
    AsyncIterator(u32),
    Byob(u32),
    Controller(u32),
    Readable(u32),
    Reader(u32),
    Transform(u32),
    Writable(u32),
    WritableController(u32),
    Writer(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadableStatus {
    Closed,
    Errored,
    Readable,
}

pub(super) struct ReadableState {
    pub object: i64,
    pub controller: u32,
    pub status: ReadableStatus,
    pub error: Option<i64>,
    pub locked: bool,
    pub response: Option<u32>,
    pub pipe: Option<PipeState>,
}

pub(super) struct PipeState {
    pub destination: u32,
    pub promise: u32,
    pub writing: bool,
    pub closing: bool,
}

pub(super) struct ControllerState {
    pub object: i64,
    pub readable: u32,
    pub queue: VecDeque<i64>,
    pub high_water_mark: f64,
    pub close_requested: bool,
    pub byte_stream: bool,
    pub source: i64,
    pub pull: Option<i64>,
    pub cancel: Option<i64>,
    pub active_byob: Option<u32>,
    pub pulling: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReaderKind {
    Byob,
    Default,
}

pub(super) struct ReaderState {
    /// 包装对象；供 GC 边图维系 reader ↔ stream 与挂起 promise 的存活。
    pub object: i64,
    pub stream: u32,
    pub kind: ReaderKind,
    pub closed_promise: u32,
    pub pending: VecDeque<PendingRead>,
}
#[derive(Clone, Copy)]
pub(super) struct PendingRead {
    pub promise: u32,
    pub view: Option<i64>,
}

pub(super) struct ByobState {
    pub object: i64,
    pub controller: u32,
    pub reader: u32,
    pub view: i64,
    pub promise: u32,
    pub responded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WritableStatus {
    Closed,
    Closing,
    Errored,
    Writable,
}

pub(super) struct WritableState {
    pub object: i64,
    pub controller: u32,
    pub status: WritableStatus,
    pub locked: bool,
    pub transform: Option<u32>,
    pub pipe_source: Option<u32>,
}

pub(super) struct WritableControllerState {
    pub object: i64,
    pub stream: u32,
    pub sink: i64,
    pub write: Option<i64>,
    pub close: Option<i64>,
    pub abort: Option<i64>,
    pub signal: i64,
}

pub(super) struct WriterState {
    /// 包装对象；供 GC 边图维系 writer ↔ stream 与挂起 promise 的存活。
    pub object: i64,
    pub stream: u32,
    pub closed_promise: u32,
    pub ready_promise: u32,
}

pub(super) struct TransformState {
    /// 包装对象；writable 写路径按槽位引用 transform，须经边图钉住。
    pub object: i64,
    pub readable: u32,
    pub writable: u32,
    pub controller: u32,
    pub transformer: i64,
    pub transform: Option<i64>,
    pub flush: Option<i64>,
}

pub(super) struct AsyncIteratorState {
    pub object: i64,
    pub reader: u32,
}

#[derive(Default)]
pub(crate) struct NativeStreamsState {
    pub(super) objects: HashMap<u32, ObjectKind>,
    pub(super) readables: SlotTable<ReadableState>,
    pub(super) controllers: SlotTable<ControllerState>,
    pub(super) readers: SlotTable<ReaderState>,
    pub(super) byob_requests: SlotTable<ByobState>,
    pub(super) writables: SlotTable<WritableState>,
    pub(super) writable_controllers: SlotTable<WritableControllerState>,
    pub(super) writers: SlotTable<WriterState>,
    pub(super) transforms: SlotTable<TransformState>,
    pub(super) async_iterators: SlotTable<AsyncIteratorState>,
    /// node:stream/web 桥对象缓存（Web Streams 构造器的可调用值集合）。
    pub(super) web_bridge: Option<i64>,
}

impl NativeStreamsState {
    /// 是否仍有存活的 body 流按下标引用指定 fetch response 槽位
    /// （resource timing 完成路径）。fetch 清扫据此延迟释放 response 槽。
    pub(crate) fn body_stream_references_response(&self, response: u32) -> bool {
        self.readables
            .iter()
            .any(|(_, readable)| readable.response == Some(response))
    }

    /// 活包装对象数（`objects` 登记表）。
    #[cfg(test)]
    pub(crate) fn live_object_count(&self) -> usize {
        self.objects.len()
    }

    /// 各内部侧表的活槽总数。
    #[cfg(test)]
    pub(crate) fn live_slot_count(&self) -> usize {
        self.readables.len()
            + self.controllers.len()
            + self.readers.len()
            + self.byob_requests.len()
            + self.writables.len()
            + self.writable_controllers.len()
            + self.writers.len()
            + self.transforms.len()
            + self.async_iterators.len()
    }
}

/// GC 完成后按 retired 句柄清扫 streams 侧表：死包装对象的登记项与槽位
/// 一并释放。宿主边图保证「存活槽位可达的所有槽位其包装对象也存活」，
/// 因此仅按包装对象死活释放槽位不会悬空存活路径上的交叉下标。
pub(crate) fn sweep_retired(streams: &mut NativeStreamsState, retired: &[u32]) {
    let NativeStreamsState {
        objects,
        readables,
        controllers,
        readers,
        byob_requests,
        writables,
        writable_controllers,
        writers,
        transforms,
        async_iterators,
        ..
    } = streams;
    objects.retain(|handle, kind| {
        if retired.binary_search(handle).is_err() {
            return true;
        }
        match kind {
            ObjectKind::AsyncIterator(slot) => {
                async_iterators.remove(*slot);
            }
            ObjectKind::Byob(slot) => {
                byob_requests.remove(*slot);
            }
            ObjectKind::Controller(slot) => {
                controllers.remove(*slot);
            }
            ObjectKind::Readable(slot) => {
                readables.remove(*slot);
            }
            ObjectKind::Reader(slot) => {
                readers.remove(*slot);
            }
            ObjectKind::Transform(slot) => {
                transforms.remove(*slot);
            }
            ObjectKind::Writable(slot) => {
                writables.remove(*slot);
            }
            ObjectKind::WritableController(slot) => {
                writable_controllers.remove(*slot);
            }
            ObjectKind::Writer(slot) => {
                writers.remove(*slot);
            }
        }
        false
    });
}

pub(super) fn register_object(state: &mut NativeAgentState, object: i64, kind: ObjectKind) {
    state
        .streams
        .objects
        .insert(value::decode_handle(object), kind);
}

pub(super) fn new_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> Option<(i64, u32)> {
    let promise = super::promise::new_promise(ctx, state)?;
    Some((promise, value::decode_handle(promise)))
}

pub(super) fn resolved(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stored: i64,
) -> i64 {
    super::promise::resolved_promise(ctx, state, stored)
}

pub(super) fn result_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    done: bool,
    stored: i64,
) -> i64 {
    // result 对象分配可触发 GC，value（可能刚从队列弹出）仅由局部值持有，
    // 须钉扎。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(stored);
    let result = state.allocate_object_with_gc_retry(ctx, 2, false);
    state.temporary_roots.truncate(initial_temp_roots);
    let Ok(result) = result else {
        return super::fail_dispatch(ctx);
    };
    if super::modules::set_named_property(state, result, "done", value::encode_bool(done)).is_err()
        || super::modules::set_named_property(state, result, "value", stored).is_err()
    {
        return super::fail_dispatch(ctx);
    }
    result
}

pub(super) fn read_named(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> i64 {
    let Some(key) = state.intern_text(name.to_owned(), value::TAG_STRING) else {
        return super::fail_dispatch(ctx);
    };
    super::runtime::get_property(ctx, state, object, key)
        .unwrap_or_else(|()| super::fail_dispatch(ctx))
}

pub(super) fn callable_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Option<i64> {
    let callable = read_named(ctx, state, object, name);
    value::is_callable(callable).then_some(callable)
}

pub(super) fn type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    super::modules::named_error_object(state, "TypeError", message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| super::fail_dispatch(ctx))
}

/// node:stream/web 桥：把 Web Streams 构造器以可调用值形式暴露给 builtin JS。
/// 与调用点拦截命中的 `dispatch_streams` 是同一实现，只是取值形态不同。
pub(crate) fn ensure_web_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.streams.web_bridge {
        return Some(bridge);
    }
    let constructors = [
        ("ReadableStream", Builtin::ReadableStreamConstructor),
        ("WritableStream", Builtin::WritableStreamConstructor),
        ("TransformStream", Builtin::TransformStreamConstructor),
        (
            "CountQueuingStrategy",
            Builtin::CountQueuingStrategyConstructor,
        ),
        (
            "ByteLengthQueuingStrategy",
            Builtin::ByteLengthQueuingStrategyConstructor,
        ),
    ];
    let bridge = state
        .allocate_object(constructors.len() as u32, false)
        .ok()?;
    for (name, builtin) in constructors {
        let callable = state.native_callable(crate::NativeCallableKind::Builtin(builtin, false))?;
        super::modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.streams.web_bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn dispatch_streams(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ReadableStreamConstructor => readable::construct(ctx, state, args),
        Builtin::WritableStreamConstructor => writable::construct(ctx, state, args),
        Builtin::TransformStreamConstructor => writable::construct_transform(ctx, state, args),
        Builtin::CountQueuingStrategyConstructor => queuing_strategy(ctx, state, args, false),
        Builtin::ByteLengthQueuingStrategyConstructor => queuing_strategy(ctx, state, args, true),
        _ => return None,
    })
}

/// 按实际 `this` 解析品牌：非对象或未登记为 stream 包装对象时返回 None。
fn this_object_kind(state: &NativeAgentState, this_value: i64) -> Option<ObjectKind> {
    if !value::is_js_object(this_value) {
        return None;
    }
    state
        .streams
        .objects
        .get(&value::decode_handle(this_value))
        .copied()
}

/// 同步形态的品牌失败：`TypeError: Value of "this" must be of type X`
///（与 Node 的 ERR_INVALID_THIS 逐字节一致）。
fn invalid_this(ctx: &mut NativeVmContext, state: &mut NativeAgentState, interface: &str) -> i64 {
    type_error(
        ctx,
        state,
        &format!("Value of \"this\" must be of type {interface}"),
    )
}

/// promise 形态方法的品牌失败：以 rejected promise 交付同文案的
/// TypeError（与 Node 一致：async 方法品牌失败不同步抛出）。
fn invalid_this_rejection(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    interface: &str,
) -> i64 {
    let Some(reason) = super::modules::named_error_object(
        state,
        "TypeError",
        format!("Value of \"this\" must be of type {interface}"),
    ) else {
        return super::fail_dispatch(ctx);
    };
    super::promise::rejected_promise(ctx, state, reason)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: StreamCallable,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let kind = this_object_kind(state, this_value);
    match callable {
        StreamCallable::Readable(method) => match kind {
            Some(ObjectKind::Readable(handle)) => {
                readable::call_readable(ctx, state, handle, method, args)
            }
            _ => match method {
                ReadableMethod::Cancel | ReadableMethod::PipeTo => {
                    invalid_this_rejection(ctx, state, "ReadableStream")
                }
                ReadableMethod::GetReader | ReadableMethod::PipeThrough => {
                    invalid_this(ctx, state, "ReadableStream")
                }
            },
        },
        StreamCallable::ReadableLockedGetter => match kind {
            Some(ObjectKind::Readable(handle)) => state
                .streams
                .readables
                .get(handle)
                .map(|stream| value::encode_bool(stream.locked))
                .unwrap_or_else(|| super::fail_dispatch(ctx)),
            _ => invalid_this(ctx, state, "ReadableStream"),
        },
        StreamCallable::Reader(method) => match kind {
            Some(ObjectKind::Reader(handle)) => {
                readable::call_reader(ctx, state, handle, method, args)
            }
            _ => match method {
                ReaderMethod::Read => {
                    invalid_this_rejection(ctx, state, "ReadableStreamDefaultReader")
                }
                ReaderMethod::ReleaseLock => {
                    invalid_this(ctx, state, "ReadableStreamDefaultReader")
                }
            },
        },
        StreamCallable::Controller(method) => match kind {
            Some(ObjectKind::Controller(handle)) => {
                readable::call_controller(ctx, state, handle, method, args)
            }
            _ => invalid_this(ctx, state, "ReadableStreamDefaultController"),
        },
        StreamCallable::Byob(method) => match kind {
            Some(ObjectKind::Byob(handle)) => readable::call_byob(ctx, state, handle, method, args),
            _ => invalid_this(ctx, state, "ReadableStreamBYOBRequest"),
        },
        StreamCallable::AsyncIterator => match kind {
            Some(ObjectKind::Readable(handle)) => {
                readable::create_async_iterator(ctx, state, handle)
            }
            _ => invalid_this(ctx, state, "ReadableStream"),
        },
        StreamCallable::AsyncIteratorNext => match kind {
            Some(ObjectKind::AsyncIterator(handle)) => {
                readable::async_iterator_next(ctx, state, handle)
            }
            _ => invalid_this_rejection(ctx, state, "ReadableStreamAsyncIterator"),
        },
        StreamCallable::AsyncIteratorReturn => match kind {
            Some(ObjectKind::AsyncIterator(handle)) => {
                readable::async_iterator_return(ctx, state, handle)
            }
            _ => invalid_this_rejection(ctx, state, "ReadableStreamAsyncIterator"),
        },
        StreamCallable::AsyncIteratorSelf => match kind {
            Some(ObjectKind::AsyncIterator(_)) => this_value,
            _ => invalid_this(ctx, state, "ReadableStreamAsyncIterator"),
        },
        StreamCallable::Writable(method) => match kind {
            Some(ObjectKind::Writable(handle)) => {
                writable::call_writable(ctx, state, handle, method, args)
            }
            _ => match method {
                WritableMethod::Abort | WritableMethod::Close => {
                    invalid_this_rejection(ctx, state, "WritableStream")
                }
                WritableMethod::GetWriter => invalid_this(ctx, state, "WritableStream"),
            },
        },
        StreamCallable::WritableLockedGetter => match kind {
            Some(ObjectKind::Writable(handle)) => state
                .streams
                .writables
                .get(handle)
                .map(|stream| value::encode_bool(stream.locked))
                .unwrap_or_else(|| super::fail_dispatch(ctx)),
            _ => invalid_this(ctx, state, "WritableStream"),
        },
        StreamCallable::Writer(method) => match kind {
            Some(ObjectKind::Writer(handle)) => {
                writable::call_writer(ctx, state, handle, method, args)
            }
            _ => match method {
                WriterMethod::ReleaseLock => {
                    invalid_this(ctx, state, "WritableStreamDefaultWriter")
                }
                WriterMethod::Abort | WriterMethod::Close | WriterMethod::Write => {
                    invalid_this_rejection(ctx, state, "WritableStreamDefaultWriter")
                }
            },
        },
        StreamCallable::WritableController(method) => match kind {
            Some(ObjectKind::WritableController(handle)) => {
                writable::call_controller(ctx, state, handle, method, args)
            }
            _ => invalid_this(ctx, state, "WritableStreamDefaultController"),
        },
        StreamCallable::TransformReadableGetter => match kind {
            Some(ObjectKind::Transform(handle)) => transform_end(ctx, state, handle, true),
            _ => invalid_this(ctx, state, "TransformStream"),
        },
        StreamCallable::TransformWritableGetter => match kind {
            Some(ObjectKind::Transform(handle)) => transform_end(ctx, state, handle, false),
            _ => invalid_this(ctx, state, "TransformStream"),
        },
        StreamCallable::QueuingSize(byte_length) => queuing_size(ctx, state, args, byte_length),
    }
}

/// TransformStream 两端包装对象的取值（readable=true 取读端）。
fn transform_end(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    readable: bool,
) -> i64 {
    state
        .streams
        .transforms
        .get(handle)
        .and_then(|transform| {
            if readable {
                state
                    .streams
                    .readables
                    .get(transform.readable)
                    .map(|stream| stream.object)
            } else {
                state
                    .streams
                    .writables
                    .get(transform.writable)
                    .map(|stream| stream.object)
            }
        })
        .unwrap_or_else(|| super::fail_dispatch(ctx))
}

/// 无共享 prototype 的接口（reader/writer/controller/BYOB request/异步
/// 迭代器）仍经虚拟属性解析；ReadableStream/WritableStream/TransformStream
/// 的成员已全部安装到对应 `prototype` 对象，不再走此路径。
pub(crate) fn property(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<StreamProperty> {
    let kind = *state.streams.objects.get(&value::decode_handle(receiver))?;
    match kind {
        ObjectKind::Reader(handle) => readable::reader_property(state, handle, key),
        ObjectKind::Controller(handle) => readable::controller_property(state, handle, key),
        ObjectKind::Byob(handle) => readable::byob_property(state, handle, key),
        ObjectKind::AsyncIterator(_) => readable::async_iterator_property(key),
        ObjectKind::Writer(handle) => writable::writer_property(state, handle, key),
        ObjectKind::WritableController(handle) => writable::controller_property(state, handle, key),
        ObjectKind::Readable(_) | ObjectKind::Writable(_) | ObjectKind::Transform(_) => None,
    }
}

pub(crate) fn async_iterator_property(
    state: &NativeAgentState,
    receiver: i64,
) -> Option<StreamCallable> {
    match state.streams.objects.get(&value::decode_handle(receiver))? {
        ObjectKind::Readable(_) => Some(StreamCallable::AsyncIterator),
        ObjectKind::AsyncIterator(_) => Some(StreamCallable::AsyncIteratorSelf),
        _ => None,
    }
}

/// 把已实现的方法/访问器安装为对应 `prototype` 对象的自有属性（Web IDL
/// 描述符：方法 {writable, enumerable, configurable}，访问器
/// {enumerable, configurable}），次序与 Node 一致。
pub(crate) fn install_prototype_members(
    state: &mut NativeAgentState,
    prototype: i64,
    builtin: Builtin,
) -> Option<()> {
    match builtin {
        Builtin::ReadableStreamConstructor => {
            state.install_web_prototype_getter(
                prototype,
                "locked",
                crate::NativeCallableKind::Stream(StreamCallable::ReadableLockedGetter),
            )?;
            for (name, method) in [
                ("cancel", ReadableMethod::Cancel),
                ("getReader", ReadableMethod::GetReader),
                ("pipeThrough", ReadableMethod::PipeThrough),
                ("pipeTo", ReadableMethod::PipeTo),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    crate::NativeCallableKind::Stream(StreamCallable::Readable(method)),
                )?;
            }
        }
        Builtin::WritableStreamConstructor => {
            state.install_web_prototype_getter(
                prototype,
                "locked",
                crate::NativeCallableKind::Stream(StreamCallable::WritableLockedGetter),
            )?;
            for (name, method) in [
                ("abort", WritableMethod::Abort),
                ("close", WritableMethod::Close),
                ("getWriter", WritableMethod::GetWriter),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    crate::NativeCallableKind::Stream(StreamCallable::Writable(method)),
                )?;
            }
        }
        Builtin::TransformStreamConstructor => {
            state.install_web_prototype_getter(
                prototype,
                "readable",
                crate::NativeCallableKind::Stream(StreamCallable::TransformReadableGetter),
            )?;
            state.install_web_prototype_getter(
                prototype,
                "writable",
                crate::NativeCallableKind::Stream(StreamCallable::TransformWritableGetter),
            )?;
        }
        _ => {}
    }
    Some(())
}

/// Streams 家族可调用值的 JS 可见 `(name, length)`（与 Node 实测一致；
/// 访问器 name 为 `get <attr>` 形态，@@asyncIterator 与 `values` 共享
/// 函数身份故 name 为 `values`）。
pub(crate) fn metadata(callable: StreamCallable) -> Option<(&'static str, u32)> {
    Some(match callable {
        StreamCallable::AsyncIterator => ("values", 0),
        StreamCallable::AsyncIteratorNext => ("next", 0),
        StreamCallable::AsyncIteratorReturn => ("return", 0),
        StreamCallable::AsyncIteratorSelf => ("[Symbol.asyncIterator]", 0),
        StreamCallable::Byob(ByobMethod::Respond) => ("respond", 1),
        StreamCallable::Controller(ControllerMethod::Close) => ("close", 0),
        StreamCallable::Controller(ControllerMethod::Enqueue) => ("enqueue", 0),
        StreamCallable::Controller(ControllerMethod::Error) => ("error", 0),
        StreamCallable::QueuingSize(false) => ("size", 0),
        StreamCallable::QueuingSize(true) => ("size", 1),
        StreamCallable::Readable(ReadableMethod::Cancel) => ("cancel", 0),
        StreamCallable::Readable(ReadableMethod::GetReader) => ("getReader", 0),
        StreamCallable::Readable(ReadableMethod::PipeThrough) => ("pipeThrough", 1),
        StreamCallable::Readable(ReadableMethod::PipeTo) => ("pipeTo", 1),
        StreamCallable::ReadableLockedGetter => ("get locked", 0),
        StreamCallable::Reader(ReaderMethod::Read) => ("read", 0),
        StreamCallable::Reader(ReaderMethod::ReleaseLock) => ("releaseLock", 0),
        StreamCallable::TransformReadableGetter => ("get readable", 0),
        StreamCallable::TransformWritableGetter => ("get writable", 0),
        StreamCallable::Writable(WritableMethod::Abort) => ("abort", 0),
        StreamCallable::Writable(WritableMethod::Close) => ("close", 0),
        StreamCallable::Writable(WritableMethod::GetWriter) => ("getWriter", 0),
        StreamCallable::WritableController(WritableControllerMethod::Error) => ("error", 0),
        StreamCallable::WritableLockedGetter => ("get locked", 0),
        StreamCallable::Writer(WriterMethod::Abort) => ("abort", 0),
        StreamCallable::Writer(WriterMethod::Close) => ("close", 0),
        StreamCallable::Writer(WriterMethod::ReleaseLock) => ("releaseLock", 0),
        StreamCallable::Writer(WriterMethod::Write) => ("write", 0),
    })
}

pub(crate) fn run_task(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    task: StreamTask,
) -> i64 {
    // 任务出队后不再被队列根覆盖，执行期间再入 JS 可触发 GC；先按任务根
    // 钉扎涉及的包装对象与 promise，防止执行中被清扫或句柄复用。
    let initial_temp_roots = state.temporary_roots.len();
    let mut roots = VecDeque::new();
    gc::extend_task_roots(&state.streams, &task, &mut roots);
    state.temporary_roots.extend(roots);
    let result = match task {
        StreamTask::Pull { controller } => readable::run_pull(ctx, state, controller),
        StreamTask::Pump { readable } => readable::pump(ctx, state, readable),
        StreamTask::Write {
            stream,
            chunk,
            promise,
        } => writable::run_write(ctx, state, stream, chunk, promise),
        StreamTask::CloseWritable { stream, promise } => {
            writable::run_close(ctx, state, stream, promise)
        }
    };
    state.temporary_roots.truncate(initial_temp_roots);
    result
}

pub(crate) fn run_reaction(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    reaction: StreamReaction,
    value: i64,
    rejected: bool,
) -> i64 {
    // 反应触发后已从 promise 反应表摘除，执行期间同样须按根钉扎。
    let initial_temp_roots = state.temporary_roots.len();
    let mut roots = VecDeque::new();
    gc::extend_reaction_roots(&state.streams, reaction, &mut roots);
    state.temporary_roots.extend(roots);
    let result = match reaction {
        StreamReaction::Pump { readable } => {
            readable::finish_pipe_write(ctx, state, readable, value, rejected)
        }
        StreamReaction::FinishClose { stream, promise } => {
            writable::finish_close(ctx, state, stream, promise, value, rejected)
        }
    };
    state.temporary_roots.truncate(initial_temp_roots);
    result
}

pub(super) fn create_body_stream(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    bytes: &[u8],
) -> Option<(i64, u32)> {
    readable::from_bytes(ctx, state, bytes)
}

fn queuing_strategy(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    byte_length: bool,
) -> i64 {
    let options = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let high_water_mark = if value::is_js_object(options) {
        let raw = read_named(ctx, state, options, "highWaterMark");
        super::runtime::to_number(state, raw).unwrap_or(f64::NAN)
    } else {
        f64::NAN
    };
    if !high_water_mark.is_finite() || high_water_mark < 0.0 {
        return type_error(
            ctx,
            state,
            "highWaterMark must be a non-negative finite number",
        );
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return super::fail_dispatch(ctx);
    };
    let Some(size) = state.native_callable(NativeCallableKind::Stream(
        StreamCallable::QueuingSize(byte_length),
    )) else {
        return super::fail_dispatch(ctx);
    };
    if super::modules::set_named_property(
        state,
        object,
        "highWaterMark",
        value::encode_f64(high_water_mark),
    )
    .is_err()
        || super::modules::set_named_property(state, object, "size", size).is_err()
    {
        return super::fail_dispatch(ctx);
    }
    object
}

fn queuing_size(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    byte_length: bool,
) -> i64 {
    if !byte_length {
        return value::encode_f64(1.0);
    }
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if let Some(length) = super::typedarray::byte_length(state, chunk) {
        return value::encode_f64(length as f64);
    }
    if value::is_js_object(chunk) {
        let length = read_named(ctx, state, chunk, "byteLength");
        if let Some(length) = super::runtime::to_number(state, length) {
            return value::encode_f64(length);
        }
    }
    value::encode_f64(1.0)
}
