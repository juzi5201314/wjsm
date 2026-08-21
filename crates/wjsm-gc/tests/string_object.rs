//! 堆内字符串对象（阶段 2.1）：分配、增长、搬迁、Cons/Slice 子引用与惰性哈希。
//!
//! 验收点：字符串入 ManagedHeap 后，GC 的 `object_size` / `scan_references` /
//! 搬迁路径都能正确处理——payload 内容在增长与搬迁后保持不变，Cons/Slice 的
//! 子引用句柄在搬迁后仍可解析。

use std::sync::Arc;

use wjsm_gc::{
    BarrierEpoch, GrowableHeapMemory, HandleTableV2, HeapAccessV2, HeapBarrier, ManagedHeapLayout,
    NativeHeapMemory, Nlab, PROTO_NULL_SENTINEL, TestHeapMemory, ZgcBarrierSet,
};
use wjsm_ir::constants;

const HEAP_BYTES: u64 = 4 * 1024 * 1024;

fn heap_disabled() -> HeapAccessV2<TestHeapMemory> {
    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    HeapAccessV2::with_handles(memory, layout, handles, HeapBarrier::Disabled).unwrap()
}

fn allocate<M: GrowableHeapMemory>(heap: &HeapAccessV2<M>, bytes: u64) -> u64 {
    heap.allocate(&mut Nlab::new(), bytes)
        .unwrap()
        .object()
        .offset()
}

fn publish_utf16<M: GrowableHeapMemory>(heap: &HeapAccessV2<M>, units: &[u16]) -> u32 {
    let handle = heap.allocate_handle().unwrap();
    let capacity = units.len().next_multiple_of(4) as u32 * 2;
    let object = allocate(
        heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + capacity as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_UTF16_FLAT,
        0,
        units.len() as u32,
        capacity,
    )
    .unwrap();
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    heap.write_string_payload(handle, 0, &bytes).unwrap();
    handle
}

fn publish_latin1<M: GrowableHeapMemory>(heap: &HeapAccessV2<M>, bytes: &[u8]) -> u32 {
    let handle = heap.allocate_handle().unwrap();
    let capacity = bytes.len().next_multiple_of(8) as u32;
    let object = allocate(
        heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + capacity as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_LATIN1_FLAT,
        0,
        bytes.len() as u32,
        capacity,
    )
    .unwrap();
    heap.write_string_payload(handle, 0, bytes).unwrap();
    handle
}

/// 发布一个 Builder 字符串（Utf16 载荷），初始容量给定。
fn publish_builder(heap: &HeapAccessV2<TestHeapMemory>, capacity: u32) -> u32 {
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(
        heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + capacity as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_BUILDER,
        0,
        0,
        capacity,
    )
    .unwrap();
    handle
}

#[test]
fn publish_utf16_flat_roundtrip() {
    let heap = heap_disabled();
    let units: Vec<u16> = "hello wjsm 世界".encode_utf16().collect();
    let handle = publish_utf16(&heap, &units);

    assert_eq!(
        heap.string_repr(handle).unwrap(),
        constants::STRING_REPR_UTF16_FLAT
    );
    assert_eq!(heap.string_length(handle).unwrap(), units.len() as u32);
    assert_eq!(
        heap.string_capacity(handle).unwrap(),
        units.len().next_multiple_of(4) as u32 * 2
    );
    // payload 逐码元读回与写入一致。
    let payload = heap.read_string_payload(handle).unwrap();
    let decoded: Vec<u16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(decoded[..units.len()], units[..]);
    // 对象尺寸公式：header + capacity。
    assert_eq!(
        heap.object_size(handle).unwrap(),
        constants::HEAP_STRING_HEADER_SIZE as u64 + heap.string_capacity(handle).unwrap() as u64
    );
}

#[test]
fn publish_latin1_flat_roundtrip() {
    let heap = heap_disabled();
    let content = b"latin1 payload: \x00\x7f\x80\xff";
    let handle = publish_latin1(&heap, content);

    assert_eq!(
        heap.string_repr(handle).unwrap(),
        constants::STRING_REPR_LATIN1_FLAT
    );
    assert_eq!(heap.string_length(handle).unwrap(), content.len() as u32);
    let payload = heap.read_string_payload(handle).unwrap();
    assert_eq!(&payload[..content.len()], content);
}

