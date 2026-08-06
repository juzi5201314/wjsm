//! JS 堆对象引用槽遍历 owner。
//!
//! 本模块只暴露 handle/value 级扫描结果，不把裸对象指针泄漏给算法层。
//! mark-sweep、G1 young/mixed 与 ZGC mark 共用这里的对象布局解析，避免
//! 每个算法复制 proto / property / element / side-table-backed 引用扫描逻辑。
//!
//! # 对象与数组是同构的
//!
//! 隐藏类重构之后堆内只有「16 字节 header + N×8 值槽」一种 payload 形态：
//! 属性名与 flags 全在宿主 `ShapeTable`，堆里每个 8 字节槽统一是一个 boxed i64。
//! 因此扫描既不需要查 shape 表，也不需要按 flags 区分数据/accessor 槽——
//! 对象与数组走同一套 `16 + i * 8` 公式，只在容量字段的取法上不同
//! （对象读 `+8` 的 value_capacity，数组读 `+8` 的 length）。
//!
//! 未使用的值槽恒为 0（即 `+0.0`，不是句柄），扫到也是惰性的。

use std::ops::Range;

use wjsm_ir::{constants, value};

use crate::runtime_gc::api::{GcContext, Handle, Value};
use crate::runtime_gc::context::GcHeapLayout;

const OBLET_SLOT_COUNT: usize = 256;
const PROTO_NULL_SENTINEL: u32 = 0xFFFF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotValue {
    pub slot_addr: usize,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanTask {
    Header {
        handle: Handle,
        ptr: usize,
    },
    /// 值槽区间 `[start, end)`；对象与数组共用，公式同为 `ptr + 16 + i * 8`。
    ValueSlots {
        handle: Handle,
        ptr: usize,
        start: usize,
        end: usize,
    },
}

#[derive(Default)]
pub(crate) struct ObjectWalker {
    raw_values: Vec<Value>,
}

impl ObjectWalker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn visit_object_children(
        &mut self,
        ctx: &mut GcContext<'_>,
        h: Handle,
        table_base: usize,
        obj_table_count: usize,
        visit: &mut dyn FnMut(Handle),
    ) {
        let tasks =
            ctx.with_memory(|data| scan_tasks_for_handle(data, h, table_base, obj_table_count));
        for task in tasks {
            ctx.with_memory(|data| {
                self.collect_task_raw_values(data, task);
            });
            for &val in &self.raw_values {
                visit_value_handles(ctx, val, obj_table_count, visit);
            }
        }
    }

    fn collect_task_raw_values(&mut self, data: &[u8], task: ScanTask) {
        self.raw_values.clear();
        collect_task_slot_values(data, task, &mut |slot| self.raw_values.push(slot.value));
    }
}

pub(crate) fn resolve_handle(
    data: &[u8],
    h: Handle,
    table_base: usize,
    obj_table_count: usize,
) -> Option<usize> {
    if (h as usize) >= obj_table_count {
        return None;
    }
    let entry_size = constants::HANDLE_TABLE_ENTRY_SIZE as usize;
    let addr = table_base.checked_add(h as usize * entry_size)?;
    let bytes: [u8; 8] = data.get(addr..addr + 8)?.try_into().ok()?;
    let entry = u64::from_le_bytes(bytes);
    // V2 entry: (addr << 16) | state；state==0 为空槽。兼容测试里直接写低 32 位 ptr 的布局。
    let ptr = if entry > u32::MAX as u64 {
        (entry >> 16) as usize
    } else {
        (entry as u32 & !0x3) as usize
    };
    (ptr != 0).then_some(ptr)
}

pub(crate) fn scan_tasks_for_handle(
    data: &[u8],
    h: Handle,
    table_base: usize,
    obj_table_count: usize,
) -> Vec<ScanTask> {
    let Some(ptr) = resolve_handle(data, h, table_base, obj_table_count) else {
        return Vec::new();
    };
    if ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize > data.len() {
        return Vec::new();
    }
    let heap_type = data[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize];
    if !crate::runtime_gc::context::is_known_gc_heap_type(heap_type) {
        return Vec::new();
    }
    scan_tasks_for_ptr(data, h, ptr)
}

pub(crate) fn scan_tasks_for_ptr(data: &[u8], handle: Handle, ptr: usize) -> Vec<ScanTask> {
    if ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize > data.len() {
        debug_assert!(
            false,
            "GC object walker: live handle points outside object header"
        );
        return Vec::new();
    }

    let mut tasks = vec![ScanTask::Header { handle, ptr }];
    let heap_type = data[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize];
    // 数组扫到 length（尾部空闲容量必为空洞/未初始化），对象扫满 value_capacity
    // （shape 之外的槽恒为 0，扫到是惰性的，因此无需查 ShapeTable 取 slot_count）。
    let slot_count = match crate::runtime_gc::context::gc_heap_layout(heap_type) {
        GcHeapLayout::Array => read_u32(data, ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize),
        GcHeapLayout::ObjectLike => {
            read_u32(data, ptr + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as usize)
        }
    }
    .unwrap_or_default() as usize;
    push_oblet_tasks(&mut tasks, handle, ptr, slot_count);
    tasks
}

