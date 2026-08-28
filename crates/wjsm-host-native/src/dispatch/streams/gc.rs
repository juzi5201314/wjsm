//! streams 侧表的 GC 根与宿主边。
//!
//! 包装对象不做永久根：生命周期由「owner 存活 ⇒ 内部引用存活」的宿主边图
//! 维系，死 owner 交给 `sweep_retired` 清扫。仍在飞的操作（active pipe、
//! 队列中的 stream 任务/反应）由宿主自身持有，须按根钉扎其涉及的包装
//! 对象与 promise，避免操作完成前被误回收。

use std::collections::VecDeque;

use wjsm_ir::value;

use super::{
    AsyncIteratorState, ByobState, ControllerState, NativeStreamsState, ReadableState,
    StreamReaction, StreamTask, TransformState, WritableControllerState, WritableState,
};

/// 把 streams 侧仍在飞的宿主 JS 值并入 GC 根队列。
///
/// active pipe 是宿主驱动的异步操作：pump 经微任务/promise 反应推进，
/// 期间源流与 pipe promise 可能不再被用户引用，必须钉扎到 pipe 完成。
pub(crate) fn extend_gc_roots(streams: &NativeStreamsState, roots: &mut VecDeque<i64>) {
    for (_, readable) in streams.readables.iter() {
        if let Some(pipe) = &readable.pipe {
            roots.push_back(readable.object);
            roots.push_back(value::encode_object_handle(pipe.promise));
        }
    }
    // node:stream/web 桥对象由宿主缓存句柄，必须钉扎以防缓存悬空。
    roots.extend(streams.web_bridge);
}

/// 把队列中的 stream 任务涉及的包装对象、chunk 与 promise 并入根队列：
/// 任务在微任务队列里只持槽位下标，不钉扎则任务执行前 owner 可能被清扫。
pub(crate) fn extend_task_roots(
    streams: &NativeStreamsState,
    task: &StreamTask,
    roots: &mut VecDeque<i64>,
) {
    match task {
        StreamTask::CloseWritable { stream, promise } => {
            if let Some(writable) = streams.writables.get(*stream) {
                roots.push_back(writable.object);
            }
            roots.push_back(value::encode_object_handle(*promise));
        }
        StreamTask::Pull { controller } => {
            if let Some(controller) = streams.controllers.get(*controller) {
                roots.push_back(controller.object);
            }
        }
        StreamTask::Pump { readable } => {
            if let Some(readable) = streams.readables.get(*readable) {
                roots.push_back(readable.object);
            }
        }
        StreamTask::Write {
            stream,
            chunk,
            promise,
        } => {
            if let Some(writable) = streams.writables.get(*stream) {
                roots.push_back(writable.object);
            }
            roots.push_back(*chunk);
            roots.push_back(value::encode_object_handle(*promise));
        }
    }
}

/// 把挂在 promise 上的 stream 反应涉及的包装对象与 promise 并入根队列。
pub(crate) fn extend_reaction_roots(
    streams: &NativeStreamsState,
    reaction: StreamReaction,
    roots: &mut VecDeque<i64>,
) {
    match reaction {
        StreamReaction::FinishClose { stream, promise } => {
            if let Some(writable) = streams.writables.get(stream) {
                roots.push_back(writable.object);
            }
            roots.push_back(value::encode_object_handle(promise));
        }
        StreamReaction::Pump { readable } => {
            if let Some(readable) = streams.readables.get(readable) {
                roots.push_back(readable.object);
            }
        }
    }
}

/// 把 stream 侧表的内部引用并入 GC 宿主边图。
///
/// 边图必须闭合：任何存活槽位经宿主操作可达的槽位，其包装对象都要有一条
/// 来自存活 owner 的边，`sweep_retired` 才能只按包装对象死活释放槽位。
pub(crate) fn extend_gc_edges(streams: &NativeStreamsState, mut add: impl FnMut(i64, i64)) {
    for (_, readable) in streams.readables.iter() {
        extend_readable_edges(streams, readable, &mut add);
    }
    for (_, controller) in streams.controllers.iter() {
        extend_controller_edges(streams, controller, &mut add);
    }
    for (_, reader) in streams.readers.iter() {
        extend_reader_edges(streams, reader, &mut add);
    }
    for (_, byob) in streams.byob_requests.iter() {
        extend_byob_edges(streams, byob, &mut add);
    }
    for (_, writable) in streams.writables.iter() {
        extend_writable_edges(streams, writable, &mut add);
    }
    for (_, controller) in streams.writable_controllers.iter() {
        extend_writable_controller_edges(streams, controller, &mut add);
    }
    for (_, writer) in streams.writers.iter() {
        extend_writer_edges(streams, writer, &mut add);
    }
    for (_, transform) in streams.transforms.iter() {
        extend_transform_edges(streams, transform, &mut add);
    }
    for (_, iterator) in streams.async_iterators.iter() {
        extend_async_iterator_edges(streams, iterator, &mut add);
    }
}