#[test]
fn grow_builder_preserves_payload_and_relocates() {
    let heap = heap_disabled();
    let handle = publish_builder(&heap, 16);
    let first_chunk = "abc".encode_utf16().collect::<Vec<u16>>();
    let mut bytes = Vec::with_capacity(6);
    for unit in &first_chunk {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    heap.write_string_payload(handle, 0, &bytes).unwrap();
    heap.set_string_length(handle, 3).unwrap();

    let old_object = heap.resolve_handle(handle).unwrap();
    assert_eq!(heap.string_capacity(handle).unwrap(), 16);

    // 扩到 64 字节：触发整块搬迁，handle 必须解析到新地址。
    heap.grow_string_capacity(handle, 64).unwrap();
    let new_object = heap.resolve_handle(handle).unwrap();
    assert_ne!(old_object, new_object);
    assert!(heap.string_capacity(handle).unwrap() >= 64);
    assert_eq!(heap.string_length(handle).unwrap(), 3);

    // 内容不变（首 3 个码元仍为 "abc"）。
    let payload = heap.read_string_payload(handle).unwrap();
    let decoded: Vec<u16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(decoded[..3], first_chunk[..]);
    // 对象尺寸与新的 capacity 一致（GC 遍历依赖）。
    assert_eq!(
        heap.object_size(handle).unwrap(),
        constants::HEAP_STRING_HEADER_SIZE as u64 + heap.string_capacity(handle).unwrap() as u64
    );
}

#[test]
fn relocate_string_preserves_payload() {
    let heap = heap_disabled();
    let units: Vec<u16> = "relocate me 搬迁".encode_utf16().collect();
    let handle = publish_utf16(&heap, &units);
    let old_object = heap.resolve_handle(handle).unwrap();

    // 经 collector capability 的整块搬迁路径（mark-sweep 同款）；返回搬迁字节数，
    // 新地址经 handle 解析。
    let moved_bytes = heap
        .collector_capability()
        .relocate(&mut Nlab::new(), handle)
        .unwrap();
    assert_eq!(moved_bytes, heap.object_size(handle).unwrap());
    let moved = heap.resolve_handle(handle).unwrap();
    assert_ne!(old_object, moved);

    assert_eq!(heap.string_length(handle).unwrap(), units.len() as u32);
    let payload = heap.read_string_payload(handle).unwrap();
    let decoded: Vec<u16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(decoded[..units.len()], units[..]);
    // 搬迁后 gc_word 的 handle 仍指向正确句柄。
    assert_eq!(heap.object_handle_at(moved).unwrap(), handle);
}

#[test]
fn cons_children_survive_relocation_and_scan() {
    let heap = heap_disabled();
    let left = publish_utf16(&heap, &"left".encode_utf16().collect::<Vec<_>>());
    let right = publish_utf16(&heap, &"right".encode_utf16().collect::<Vec<_>>());

    let handle = heap.allocate_handle().unwrap();
    let object = allocate(
        &heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + constants::HEAP_STRING_CONS_PAYLOAD_SIZE as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_CONS,
        0,
        8,
        constants::HEAP_STRING_CONS_PAYLOAD_SIZE,
    )
    .unwrap();
    heap.set_cons_children(handle, left, right).unwrap();
    assert_eq!(heap.cons_children(handle).unwrap(), Some((left, right)));

    // scan_references 产出两个子句柄（以及 prototype；本测试 proto 为哨兵，不产出）。
    let mut references = Vec::new();
    heap.scan_references(handle, |encoded| references.push(encoded))
        .unwrap();
    let mut children: Vec<u32> = references
        .iter()
        .map(|value| {
            assert!(wjsm_ir::value::is_handle_backed_reference(*value));
            wjsm_ir::value::decode_handle(*value)
        })
        .collect();
    children.sort_unstable();
    let mut expected = vec![left, right];
    expected.sort_unstable();
    assert_eq!(children, expected);

    // 搬迁 Cons 节点：子句柄不变，仍可解析。
    let old_object = heap.resolve_handle(handle).unwrap();
    let _moved_bytes = heap
        .collector_capability()
        .relocate(&mut Nlab::new(), handle)
        .unwrap();
    let moved = heap.resolve_handle(handle).unwrap();
    assert_ne!(old_object, moved);
    assert_eq!(heap.cons_children(handle).unwrap(), Some((left, right)));
    assert_eq!(
        heap.object_size(handle).unwrap(),
        constants::HEAP_STRING_HEADER_SIZE as u64 + constants::HEAP_STRING_CONS_PAYLOAD_SIZE as u64
    );
}

#[test]
fn slice_parts_survive_relocation() {
    let heap = heap_disabled();
    let base = publish_utf16(&heap, &"slice base 切片".encode_utf16().collect::<Vec<_>>());

    let handle = heap.allocate_handle().unwrap();
    let object = allocate(
        &heap,
        constants::HEAP_STRING_HEADER_SIZE as u64
            + constants::HEAP_STRING_SLICE_PAYLOAD_SIZE as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_SLICE,
        0,
        5,
        constants::HEAP_STRING_SLICE_PAYLOAD_SIZE,
    )
    .unwrap();
    heap.set_slice_parts(handle, base, 1, 6).unwrap();
    assert_eq!(heap.slice_parts(handle).unwrap(), Some((base, 1, 6)));

    // scan_references 产出 base 句柄。
    let mut references = Vec::new();
    heap.scan_references(handle, |encoded| references.push(encoded))
        .unwrap();
    let decoded: Vec<u32> = references
        .iter()
        .map(|value| {
            assert!(wjsm_ir::value::is_handle_backed_reference(*value));
            wjsm_ir::value::decode_handle(*value)
        })
        .collect();
    assert_eq!(decoded, vec![base]);

    // 搬迁后 slice 三元组不变；新地址经 handle 解析。
    let _moved_bytes = heap
        .collector_capability()
        .relocate(&mut Nlab::new(), handle)
        .unwrap();
    assert_eq!(heap.slice_parts(handle).unwrap(), Some((base, 1, 6)));
    let moved = heap.resolve_handle(handle).unwrap();
    assert_eq!(heap.object_handle_at(moved).unwrap(), handle);
    assert_eq!(
        heap.object_size(handle).unwrap(),
        constants::HEAP_STRING_HEADER_SIZE as u64
            + constants::HEAP_STRING_SLICE_PAYLOAD_SIZE as u64
    );
}

#[test]
fn string_content_hash_is_lazy_and_stable() {
    let heap = heap_disabled();
    let first = publish_utf16(&heap, &"hash me 哈希".encode_utf16().collect::<Vec<_>>());
    let second = publish_utf16(&heap, &"hash me 哈希".encode_utf16().collect::<Vec<_>>());

    // 未计算：hash 字段为 0。
    assert_eq!(heap.string_hash(first).unwrap(), 0);
    let hash = heap.string_content_hash(first).unwrap();
    assert_ne!(hash, 0);
    // 已缓存：重复调用返回同一值，字段也写入非 0。
    assert_eq!(heap.string_content_hash(first).unwrap(), hash);
    assert_eq!(heap.string_hash(first).unwrap(), hash);
    // 相同内容（进程内同一种子）得到相同哈希。
    assert_eq!(heap.string_content_hash(second).unwrap(), hash);

    // 不同内容哈希不同（非随机断言：内容明确不同，仅断言不等）。
    let other = publish_utf16(&heap, &"other".encode_utf16().collect::<Vec<_>>());
    assert_ne!(heap.string_content_hash(other).unwrap(), hash);
}

#[test]
fn latin1_and_utf16_same_content_same_hash() {
    let heap = heap_disabled();
    // "abc" 既能以 Latin1 单字节存，也能以 UTF-16 双字节存；哈希按码元序列计算，
    // 两种表示必须得到同一值（ECMAScript 语义是 UTF-16 码元序列）。
    let latin1 = publish_latin1(&heap, b"abc");
    let utf16 = publish_utf16(&heap, &['a' as u16, 'b' as u16, 'c' as u16]);
    assert_eq!(
        heap.string_content_hash(latin1).unwrap(),
        heap.string_content_hash(utf16).unwrap()
    );
}

#[test]
fn cons_hash_requires_flatten() {
    let heap = heap_disabled();
    let left = publish_utf16(&heap, &"left".encode_utf16().collect::<Vec<_>>());
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(
        &heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + constants::HEAP_STRING_CONS_PAYLOAD_SIZE as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_CONS,
        0,
        8,
        constants::HEAP_STRING_CONS_PAYLOAD_SIZE,
    )
    .unwrap();
    heap.set_cons_children(handle, left, left).unwrap();
    assert!(matches!(
        heap.string_content_hash(handle),
        Err(wjsm_gc::HeapAccessV2Error::StringHashRequiresFlatten { .. })
    ));
}

#[test]
fn update_string_flags_and_length_survive_zgc_epoch() {
    // ZGC 下 header 字段写入必须同步到搬迁目的地；验证 flags/length 更新链路。
    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = TestHeapMemory::for_layout(&layout);
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let barriers = Arc::new(ZgcBarrierSet::new(Arc::clone(&handles), memory.clone(), 8));
    barriers.set_epoch(BarrierEpoch {
        young_marking: true,
        ..BarrierEpoch::IDLE
    });
    let heap = HeapAccessV2::with_handles(
        memory,
        layout,
        handles,
        HeapBarrier::Zgc(Arc::clone(&barriers)),
    )
    .unwrap();

    let handle = publish_builder(&heap, 16);
    heap.update_string_flags(handle, constants::STRING_FLAG_INTERNED, 0)
        .unwrap();
    heap.set_string_length(handle, 7).unwrap();
    heap.grow_string_capacity(handle, 64).unwrap();

    assert_eq!(
        heap.string_flags(handle).unwrap() & constants::STRING_FLAG_INTERNED,
        constants::STRING_FLAG_INTERNED
    );
    assert_eq!(heap.string_length(handle).unwrap(), 7);
    assert_eq!(heap.string_capacity(handle).unwrap(), 64);
}

#[test]
fn with_string_units_reads_utf16_and_latin1() {
    let heap = heap_disabled();
    let utf16_units = "读取 UTF16".encode_utf16().collect::<Vec<_>>();
    let utf16 = publish_utf16(&heap, &utf16_units);
    let latin1 = publish_latin1(&heap, b"latin1\xff");

    assert_eq!(
        heap.with_string_units(utf16, |units| units.to_vec())
            .unwrap(),
        utf16_units
    );
    assert_eq!(
        heap.with_string_units(latin1, |units| units.to_vec())
            .unwrap(),
        b"latin1\xff"
            .iter()
            .map(|&byte| u16::from(byte))
            .collect::<Vec<_>>()
    );
}

#[test]
fn with_string_bytes_reads_latin1_on_copy_and_native_views() {
    let test_heap = heap_disabled();
    let test_handle = publish_latin1(&test_heap, b"copy path");
    assert_eq!(
        test_heap
            .with_string_bytes(test_handle, |view| view.as_latin1().unwrap().to_vec())
            .unwrap(),
        b"copy path"
    );

    let layout = Arc::new(ManagedHeapLayout::new(HEAP_BYTES, 64 * 1024).unwrap());
    let memory = NativeHeapMemory::for_layout(&layout).unwrap();
    let handles = Arc::new(HandleTableV2::new(layout.as_ref().clone()).unwrap());
    let native_heap =
        HeapAccessV2::with_handles(memory, layout, handles, HeapBarrier::Disabled).unwrap();
    let native_handle = publish_latin1(&native_heap, b"native path");
    assert_eq!(
        native_heap
            .with_string_bytes(native_handle, |view| view.as_latin1().unwrap().len())
            .unwrap(),
        b"native path".len()
    );
}

#[test]
fn with_string_flatten_required() {
    let heap = heap_disabled();
    let left = publish_utf16(&heap, &"left".encode_utf16().collect::<Vec<_>>());
    let handle = heap.allocate_handle().unwrap();
    let object = allocate(
        &heap,
        constants::HEAP_STRING_HEADER_SIZE as u64 + constants::HEAP_STRING_CONS_PAYLOAD_SIZE as u64,
    );
    heap.publish_string(
        handle,
        object,
        PROTO_NULL_SENTINEL,
        constants::STRING_REPR_CONS,
        0,
        8,
        constants::HEAP_STRING_CONS_PAYLOAD_SIZE,
    )
    .unwrap();
    heap.set_cons_children(handle, left, left).unwrap();

    assert!(matches!(
        heap.with_string_units(handle, |_| ()),
        Err(wjsm_gc::HeapAccessV2Error::StringFlattenRequired { .. })
    ));
}

#[cfg(debug_assertions)]
#[test]
fn with_string_closure_no_alloc_guard() {
    let heap = heap_disabled();
    let handle = publish_utf16(&heap, &[u16::from(b'x')]);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = heap.with_string_units(handle, |_| heap.allocate_handle());
    }));
    assert!(panic.is_err());
    assert!(heap.allocate_handle().is_ok());
}
