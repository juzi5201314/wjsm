//! JS 堆对象引用槽遍历 owner。
//!
//! 本模块只暴露 handle/value 级扫描结果，不把裸对象指针泄漏给算法层。
//! mark-sweep、G1 young/mixed 与未来 ZGC mark 共用这里的对象布局解析，避免
//! 每个算法复制 proto / property / element / side-table-backed 引用扫描逻辑。

use std::ops::Range;

use wjsm_ir::{constants, value};

use crate::runtime_gc::api::{GcContext, Handle, Value};
use crate::runtime_gc::context::GcHeapLayout;

const TYPEDARRAY_HANDLE_PROP: &str = "__typedarray_handle__";
const DATAVIEW_HANDLE_PROP: &str = "__dataview_handle__";
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
    ArrayElements {
        handle: Handle,
        ptr: usize,
        start: usize,
        end: usize,
    },
    PropertySlots {
        handle: Handle,
        ptr: usize,
        start: usize,
        end: usize,
    },
}

#[derive(Default)]
pub(crate) struct ObjectWalker {
    raw_values: Vec<Value>,
    side_children: SideTableChildHandles,
}

impl ObjectWalker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn visit_object_children(
        &mut self,
        ctx: &mut GcContext<'_>,
        h: Handle,
        obj_table_ptr: usize,
        obj_table_count: usize,
        visit: &mut dyn FnMut(Handle),
    ) {
        let tasks =
            ctx.with_memory(|data| scan_tasks_for_handle(data, h, obj_table_ptr, obj_table_count));
        for task in tasks {
            ctx.with_memory(|data| {
                self.collect_task_raw_values(data, task);
            });
            collect_side_table_child_raw_values(ctx, &self.side_children, &mut self.raw_values);
            for &val in &self.raw_values {
                visit_value_handles(ctx, val, obj_table_count, visit);
            }
        }
    }

    fn collect_task_raw_values(&mut self, data: &[u8], task: ScanTask) {
        self.raw_values.clear();
        self.side_children.clear();
        collect_task_slot_values(
            data,
            task,
            &mut |slot| self.raw_values.push(slot.value),
            &mut self.side_children,
        );
    }
}

#[derive(Default)]
struct SideTableChildHandles {
    typedarrays: Vec<usize>,
    dataviews: Vec<usize>,
}

impl SideTableChildHandles {
    fn clear(&mut self) {
        self.typedarrays.clear();
        self.dataviews.clear();
    }
}

pub(crate) fn resolve_handle(
    data: &[u8],
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Option<usize> {
    if (h as usize) >= obj_table_count {
        return None;
    }
    let addr =
        obj_table_ptr.checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)?;
    let bytes: [u8; 4] = data.get(addr..addr + 4)?.try_into().ok()?;
    let ptr = u32::from_le_bytes(bytes) as usize;
    (ptr != 0).then_some(ptr)
}

