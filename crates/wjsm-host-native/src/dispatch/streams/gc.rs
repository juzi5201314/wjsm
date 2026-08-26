use std::collections::VecDeque;

use wjsm_ir::value;

use super::{
    AsyncIteratorState, ByobState, ControllerState, NativeStreamsState, ObjectKind, ReadableState,
    TransformState, WritableControllerState, WritableState, WriterState,
};

/// 把 stream 侧表注册的包装对象并入 GC 根队列。
///
/// readable/writable 等通过虚拟属性暴露，堆对象图上没有对应 slot；若不钉扎，
/// 用户仅持有 writer/reader 时内部 controller 对象会在 GC 后被回收。
pub(crate) fn extend_gc_roots(streams: &NativeStreamsState, roots: &mut VecDeque<i64>) {
    for handle in streams.objects.keys() {
        roots.push_back(value::encode_object_handle(*handle));
    }
}

/// 把 stream 侧表中的回调、队列 chunk 等 JS 值引用并入 GC 宿主边图。
pub(crate) fn extend_gc_edges(streams: &NativeStreamsState, mut add: impl FnMut(i64, i64)) {
    for readable in &streams.readables {
        extend_readable_edges(streams, readable, &mut add);
    }
    for controller in &streams.controllers {
        extend_controller_edges(controller, &mut add);
    }
    for writable in &streams.writables {
        extend_writable_edges(streams, writable, &mut add);
    }
    for controller in &streams.writable_controllers {
        extend_writable_controller_edges(controller, &mut add);
    }
    for byob in &streams.byob_requests {
        extend_byob_edges(byob, &mut add);
    }
    for iterator in &streams.async_iterators {
        extend_async_iterator_edges(streams, iterator, &mut add);
    }
    for (handle, kind) in &streams.objects {
        let owner = value::encode_object_handle(*handle);
        match kind {
            ObjectKind::Transform(index) => {
                if let Some(transform) = streams.transforms.get(*index as usize) {
                    extend_transform_edges(streams, owner, transform, &mut add);
                }
            }
            ObjectKind::Reader(index) => {
                if let Some(reader) = streams.readers.get(*index as usize) {
                    extend_reader_edges(streams, owner, reader, &mut add);
                }
            }
            ObjectKind::Writer(index) => {
                if let Some(writer) = streams.writers.get(*index as usize) {
                    extend_writer_edges(streams, owner, writer, &mut add);
                }
            }
            _ => {}
        }
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

fn extend_transform_edges(
    streams: &NativeStreamsState,
    owner: i64,
    transform: &TransformState,
    add: &mut impl FnMut(i64, i64),
) {
    if let Some(readable) = streams.readables.get(transform.readable as usize) {
        add_value(add, owner, readable.object);
    }
    if let Some(writable) = streams.writables.get(transform.writable as usize) {
        add_value(add, owner, writable.object);
    }
    add_value(add, owner, transform.transformer);
    add_optional(add, owner, transform.transform);
    add_optional(add, owner, transform.flush);
    if let Some(controller) = streams.controllers.get(transform.controller as usize) {
        add_value(add, owner, controller.object);
    }
}

fn extend_readable_edges(
    streams: &NativeStreamsState,
    readable: &ReadableState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = readable.object;
    if let Some(controller) = streams.controllers.get(readable.controller as usize) {
        add_value(add, owner, controller.object);
    }
    add_optional(add, owner, readable.error);
    if let Some(pipe) = &readable.pipe {
        if let Some(destination) = streams.writables.get(pipe.destination as usize) {
            add_value(add, owner, destination.object);
        }
    }
}

fn extend_controller_edges(controller: &ControllerState, add: &mut impl FnMut(i64, i64)) {
    let owner = controller.object;
    add_value(add, owner, controller.source);
    add_optional(add, owner, controller.pull);
    add_optional(add, owner, controller.cancel);
    for chunk in &controller.queue {
        add_value(add, owner, *chunk);
    }
}

fn extend_writable_edges(
    streams: &NativeStreamsState,
    writable: &WritableState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = writable.object;
    if let Some(controller) = streams.writable_controllers.get(writable.controller as usize) {
        add_value(add, owner, controller.object);
    }
    if let Some(transform) = writable.transform {
        if let Some(transform_state) = streams.transforms.get(transform as usize) {
            extend_transform_edges(streams, owner, transform_state, add);
        }
    }
}

fn extend_writable_controller_edges(
    controller: &WritableControllerState,
    add: &mut impl FnMut(i64, i64),
) {
    let owner = controller.object;
    add_value(add, owner, controller.sink);
    add_optional(add, owner, controller.write);
    add_optional(add, owner, controller.close);
    add_optional(add, owner, controller.abort);
    add_value(add, owner, controller.signal);
}

fn extend_reader_edges(
    streams: &NativeStreamsState,
    owner: i64,
    reader: &super::ReaderState,
    add: &mut impl FnMut(i64, i64),
) {
    if let Some(readable) = streams.readables.get(reader.stream as usize) {
        add_value(add, owner, readable.object);
    }
    for pending in &reader.pending {
        add_optional(add, owner, pending.view);
    }
}

fn extend_writer_edges(
    streams: &NativeStreamsState,
    owner: i64,
    writer: &WriterState,
    add: &mut impl FnMut(i64, i64),
) {
    if let Some(writable) = streams.writables.get(writer.stream as usize) {
        add_value(add, owner, writable.object);
    }
}

fn extend_byob_edges(byob: &ByobState, add: &mut impl FnMut(i64, i64)) {
    add_value(add, byob.object, byob.view);
}

fn extend_async_iterator_edges(
    streams: &NativeStreamsState,
    iterator: &AsyncIteratorState,
    add: &mut impl FnMut(i64, i64),
) {
    if let Some(reader) = streams.readers.get(iterator.reader as usize) {
        if let Some(readable) = streams.readables.get(reader.stream as usize) {
            add_value(add, iterator.object, readable.object);
        }
    }
}