#[allow(dead_code)]
pub(crate) fn collect_slots_in_range(
    data: &[u8],
    table_base: usize,
    obj_table_count: usize,
    range: Range<usize>,
    out: &mut Vec<SlotValue>,
) {
    out.clear();
    for h in 0..obj_table_count as Handle {
        let Some(ptr) = resolve_handle(data, h, table_base, obj_table_count) else {
            continue;
        };
        let tasks = scan_tasks_for_ptr(data, h, ptr);
        for task in tasks {
            collect_task_slot_values(data, task, &mut |slot| {
                if range.contains(&slot.slot_addr) {
                    out.push(slot);
                }
            });
        }
    }
}

pub(crate) fn visit_value_handles(
    ctx: &mut GcContext<'_>,
    val: Value,
    obj_table_count: usize,
    visit: &mut dyn FnMut(Handle),
) {
    visit_value_references(ctx, val, obj_table_count, visit, &mut |_| {});
}

pub(crate) fn visit_value_references(
    ctx: &mut GcContext<'_>,
    val: Value,
    obj_table_count: usize,
    visit_object: &mut dyn FnMut(Handle),
    visit_runtime_string: &mut dyn FnMut(u32),
) {
    if value::is_runtime_string_handle(val) {
        visit_runtime_string(value::decode_runtime_string_handle(val));
        return;
    }
    if !value::tag_needs_root(val) {
        return;
    }
    if value::is_object(val) || value::is_array(val) {
        let handle = value::decode_object_handle(val);
        if usize::try_from(handle).is_ok_and(|handle| handle < obj_table_count) {
            visit_object(handle);
        }
        return;
    }
    if value::is_function(val) {
        let function_idx = usize::try_from(val as u32).expect("u32 fits usize");
        if function_idx < ctx.num_ir_functions() {
            let handle = function_idx.saturating_add(ctx.function_props_base());
            if handle < obj_table_count {
                visit_object(u32::try_from(handle).expect("object handle fits u32"));
            }
        }
        return;
    }
    let references = if value::is_closure(val) {
        let index = usize::try_from(value::decode_closure_idx(val)).expect("u32 fits usize");
        ctx.with_state(|state| {
            state
                .closures
                .lock()
                .ok()
                .and_then(|entries| entries.get(index).map(|entry| vec![entry.env_obj]))
                .unwrap_or_default()
        })
    } else if value::is_native_callable(val) {
        let index =
            usize::try_from(value::decode_native_callable_idx(val)).expect("u32 fits usize");
        ctx.with_state(|state| {
            crate::runtime_gc::native_callable_refs::collect_native_callable_refs(state, index)
        })
    } else if value::is_bound(val) {
        let index = usize::try_from(value::decode_bound_idx(val)).expect("u32 fits usize");
        ctx.with_state(|state| crate::runtime_gc::side_table_refs::collect_bound_refs(state, index))
    } else if value::is_proxy(val) {
        let index = usize::try_from(value::decode_proxy_handle(val)).expect("u32 fits usize");
        ctx.with_state(|state| crate::runtime_gc::side_table_refs::collect_proxy_refs(state, index))
    } else if value::is_iterator(val) {
        let index = usize::try_from(value::decode_handle(val)).expect("u32 fits usize");
        ctx.with_state(|state| {
            crate::runtime_gc::side_table_refs::collect_iterator_refs(state, index)
        })
    } else if value::is_scope_record(val) {
        let handle = value::decode_scope_record_handle(val);
        ctx.with_state(|state| {
            crate::runtime_gc::side_table_refs::collect_scope_record_refs(state, handle)
        })
    } else if value::is_exception(val) {
        vec![ctx.with_state(|state| {
            crate::runtime_host_helpers::exception_reason_from_state(state, val)
        })]
    } else {
        Vec::new()
    };
    for reference in references {
        visit_value_references(
            ctx,
            reference,
            obj_table_count,
            visit_object,
            visit_runtime_string,
        );
    }
}

fn push_oblet_tasks(tasks: &mut Vec<ScanTask>, handle: Handle, ptr: usize, len: usize) {
    let mut start = 0;
    while start < len {
        let end = (start + OBLET_SLOT_COUNT).min(len);
        tasks.push(ScanTask::ValueSlots {
            handle,
            ptr,
            start,
            end,
        });
        start = end;
    }
}