pub(crate) fn scan_tasks_for_handle(
    data: &[u8],
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Vec<ScanTask> {
    let Some(ptr) = resolve_handle(data, h, obj_table_ptr, obj_table_count) else {
        return Vec::new();
    };
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
    match crate::runtime_gc::context::gc_heap_layout(heap_type) {
        GcHeapLayout::Array => {
            let len = read_u32(data, ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize)
                .unwrap_or_default() as usize;
            push_oblet_tasks(&mut tasks, handle, ptr, len, true);
        }
        GcHeapLayout::ObjectLike => {
            let num_props = read_u32(
                data,
                ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize,
            )
            .unwrap_or_default() as usize;
            push_oblet_tasks(&mut tasks, handle, ptr, num_props, false);
        }
    }
    tasks
}

pub(crate) fn collect_slots_in_range(
    data: &[u8],
    obj_table_ptr: usize,
    obj_table_count: usize,
    range: Range<usize>,
    out: &mut Vec<SlotValue>,
) {
    out.clear();
    for h in 0..obj_table_count as Handle {
        let Some(ptr) = resolve_handle(data, h, obj_table_ptr, obj_table_count) else {
            continue;
        };
        let tasks = scan_tasks_for_ptr(data, h, ptr);
        for task in tasks {
            collect_task_slot_values(
                data,
                task,
                &mut |slot| {
                    if range.contains(&slot.slot_addr) {
                        out.push(slot);
                    }
                },
                &mut SideTableChildHandles::default(),
            );
        }
    }
}

pub(crate) fn visit_value_handles(
    ctx: &mut GcContext<'_>,
    val: Value,
    obj_table_count: usize,
    visit: &mut dyn FnMut(Handle),
) {
    if !value::tag_needs_root(val) {
        return;
    }
    if value::is_object(val) || value::is_array(val) {
        let h = value::decode_object_handle(val);
        if (h as usize) < obj_table_count {
            visit(h);
        }
        return;
    }
    if value::is_function(val) {
        let function_idx = val as u32 as usize;
        if function_idx < ctx.num_ir_functions() {
            let h = function_idx.saturating_add(ctx.function_props_base()) as Handle;
            if (h as usize) < obj_table_count {
                visit(h);
            }
        }
        return;
    }
    if value::is_closure(val) {
        let closure_idx = value::decode_closure_idx(val) as usize;
        let env_obj = ctx.with_state(|st| {
            st.closures
                .lock()
                .ok()
                .and_then(|g| g.get(closure_idx).map(|e| e.env_obj))
        });
        if let Some(env) = env_obj {
            visit_value_handles(ctx, env, obj_table_count, visit);
        }
        return;
    }
    if value::is_native_callable(val) {
        let idx = value::decode_native_callable_idx(val) as usize;
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::native_callable_refs::collect_native_callable_refs(st, idx)
        });
        for r in refs {
            visit_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_bound(val) {
        let idx = value::decode_bound_idx(val) as usize;
        let refs =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_bound_refs(st, idx));
        for r in refs {
            visit_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_proxy(val) {
        let idx = value::decode_proxy_handle(val) as usize;
        let refs =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_proxy_refs(st, idx));
        for r in refs {
            visit_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_iterator(val) {
        let idx = value::decode_handle(val) as usize;
        let refs =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_iterator_refs(st, idx));
        for r in refs {
            visit_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_scope_record(val) {
        let handle = value::decode_scope_record_handle(val);
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_scope_record_refs(st, handle)
        });
        for r in refs {
            visit_value_handles(ctx, r, obj_table_count, visit);
        }
    }
}

fn push_oblet_tasks(
    tasks: &mut Vec<ScanTask>,
    handle: Handle,
    ptr: usize,
    len: usize,
    array: bool,
) {
    let mut start = 0;
    while start < len {
        let end = (start + OBLET_SLOT_COUNT).min(len);
        if array {
            tasks.push(ScanTask::ArrayElements {
                handle,
                ptr,
                start,
                end,
            });
        } else {
            tasks.push(ScanTask::PropertySlots {
                handle,
                ptr,
                start,
                end,
            });
        }
        start = end;
    }
}

fn collect_task_slot_values(
    data: &[u8],
    task: ScanTask,
    visit: &mut dyn FnMut(SlotValue),
    side_children: &mut SideTableChildHandles,
) {
    match task {
        ScanTask::Header { ptr, .. } => collect_header_value(data, ptr, visit),
        ScanTask::ArrayElements {
            ptr, start, end, ..
        } => {
            for idx in start..end {
                let slot_addr = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + idx * constants::HEAP_ARRAY_ELEMENT_SIZE as usize;
                if let Some(value) = read_i64(data, slot_addr) {
                    visit(SlotValue { slot_addr, value });
                }
            }
        }
        ScanTask::PropertySlots {
            ptr, start, end, ..
        } => {
            for idx in start..end {
                let slot = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + idx * constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
                if slot + constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize > data.len() {
                    break;
                }
                let name_id = read_u32(data, slot + constants::PROP_SLOT_NAME_ID_OFFSET as usize)
                    .unwrap_or_default();
                let value_raw = read_i64(data, slot + constants::PROP_SLOT_VALUE_OFFSET as usize);
                if let Some(value) = value_raw {
                    visit(SlotValue {
                        slot_addr: slot + constants::PROP_SLOT_VALUE_OFFSET as usize,
                        value,
                    });
                }
                for val_off in [
                    constants::PROP_SLOT_GETTER_OFFSET as usize,
                    constants::PROP_SLOT_SETTER_OFFSET as usize,
                ] {
                    if let Some(value) = read_i64(data, slot + val_off) {
                        visit(SlotValue {
                            slot_addr: slot + val_off,
                            value,
                        });
                    }
                }
                if memory_c_string_eq(data, name_id, TYPEDARRAY_HANDLE_PROP) {
                    if let Some(raw) = value_raw
                        && let Some(handle) = decode_side_table_handle_value(raw)
                    {
                        side_children.typedarrays.push(handle);
                    }
                } else if memory_c_string_eq(data, name_id, DATAVIEW_HANDLE_PROP)
                    && let Some(raw) = value_raw
                    && let Some(handle) = decode_side_table_handle_value(raw)
                {
                    side_children.dataviews.push(handle);
                }
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

fn collect_side_table_child_raw_values(
    ctx: &mut GcContext<'_>,
    handles: &SideTableChildHandles,
    out: &mut Vec<Value>,
) {
    ctx.with_state(|st| {
        if let Ok(table) = st.typedarray_table.lock() {
            for &handle in &handles.typedarrays {
                if let Some(value) = table.get(handle).and_then(|entry| entry.buffer_object) {
                    out.push(value);
                }
            }
        }
        if let Ok(table) = st.dataview_table.lock() {
            for &handle in &handles.dataviews {
                if let Some(value) = table.get(handle).and_then(|entry| entry.buffer_object) {
                    out.push(value);
                }
            }
        }
    });
}

fn decode_side_table_handle_value(raw: Value) -> Option<usize> {
    if !value::is_f64(raw) {
        return None;
    }
    let n = value::decode_f64(raw);
    (n.is_finite() && n >= 0.0 && n.fract() == 0.0).then_some(n as usize)
}

fn memory_c_string_eq(data: &[u8], name_id: u32, expected: &str) -> bool {
    let start = name_id as usize;
    let end = match start.checked_add(expected.len()) {
        Some(end) => end,
        None => return false,
    };
    end < data.len() && data.get(start..end) == Some(expected.as_bytes()) && data[end] == 0
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
    obj_table_ptr: usize,
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
    let mut side_children = SideTableChildHandles::default();
    while let Some(h) = worklist.pop() {
        for task in scan_tasks_for_handle(data, h, obj_table_ptr, obj_table_count) {
            raw_values.clear();
            side_children.clear();
            collect_task_slot_values(
                data,
                task,
                &mut |slot| raw_values.push(slot.value),
                &mut side_children,
            );
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

    fn build_object_buffer(
        obj_table_ptr: usize,
        objects: &[(Handle, usize, u32, Vec<Value>)],
        obj_table_count: usize,
    ) -> Vec<u8> {
        let mut size = obj_table_ptr + obj_table_count * 4;
        for (_h, ptr, _proto, props) in objects {
            let end = *ptr
                + constants::HEAP_OBJECT_HEADER_SIZE as usize
                + props.len() * constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
            size = size.max(end);
        }
        let mut buf = vec![0u8; size];
        for (h, ptr, _, _) in objects {
            let addr = obj_table_ptr + *h as usize * 4;
            buf[addr..addr + 4].copy_from_slice(&(*ptr as u32).to_le_bytes());
        }
        for (_h, ptr, proto, props) in objects {
            let ptr = *ptr;
            buf[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
            buf[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_OBJECT;
            let cap = props.len() as u32;
            buf[ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize
                ..ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize + 4]
                .copy_from_slice(&cap.to_le_bytes());
            buf[ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize
                ..ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize + 4]
                .copy_from_slice(&cap.to_le_bytes());
            for (i, pval) in props.iter().enumerate() {
                let slot = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + i * constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
                buf[slot + constants::PROP_SLOT_VALUE_OFFSET as usize
                    ..slot + constants::PROP_SLOT_VALUE_OFFSET as usize + 8]
                    .copy_from_slice(&pval.to_le_bytes());
            }
        }
        buf
    }

    fn build_array_buffer(obj_table_ptr: usize, handle: Handle, ptr: usize, len: usize) -> Vec<u8> {
        let mut buf = vec![
            0u8;
            (ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize
                + len * constants::HEAP_ARRAY_ELEMENT_SIZE as usize)
                .max(obj_table_ptr + 4)
        ];
        buf[obj_table_ptr + handle as usize * 4..obj_table_ptr + handle as usize * 4 + 4]
            .copy_from_slice(&(ptr as u32).to_le_bytes());
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
        let obj_table_ptr = 1000;
        let objects = vec![
            (0u32, 2000, 1, vec![enc_obj(2)]),
            (1u32, 3000, PROTO_NULL_SENTINEL, vec![]),
            (2u32, 4000, PROTO_NULL_SENTINEL, vec![]),
        ];
        let buf = build_object_buffer(obj_table_ptr, &objects, 3);
        let mut bm = MarkBitmap::new();
        bm.reset(3);

        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 3, &[0], 0, 0);

        assert!(bm.is_marked(0));
        assert!(bm.is_marked(1));
        assert!(bm.is_marked(2));
        assert_eq!(bm.popcount(), 3);
    }

    #[test]
    fn object_walker_splits_large_arrays_into_oblets() {
        let obj_table_ptr = 64;
        let ptr = 512;
        let buf = build_array_buffer(obj_table_ptr, 0, ptr, 600);

        let tasks = scan_tasks_for_handle(&buf, 0, obj_table_ptr, 1);

        assert_eq!(tasks[0], ScanTask::Header { handle: 0, ptr });
        assert_eq!(
            tasks[1],
            ScanTask::ArrayElements {
                handle: 0,
                ptr,
                start: 0,
                end: 256
            }
        );
        assert_eq!(
            tasks[2],
            ScanTask::ArrayElements {
                handle: 0,
                ptr,
                start: 256,
                end: 512
            }
        );
        assert_eq!(
            tasks[3],
            ScanTask::ArrayElements {
                handle: 0,
                ptr,
                start: 512,
                end: 600
            }
        );
    }

    #[test]
    fn object_walker_collects_slots_in_card_range() {
        let obj_table_ptr = 1000;
        let obj_ptr = 2000;
        let objects = vec![(
            0u32,
            obj_ptr,
            PROTO_NULL_SENTINEL,
            vec![enc_obj(7), enc_obj(8)],
        )];
        let buf = build_object_buffer(obj_table_ptr, &objects, 1);
        let first_value_addr = obj_ptr
            + constants::HEAP_OBJECT_HEADER_SIZE as usize
            + constants::PROP_SLOT_VALUE_OFFSET as usize;
        let mut slots = Vec::new();

        collect_slots_in_range(
            &buf,
            obj_table_ptr,
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
        let obj_table_ptr = 0;
        let root_ptr = 100;
        let function_value = value::encode_function_idx(2);
        let buf = build_object_buffer(
            obj_table_ptr,
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

        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 4, &[0], 1, 2);

        assert!(bm.is_marked(0));
        assert!(!bm.is_marked(3));
    }
}
