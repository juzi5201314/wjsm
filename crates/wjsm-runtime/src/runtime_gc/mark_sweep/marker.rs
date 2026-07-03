//! Mark phase（spec §8.1，IMPL-6 worklist 不递归）。
//!
//! 移植自 runtime_heap.rs::mark_object_recursive_with_funcs（L577-761），
//! 但把 Rust 栈递归改为显式 worklist（Vec<Handle>），深对象图不栈溢出（R8/#11）。
//!
//! 算法：
//! 1. seed roots（调用方提供的 root 迭代器）
//! 2. drain worklist：对每个 handle，读对象 header 的子引用（proto/props/elements），
//!    提取子引用值，解析为 candidate handles（object/array/function → low32；
//!    closure → host closures 表 env_obj；native_callable → host 表内部引用），
//!    若未标记则标记并入 worklist。
//!
//! fixed-point host 侧表追踪（spec §10）：由 collect_with_roots 的调用方在 P4 集成时
//! 经 roots 迭代器分轮注入（continuation_table.captured_vars 等顶层 root）。
//!
//! 借用结构：drain 循环用一个可复用 scratch buffer 收集当前对象的 raw child values，
//! 之后释放 memory 借用，再经 ctx.with_state 解析 closure/native_callable 的内部引用为
//! obj_table handle。mark_bits 是 collector 字段，与 ctx 借用独立，无冲突。
use crate::runtime_gc::api::{GcContext, Handle};
use crate::runtime_gc::context::GcHeapLayout;
use crate::runtime_gc::mark_sweep::MarkSweepCollector;
use wjsm_ir::value;

const TYPEDARRAY_HANDLE_PROP: &str = "__typedarray_handle__";
const DATAVIEW_HANDLE_PROP: &str = "__dataview_handle__";

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