fn collect_task_slot_values(data: &[u8], task: ScanTask, visit: &mut dyn FnMut(SlotValue)) {
    match task {
        ScanTask::Header { ptr, .. } => collect_header_value(data, ptr, visit),
        ScanTask::ValueSlots {
            ptr, start, end, ..
        } => {
            for idx in start..end {
                let slot_addr = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + idx * constants::HEAP_OBJECT_VALUE_SLOT_SIZE as usize;
                let Some(value) = read_i64(data, slot_addr) else {
                    break;
                };
                visit(SlotValue { slot_addr, value });
            }
        }
    }
}

fn collect_header_value(data: &[u8], ptr: usize, visit: &mut dyn FnMut(SlotValue)) {
    let slot_addr = ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize;
    let Some(proto_handle) = read_u32(data, slot_addr) else {
        return;
    };
    if proto_handle != PROTO_NULL_SENTINEL {
        let value = if proto_handle & 0x8000_0000 != 0 {
            value::encode_proxy_handle(proto_handle & 0x7FFF_FFFF)
        } else {
            value::encode_object_handle(proto_handle)
        };
        visit(SlotValue { slot_addr, value });
    }
}

fn read_u32(data: &[u8], addr: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(addr..addr + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_i64(data: &[u8], addr: usize) -> Option<Value> {
    let bytes: [u8; 8] = data.get(addr..addr + 8)?.try_into().ok()?;
    Some(Value::from_le_bytes(bytes))
}

#[cfg(test)]
pub(crate) fn mark_drain_on_buffer(
    mark_bits: &mut crate::runtime_gc::mark_bitmap::MarkBitmap,
    data: &[u8],
    table_base: usize,
    obj_table_count: usize,
    roots: &[Handle],
    function_props_base: usize,
    num_ir_functions: usize,
) {
    let mut worklist: Vec<Handle> = Vec::new();
    for &h in roots {
        if mark_bits.mark_if_new(h) {
            worklist.push(h);
        }
    }
    let mut raw_values = Vec::new();
    while let Some(h) = worklist.pop() {
        for task in scan_tasks_for_handle(data, h, table_base, obj_table_count) {
            raw_values.clear();
            collect_task_slot_values(data, task, &mut |slot| raw_values.push(slot.value));
            for &val in &raw_values {
                if let Some(child) = resolve_buffer_value_handle(
                    val,
                    obj_table_count,
                    function_props_base,
                    num_ir_functions,
                ) && mark_bits.mark_if_new(child)
                {
                    worklist.push(child);
                }
            }
        }
    }
}

#[cfg(test)]
fn resolve_buffer_value_handle(
    val: Value,
    obj_table_count: usize,
    function_props_base: usize,
    num_ir_functions: usize,
) -> Option<Handle> {
    if !value::tag_needs_root(val) {
        return None;
    }
    if value::is_object(val) || value::is_array(val) {
        let h = value::decode_object_handle(val);
        return ((h as usize) < obj_table_count).then_some(h);
    }
    if value::is_function(val) {
        let function_idx = val as u32 as usize;
        if function_idx < num_ir_functions {
            let h = function_idx.saturating_add(function_props_base) as Handle;
            return ((h as usize) < obj_table_count).then_some(h);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_gc::mark_bitmap::MarkBitmap;

    /// 按新布局伪造对象堆：16 字节 header + N×8 值槽，`+8` 写值槽容量、`+12` 写 shape_id。
    fn build_object_buffer(
        table_base: usize,
        objects: &[(Handle, usize, u32, Vec<Value>)],
        obj_table_count: usize,
    ) -> Vec<u8> {
        let mut size = table_base + obj_table_count * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
        for (_h, ptr, _proto, props) in objects {
            let end = *ptr
                + constants::HEAP_OBJECT_HEADER_SIZE as usize
                + props.len() * constants::HEAP_OBJECT_VALUE_SLOT_SIZE as usize;
            size = size.max(end);
        }
        let mut buf = vec![0u8; size];
        for (h, ptr, _, _) in objects {
            let addr = table_base + *h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
            buf[addr..addr + 8].copy_from_slice(&(*ptr as u64).to_le_bytes());
        }
        for (_h, ptr, proto, props) in objects {
            let ptr = *ptr;
            buf[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
            buf[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_OBJECT;
            let capacity = props.len() as u32;
            let capacity_off = ptr + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as usize;
            buf[capacity_off..capacity_off + 4].copy_from_slice(&capacity.to_le_bytes());
            let shape_off = ptr + constants::HEAP_OBJECT_SHAPE_ID_OFFSET as usize;
            buf[shape_off..shape_off + 4].copy_from_slice(&constants::SHAPE_ID_EMPTY.to_le_bytes());
            for (index, slot_value) in props.iter().enumerate() {
                let slot = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + index * constants::HEAP_OBJECT_VALUE_SLOT_SIZE as usize;
                buf[slot..slot + 8].copy_from_slice(&slot_value.to_le_bytes());
            }
        }
        buf
    }

    fn build_array_buffer(table_base: usize, handle: Handle, ptr: usize, len: usize) -> Vec<u8> {
        let mut buf = vec![
            0u8;
            (ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize
                + len * constants::HEAP_ARRAY_ELEMENT_SIZE as usize)
                .max(table_base + 8)
        ];
        buf[table_base + handle as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize
            ..table_base + handle as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize + 8]
            .copy_from_slice(&(ptr as u64).to_le_bytes());
        buf[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_ARRAY;
        let len_u32 = len as u32;
        buf[ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize
            ..ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize + 4]
            .copy_from_slice(&len_u32.to_le_bytes());
        buf[ptr + constants::HEAP_ARRAY_CAPACITY_OFFSET as usize
            ..ptr + constants::HEAP_ARRAY_CAPACITY_OFFSET as usize + 4]
            .copy_from_slice(&len_u32.to_le_bytes());
        buf
    }

    fn enc_obj(h: u32) -> Value {
        value::encode_object_handle(h)
    }

    #[test]
    fn object_walker_marks_linear_chain_without_recursion() {
        let table_base = 1000;
        let objects = vec![
            (0u32, 2000, 1, vec![enc_obj(2)]),
            (1u32, 3000, PROTO_NULL_SENTINEL, vec![]),
            (2u32, 4000, PROTO_NULL_SENTINEL, vec![]),
        ];
        let buf = build_object_buffer(table_base, &objects, 3);
        let mut bm = MarkBitmap::new();
        bm.reset(3);

        mark_drain_on_buffer(&mut bm, &buf, table_base, 3, &[0], 0, 0);

        assert!(bm.is_marked(0));
        assert!(bm.is_marked(1));
        assert!(bm.is_marked(2));
        assert_eq!(bm.popcount(), 3);
    }

    #[test]
    fn object_walker_splits_large_arrays_into_oblets() {
        let table_base = 64;
        let ptr = 512;
        let buf = build_array_buffer(table_base, 0, ptr, 600);

        let tasks = scan_tasks_for_handle(&buf, 0, table_base, 1);

        assert_eq!(tasks[0], ScanTask::Header { handle: 0, ptr });
        assert_eq!(
            tasks[1],
            ScanTask::ValueSlots {
                handle: 0,
                ptr,
                start: 0,
                end: 256
            }
        );
        assert_eq!(
            tasks[2],
            ScanTask::ValueSlots {
                handle: 0,
                ptr,
                start: 256,
                end: 512
            }
        );
        assert_eq!(
            tasks[3],
            ScanTask::ValueSlots {
                handle: 0,
                ptr,
                start: 512,
                end: 600
            }
        );
    }

    #[test]
    fn object_walker_collects_slots_in_card_range() {
        let table_base = 1000;
        let obj_ptr = 2000;
        let objects = vec![(
            0u32,
            obj_ptr,
            PROTO_NULL_SENTINEL,
            vec![enc_obj(7), enc_obj(8)],
        )];
        let buf = build_object_buffer(table_base, &objects, 1);
        // 第 0 号值槽紧随 header，其地址即 card 区间的下界。
        let first_value_addr = obj_ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize;
        let mut slots = Vec::new();

        collect_slots_in_range(
            &buf,
            table_base,
            1,
            first_value_addr..first_value_addr + 8,
            &mut slots,
        );

        assert_eq!(
            slots,
            vec![SlotValue {
                slot_addr: first_value_addr,
                value: enc_obj(7)
            }]
        );
    }

    #[test]
    fn object_walker_rejects_out_of_range_function_ids() {
        let table_base = 0;
        let root_ptr = 100;
        let function_value = value::encode_function_idx(2);
        let buf = build_object_buffer(
            table_base,
            &[
                (0u32, root_ptr, PROTO_NULL_SENTINEL, vec![function_value]),
                (1u32, 200, PROTO_NULL_SENTINEL, vec![]),
                (2u32, 300, PROTO_NULL_SENTINEL, vec![]),
                (3u32, 400, PROTO_NULL_SENTINEL, vec![]),
            ],
            4,
        );
        let mut bm = MarkBitmap::new();
        bm.reset(4);

        mark_drain_on_buffer(&mut bm, &buf, table_base, 4, &[0], 1, 2);

        assert!(bm.is_marked(0));
        assert!(!bm.is_marked(3));
    }
}