fn add_value(add: &mut impl FnMut(i64, i64), owner: i64, target: i64) {
    if !value::is_undefined(target) {
        add(owner, target);
    }
}

fn add_optional(add: &mut impl FnMut(i64, i64), owner: i64, target: Option<i64>) {
    if let Some(target) = target.filter(|stored| !value::is_undefined(*stored)) {
        add(owner, target);
    }
}

fn add_promise(add: &mut impl FnMut(i64, i64), owner: i64, promise: u32) {
    add(owner, value::encode_object_handle(promise));
}

fn extend_readable_edges(
    streams: &NativeStreamsState,
    readable: &ReadableState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = readable.object;
    if let Some(controller) = streams.controllers.get(readable.controller) {
        add_value(add, owner, controller.object);
    }
    add_optional(add, owner, readable.error);
    if let Some(pipe) = &readable.pipe {
        if let Some(destination) = streams.writables.get(pipe.destination) {
            add_value(add, owner, destination.object);
        }
        add_promise(add, owner, pipe.promise);
    }
}

fn extend_controller_edges(
    streams: &NativeStreamsState,
    controller: &ControllerState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = controller.object;
    // controller 反向钉住所属流（规范内部槽 [[stream]]）：仅持 controller
    // 时 enqueue/close 仍须触达 readable。
    if let Some(readable) = streams.readables.get(controller.readable) {
        add_value(add, owner, readable.object);
    }
    if let Some(byob) = controller
        .active_byob
        .and_then(|request| streams.byob_requests.get(request))
    {
        add_value(add, owner, byob.object);
    }
    add_value(add, owner, controller.source);
    add_optional(add, owner, controller.pull);
    add_optional(add, owner, controller.cancel);
    for chunk in &controller.queue {
        add_value(add, owner, *chunk);
    }
}

fn extend_reader_edges(
    streams: &NativeStreamsState,
    reader: &super::ReaderState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = reader.object;
    if let Some(readable) = streams.readables.get(reader.stream) {
        add_value(add, owner, readable.object);
        // 锁定流反向钉住 reader（规范内部槽 [[reader]]）：closed/pending
        // promise 须在流可达期间保持可 settle。
        add_value(add, readable.object, owner);
    }
    add_promise(add, owner, reader.closed_promise);
    for pending in &reader.pending {
        add_promise(add, owner, pending.promise);
        add_optional(add, owner, pending.view);
    }
}

fn extend_byob_edges(
    streams: &NativeStreamsState,
    byob: &ByobState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = byob.object;
    add_value(add, owner, byob.view);
    add_promise(add, owner, byob.promise);
    if let Some(controller) = streams.controllers.get(byob.controller) {
        add_value(add, owner, controller.object);
    }
}

fn extend_writable_edges(
    streams: &NativeStreamsState,
    writable: &WritableState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = writable.object;
    if let Some(controller) = streams.writable_controllers.get(writable.controller) {
        add_value(add, owner, controller.object);
    }
    // 写路径按槽位引用 transform，须钉住 transform 包装对象。
    if let Some(transform) = writable
        .transform
        .and_then(|transform| streams.transforms.get(transform))
    {
        add_value(add, owner, transform.object);
    }
}

fn extend_writable_controller_edges(
    streams: &NativeStreamsState,
    controller: &WritableControllerState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = controller.object;
    if let Some(writable) = streams.writables.get(controller.stream) {
        add_value(add, owner, writable.object);
    }
    add_value(add, owner, controller.sink);
    add_optional(add, owner, controller.write);
    add_optional(add, owner, controller.close);
    add_optional(add, owner, controller.abort);
    add_value(add, owner, controller.signal);
}

fn extend_writer_edges(
    streams: &NativeStreamsState,
    writer: &super::WriterState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = writer.object;
    if let Some(writable) = streams.writables.get(writer.stream) {
        add_value(add, owner, writable.object);
        // 锁定流反向钉住 writer：closed promise 须在流可达期间可 settle。
        add_value(add, writable.object, owner);
    }
    add_promise(add, owner, writer.closed_promise);
    add_promise(add, owner, writer.ready_promise);
}

fn extend_transform_edges(
    streams: &NativeStreamsState,
    transform: &TransformState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = transform.object;
    if let Some(readable) = streams.readables.get(transform.readable) {
        add_value(add, owner, readable.object);
    }
    if let Some(writable) = streams.writables.get(transform.writable) {
        add_value(add, owner, writable.object);
    }
    if let Some(controller) = streams.controllers.get(transform.controller) {
        add_value(add, owner, controller.object);
    }
    add_value(add, owner, transform.transformer);
    add_optional(add, owner, transform.transform);
    add_optional(add, owner, transform.flush);
}

fn extend_async_iterator_edges(
    streams: &NativeStreamsState,
    iterator: &AsyncIteratorState,
    add: &mut impl FnMut(i64, i64),
) {
    if let Some(reader) = streams.readers.get(iterator.reader) {
        add_value(add, iterator.object, reader.object);
    }
}