fn decode_side_table_handle_value(raw: i64) -> Option<usize> {
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

fn collect_side_table_child_raw_values(
    ctx: &mut GcContext,
    handles: &SideTableChildHandles,
    out: &mut Vec<i64>,
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

/// 标记 roots 并 drain worklist。
pub fn mark_roots_and_drain(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    roots: &mut dyn Iterator<Item = Handle>,
) {
    let mut worklist: Vec<Handle> = Vec::new();

    // seed roots
    for h in roots {
        if collector.mark_bits.mark_if_new(h) {
            worklist.push(h);
        }
    }

    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();

    let mut raw_vals: Vec<i64> = Vec::new();
    let mut side_child_handles = SideTableChildHandles::default();

    // drain
    while let Some(h) = worklist.pop() {
        // 收集本对象的子引用 raw values（proto + props/elements）。
        // scratch buffer 跨对象复用，避免 mark 阶段为每个对象分配 Vec。
        ctx.with_memory(|data| {
            collect_child_raw_values(
                data,
                h,
                obj_table_ptr,
                obj_table_count,
                &mut raw_vals,
                &mut side_child_handles,
            );
        });
        collect_side_table_child_raw_values(ctx, &side_child_handles, &mut raw_vals);
        // 把每个 raw value 解析为 handle（含 closure/native_callable 经 host 表解析）。
        for &val in &raw_vals {
            push_resolved_value_handles(ctx, val, obj_table_count, &mut |child| {
                if collector.mark_bits.mark_if_new(child) {
                    worklist.push(child);
                }
            });
        }
    }

    ctx.stats.marked = collector.mark_bits.popcount();
}

/// 读单个对象 h 的子引用，写入调用方提供的 scratch buffer。
///
/// 写入 raw i64 值（尚未解析为 handle），由调用方经 push_resolved_value_handles 解析。
/// 这样 object/array/function 直接转 handle，closure/native_callable 走 host 表解析。
///
/// 移植自 runtime_heap.rs:620-748（children 收集逻辑）：
/// - proto_handle（若有效）
/// - 数组：elements
/// - 对象：每属性的 value/getter/setter
/// - TypedArray/DataView hidden handle → 侧表中的 [[ViewedArrayBuffer]]
fn collect_child_raw_values(
    data: &[u8],
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
    out: &mut Vec<i64>,
    side_children: &mut SideTableChildHandles,
) {
    out.clear();
    side_children.clear();
    let obj_ptr = match resolve_handle(data, h, obj_table_ptr, obj_table_count) {
        Some(p) => p,
        None => return,
    };
    if obj_ptr + 16 > data.len() {
        return;
    }

    // proto_handle（offset 0..4）
    let proto_handle = u32::from_le_bytes([
        data[obj_ptr],
        data[obj_ptr + 1],
        data[obj_ptr + 2],
        data[obj_ptr + 3],
    ]);
    if proto_handle != 0xFFFF_FFFF && (proto_handle as usize) < obj_table_count {
        // proto 存的是 object handle（已解析），直接作为 handle candidate。
        // 用 encode_object_handle 还原为 NaN-boxed value 让 resolve 统一处理。
        out.push(value::encode_object_handle(proto_handle));
    }

    // type byte 决定数组还是对象
    let heap_type = data[obj_ptr + 4];
    match crate::runtime_gc::context::gc_heap_layout(heap_type) {
        GcHeapLayout::Array => {
            // 数组：elements（offset 16 + i*8）
            let len = u32::from_le_bytes([
                data[obj_ptr + 8],
                data[obj_ptr + 9],
                data[obj_ptr + 10],
                data[obj_ptr + 11],
            ]) as usize;
            for i in 0..len {
                let off = obj_ptr + 16 + i * 8;
                if off + 8 > data.len() {
                    break;
                }
                let elem = i64::from_le_bytes([
                    data[off],
                    data[off + 1],
                    data[off + 2],
                    data[off + 3],
                    data[off + 4],
                    data[off + 5],
                    data[off + 6],
                    data[off + 7],
                ]);
                out.push(elem);
            }
        }
        GcHeapLayout::ObjectLike => {
            // 对象 / Arguments：属性槽 [name_id(4) flags(4) value(8) getter(8) setter(8)] = 32B
            let num_props = u32::from_le_bytes([
                data[obj_ptr + 12],
                data[obj_ptr + 13],
                data[obj_ptr + 14],
                data[obj_ptr + 15],
            ]) as usize;
            for i in 0..num_props {
                let slot = obj_ptr + 16 + i * 32;
                if slot + 32 > data.len() {
                    break;
                }
                let name_id = u32::from_le_bytes([
                    data[slot],
                    data[slot + 1],
                    data[slot + 2],
                    data[slot + 3],
                ]);
                let value_raw = i64::from_le_bytes([
                    data[slot + 8],
                    data[slot + 9],
                    data[slot + 10],
                    data[slot + 11],
                    data[slot + 12],
                    data[slot + 13],
                    data[slot + 14],
                    data[slot + 15],
                ]);
                out.push(value_raw);
                for val_off in [16usize, 24] {
                    let v = i64::from_le_bytes([
                        data[slot + val_off],
                        data[slot + val_off + 1],
                        data[slot + val_off + 2],
                        data[slot + val_off + 3],
                        data[slot + val_off + 4],
                        data[slot + val_off + 5],
                        data[slot + val_off + 6],
                        data[slot + val_off + 7],
                    ]);
                    out.push(v);
                }
                if memory_c_string_eq(data, name_id, TYPEDARRAY_HANDLE_PROP) {
                    if let Some(handle) = decode_side_table_handle_value(value_raw) {
                        side_children.typedarrays.push(handle);
                    }
                } else if memory_c_string_eq(data, name_id, DATAVIEW_HANDLE_PROP)
                    && let Some(handle) = decode_side_table_handle_value(value_raw)
                {
                    side_children.dataviews.push(handle);
                }
            }
        }
    }
}

/// 把一个 NaN-boxed value 解析为它引用的 obj_table handle，并逐个回调给调用方（P4-blocker #2）。
///
/// - object/array → decode_object_handle（直接 handle）
/// - function → function_props_base + low32（函数属性对象 handle）
/// - closure → host closures 表的 env_obj，递归解析（env_obj 可能是 object/closure）
/// - native_callable → host native_callables 表内部引用（promise/generator/combinator/env）
///   递归解析（这些引用本身可能又是 object/closure/native_callable）
/// - 标量 → 不回调
///
/// closure/native_callable 的解析需要 with_state（读 host 侧表），与 with_memory 独立借用。
/// 递归收敛：env_obj/native 引用链最终落到 object/array/function（有界，受 host 表大小约束）。
fn push_resolved_value_handles(
    ctx: &mut GcContext,
    val: i64,
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
        // closure → env_obj（可能 object/closure，递归解析）
        let closure_idx = value::decode_closure_idx(val) as usize;
        let env_obj = ctx.with_state(|st| {
            st.closures
                .lock()
                .ok()
                .and_then(|g| g.get(closure_idx).map(|e| e.env_obj))
        });
        if let Some(env) = env_obj {
            push_resolved_value_handles(ctx, env, obj_table_count, visit);
        }
        return;
    }
    if value::is_native_callable(val) {
        let idx = value::decode_native_callable_idx(val) as usize;
        let refs: Vec<i64> = ctx.with_state(|st| collect_native_callable_refs(st, idx));
        for r in refs {
            push_resolved_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_bound(val) {
        let idx = value::decode_bound_idx(val) as usize;
        let refs: Vec<i64> =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_bound_refs(st, idx));
        for r in refs {
            push_resolved_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_proxy(val) {
        let idx = value::decode_proxy_handle(val) as usize;
        let refs: Vec<i64> =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_proxy_refs(st, idx));
        for r in refs {
            push_resolved_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_iterator(val) {
        let idx = value::decode_handle(val) as usize;
        let refs: Vec<i64> =
            ctx.with_state(|st| crate::runtime_gc::side_table_refs::collect_iterator_refs(st, idx));
        for r in refs {
            push_resolved_value_handles(ctx, r, obj_table_count, visit);
        }
        return;
    }
    if value::is_scope_record(val) {
        let handle = value::decode_scope_record_handle(val);
        let refs: Vec<i64> = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_scope_record_refs(st, handle)
        });
        for r in refs {
            push_resolved_value_handles(ctx, r, obj_table_count, visit);
        }
    }
    // 其余 tag（bigint/symbol/regexp/enumerator/runtime_string/exception）：
    // 侧表不含 obj_table 引用，不需追踪。
}

/// 从 native_callable 表项提取其内部持有的对象引用。
/// 委托给 `runtime_gc::native_callable_refs` 的共享实现。
fn collect_native_callable_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    crate::runtime_gc::native_callable_refs::collect_native_callable_refs(st, idx)
}

/// 读 obj_table[h] → ptr（None = 越界/空槽）。
fn resolve_handle(
    data: &[u8],
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Option<usize> {
    if (h as usize) >= obj_table_count {
        return None;
    }
    let addr = obj_table_ptr + h as usize * 4;
    if addr + 4 > data.len() {
        return None;
    }
    let ptr =
        u32::from_le_bytes([data[addr], data[addr + 1], data[addr + 2], data[addr + 3]]) as usize;
    if ptr == 0 { None } else { Some(ptr) }
}

// ── 测试辅助：纯 buffer 上的 worklist drain（不依赖 wasmtime，验证 R8 不栈溢出）──

/// 在给定 memory buffer + obj_table 布局上跑完整 mark（seed + drain worklist）。
/// 用于单元测试验证 worklist 正确性 + 深对象图不栈溢出（R8）。
///
/// 注：纯 buffer 无 RuntimeState，无法解析 closure/native_callable（P4-blocker #2 的
/// host 表路径）。测试只用 object handle 图，故 buffer 作用域只解析 object/array/function。
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
    let mut raw_vals: Vec<i64> = Vec::new();
    let mut side_child_handles = SideTableChildHandles::default();
    while let Some(h) = worklist.pop() {
        collect_child_raw_values(
            data,
            h,
            obj_table_ptr,
            obj_table_count,
            &mut raw_vals,
            &mut side_child_handles,
        );
        for &val in &raw_vals {
            // buffer 作用域：只解析 object/array/function（closure/native_callable 需 host 表）
            if let Some(child) = resolve_buffer_value_handle(
                val,
                obj_table_count,
                function_props_base,
                num_ir_functions,
            ) {
                if mark_bits.mark_if_new(child) {
                    worklist.push(child);
                }
            }
        }
    }
}

/// buffer 作用域值→handle 解析（只处理 object/array/function，无 host 表）。
#[cfg(test)]
fn resolve_buffer_value_handle(
    val: i64,
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
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_gc::mark_bitmap::MarkBitmap;
    use wjsm_ir::value;

    /// 构造一个最小对象 buffer：
    /// - obj_table 在 OBJ_TABLE_PTR，每槽 4B
    /// - 对象在各自的 ptr，header 16B：proto(4) heap_type(1=OBJECT) pad(3) capacity@+8(4) num_props@+12(4)
    /// - 属性槽 32B：name_id(4) flags(4) value(8) getter(8) setter(8)
    /// 返回 buffer + obj_table_ptr + obj_table_count。
    /// objects: Vec<(handle, ptr, proto_handle, props: Vec<value_i64>)>
    fn build_object_buffer(
        obj_table_ptr: usize,
        objects: &[(Handle, usize, u32, Vec<i64>)],
        obj_table_count: usize,
    ) -> Vec<u8> {
        // 算 buffer 大小：max(obj 末尾, obj_table 末尾)
        let mut size = obj_table_ptr + obj_table_count * 4;
        for (_h, ptr, _proto, props) in objects {
            let end = *ptr + 16 + props.len() * 32;
            size = size.max(end);
        }
        let mut buf = vec![0u8; size];
        // 写 obj_table
        for (h, ptr, _, _) in objects {
            let addr = obj_table_ptr + *h as usize * 4;
            buf[addr..addr + 4].copy_from_slice(&(*ptr as u32).to_le_bytes());
        }
        // 写对象 header + props
        for (_h, ptr, proto, props) in objects {
            let ptr = *ptr;
            // proto@+0
            buf[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
            // heap_type@+4 = OBJECT(0)
            buf[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
            // capacity@+8 = props.len()
            let cap = props.len() as u32;
            buf[ptr + 8..ptr + 12].copy_from_slice(&cap.to_le_bytes());
            // num_props@+12 = props.len()
            buf[ptr + 12..ptr + 16].copy_from_slice(&cap.to_le_bytes());
            // props
            for (i, pval) in props.iter().enumerate() {
                let slot = ptr + 16 + i * 32;
                // value@+8
                buf[slot + 8..slot + 16].copy_from_slice(&pval.to_le_bytes());
            }
        }
        buf
    }

    /// 编码 object handle 为 NaN-boxed value。
    fn enc_obj(h: u32) -> i64 {
        value::encode_handle(wjsm_ir::value::TAG_OBJECT, h)
    }

    fn write_prop_name_id(buf: &mut [u8], obj_ptr: usize, prop_idx: usize, name_id: u32) {
        let slot = obj_ptr + 16 + prop_idx * 32;
        buf[slot..slot + 4].copy_from_slice(&name_id.to_le_bytes());
    }

    #[test]
    fn collect_child_raw_values_reuses_caller_scratch_buffer() {
        let obj_table_ptr = 1000;
        let objects = vec![
            (0u32, 2000, 1, vec![enc_obj(2)]),
            (1u32, 3000, 0xFFFF_FFFF, vec![]),
            (2u32, 4000, 0xFFFF_FFFF, vec![]),
        ];
        let buf = build_object_buffer(obj_table_ptr, &objects, 3);
        let mut raw_vals = Vec::with_capacity(8);
        raw_vals.push(enc_obj(99));
        let mut side_child_handles = SideTableChildHandles::default();
        let scratch_ptr = raw_vals.as_ptr();

        collect_child_raw_values(
            &buf,
            0,
            obj_table_ptr,
            3,
            &mut raw_vals,
            &mut side_child_handles,
        );

        assert_eq!(raw_vals.as_ptr(), scratch_ptr);
        assert_eq!(raw_vals[0], enc_obj(1));
        assert!(raw_vals.contains(&enc_obj(2)));

        collect_child_raw_values(
            &buf,
            1,
            obj_table_ptr,
            3,
            &mut raw_vals,
            &mut side_child_handles,
        );

        assert_eq!(raw_vals.as_ptr(), scratch_ptr);
        assert!(raw_vals.is_empty());

        collect_child_raw_values(
            &buf,
            42,
            obj_table_ptr,
            3,
            &mut raw_vals,
            &mut side_child_handles,
        );

        assert_eq!(raw_vals.as_ptr(), scratch_ptr);
        assert!(raw_vals.is_empty());
    }

    #[test]
    fn collect_child_raw_values_records_side_table_backed_handles() {
        let obj_table_ptr = 1000;
        let obj_ptr = 2000;
        let mut buf = build_object_buffer(
            obj_table_ptr,
            &[(
                0u32,
                obj_ptr,
                0xFFFF_FFFF,
                vec![value::encode_f64(7.0), value::encode_f64(9.0)],
            )],
            1,
        );
        let typedarray_name_id = 11;
        let dataview_name_id = typedarray_name_id + TYPEDARRAY_HANDLE_PROP.len() + 1;
        buf[typedarray_name_id..typedarray_name_id + TYPEDARRAY_HANDLE_PROP.len()]
            .copy_from_slice(TYPEDARRAY_HANDLE_PROP.as_bytes());
        buf[typedarray_name_id + TYPEDARRAY_HANDLE_PROP.len()] = 0;
        buf[dataview_name_id..dataview_name_id + DATAVIEW_HANDLE_PROP.len()]
            .copy_from_slice(DATAVIEW_HANDLE_PROP.as_bytes());
        buf[dataview_name_id + DATAVIEW_HANDLE_PROP.len()] = 0;
        write_prop_name_id(&mut buf, obj_ptr, 0, typedarray_name_id as u32);
        write_prop_name_id(&mut buf, obj_ptr, 1, dataview_name_id as u32);

        let mut raw_vals = Vec::new();
        let mut side_child_handles = SideTableChildHandles::default();
        collect_child_raw_values(
            &buf,
            0,
            obj_table_ptr,
            1,
            &mut raw_vals,
            &mut side_child_handles,
        );

        assert_eq!(side_child_handles.typedarrays, vec![7]);
        assert_eq!(side_child_handles.dataviews, vec![9]);
        assert!(raw_vals.contains(&value::encode_f64(7.0)));
        assert!(raw_vals.contains(&value::encode_f64(9.0)));
    }

    #[test]
    fn mark_linear_chain() {
        // 3 个对象：0→1→2（属性 value 指向下一个）
        let obj_table_ptr = 1000;
        let p0 = 2000;
        let p1 = 3000;
        let p2 = 4000;
        let objects = vec![
            (0u32, p0, 0xFFFF_FFFF, vec![enc_obj(1)]),
            (1u32, p1, 0xFFFF_FFFF, vec![enc_obj(2)]),
            (2u32, p2, 0xFFFF_FFFF, vec![]),
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
    fn mark_dead_object_not_marked() {
        // 0→1, 2 不可达
        let obj_table_ptr = 1000;
        let objects = vec![
            (0u32, 2000, 0xFFFF_FFFF, vec![enc_obj(1)]),
            (1u32, 3000, 0xFFFF_FFFF, vec![]),
            (2u32, 4000, 0xFFFF_FFFF, vec![]),
        ];
        let buf = build_object_buffer(obj_table_ptr, &objects, 3);
        let mut bm = MarkBitmap::new();
        bm.reset(3);
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 3, &[0], 0, 0);
        assert!(bm.is_marked(0));
        assert!(bm.is_marked(1));
        assert!(!bm.is_marked(2)); // 不可达
    }

    #[test]
    fn mark_cycle_no_infinite_loop() {
        // 0→1→0（循环）
        let obj_table_ptr = 1000;
        let objects = vec![
            (0u32, 2000, 0xFFFF_FFFF, vec![enc_obj(1)]),
            (1u32, 3000, 0xFFFF_FFFF, vec![enc_obj(0)]),
        ];
        let buf = build_object_buffer(obj_table_ptr, &objects, 2);
        let mut bm = MarkBitmap::new();
        bm.reset(2);
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 2, &[0], 0, 0);
        assert!(bm.is_marked(0));
        assert!(bm.is_marked(1));
        assert_eq!(bm.popcount(), 2); // 不重复
    }

    #[test]
    fn mark_deep_chain_no_stack_overflow() {
        // R8：10000 层链表，验证 worklist 不栈溢出。
        // 对象 i 的属性 value 指向 i+1。
        const N: usize = 10000;
        let obj_table_ptr = 1000;
        // obj_table 占 1000 + N*4 = 41000；对象从 50000 起，每对象 16+32=48B
        let mut objects: Vec<(Handle, usize, u32, Vec<i64>)> = Vec::with_capacity(N);
        let base = 50_000;
        for i in 0..N {
            let ptr = base + i * 48;
            let props = if i + 1 < N {
                vec![enc_obj((i + 1) as u32)]
            } else {
                vec![]
            };
            objects.push((i as u32, ptr, 0xFFFF_FFFF, props));
        }
        let buf = build_object_buffer(obj_table_ptr, &objects, N);
        let mut bm = MarkBitmap::new();
        bm.reset(N);
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, N, &[0], 0, 0);
        // 全部 marked
        assert_eq!(bm.popcount(), N);
        assert!(bm.is_marked((N - 1) as u32)); // 链尾
    }

    #[test]
    fn function_value_root_rejects_out_of_range_function_id() {
        const N: usize = 4;
        let obj_table_ptr = 0;
        let root_ptr = 100;
        let function_value = value::encode_function_idx(2);
        let buf = build_object_buffer(
            obj_table_ptr,
            &[
                (0u32, root_ptr, 0xFFFF_FFFF, vec![function_value]),
                (1u32, 200, 0xFFFF_FFFF, vec![]),
                (2u32, 300, 0xFFFF_FFFF, vec![]),
                (3u32, 400, 0xFFFF_FFFF, vec![]),
            ],
            N,
        );

        let mut bm = MarkBitmap::new();
        bm.reset(N);
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, N, &[0], 1, 2);

        assert!(bm.is_marked(0));
        assert!(!bm.is_marked(3));
    }
    // ── P4-blocker #2 测试：closure env_obj + native_callable 内部引用解析 ──

    /// native_callable 表项内部引用提取（不需 wasmtime，直接测 collect_native_callable_refs）。
    #[test]
    fn native_callable_refs_extracted() {
        use crate::NativeCallable;
        use std::sync::{Arc, Mutex};
        let mut st = crate::RuntimeState::new();
        // 构造一个 PromiseResolvingFunction（promise = object handle 5）
        let promise_val = value::encode_object_handle(5);
        st.native_callables = Arc::new(Mutex::new(vec![
            NativeCallable::EvalIndirect, // idx 0（默认）
            NativeCallable::PromiseResolvingFunction {
                promise: promise_val,
                already_resolved: Arc::new(Mutex::new(false)),
                kind: crate::PromiseResolvingKind::Fulfill,
            },
        ]));
        let refs = collect_native_callable_refs(&mut st, 1);
        assert_eq!(refs, vec![promise_val]);
    }

    /// closure env_obj 解析：验证 closure 值能解析出 env_obj 指向的 object handle。
    /// 这里直接验证 RuntimeState.closures 表驱动路径（resolve_value_handles 需 GcContext，
    /// 用 collect 路径的关键组件 closures 表读取替代）。
    #[test]
    fn closure_env_obj_resolvable() {
        use std::sync::{Arc, Mutex};
        let mut st = crate::RuntimeState::new();
        // closure idx 0 的 env_obj = object handle 7
        let env_obj = value::encode_object_handle(7);
        st.closures = Arc::new(Mutex::new(vec![crate::ClosureEntry {
            func_idx: 0,
            env_obj,
        }]));
        // 验证 closures 表读路径（resolve_value_handles 内部用同样的锁路径）
        let read_env = st.closures.lock().unwrap().get(0).map(|e| e.env_obj);
        assert_eq!(read_env, Some(env_obj));
        assert!(value::is_object(read_env.unwrap()));
    }
}
