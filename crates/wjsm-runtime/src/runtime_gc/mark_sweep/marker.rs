//! Mark phase（spec §8.1，IMPL-6 worklist 不递归）。
//!
//! 移植自 runtime_heap.rs::mark_object_recursive_with_funcs（L577-761），
//! 但把 Rust 栈递归改为显式 worklist（Vec<Handle>），深对象图不栈溢出（R8/#11）。
//!
//! 算法：
//! 1. seed roots（调用方提供的 root 迭代器）
//! 2. drain worklist：对每个 handle，读对象 header 的子引用（proto/props/elements），
//!    提取子 handle（object/array/function → low32），若未标记则标记并入 worklist。
//!
//! fixed-point host 侧表追踪（spec §10）：由 collect_with_roots 的调用方在 P4 集成时
//! 经 roots 迭代器分轮注入（continuation_table.captured_vars 等顶层 root）。
//!
//! 借用结构：drain 循环交替两步——(a) ctx.with_memory 读子引用收集 candidate handles，
//! (b) 对每个 candidate，mark_bits.mark_if_new + push。mark_bits 是 collector 字段，
//! 与 ctx 借用独立，无冲突。
use crate::runtime_gc::api::{GcContext, Handle};
use crate::runtime_gc::mark_sweep::MarkSweepCollector;
use wjsm_ir::value;

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

    // drain
    while let Some(h) = worklist.pop() {
        // 收集本对象的子引用值（proto + props/elements），解析为 candidate handles。
        // 每次只读一个对象的子，借用周期短（单对象），无 grow。
        let candidates: Vec<Handle> = ctx.with_memory(|_caller, data| {
            collect_child_handles(data, h, obj_table_ptr, obj_table_count)
        });
        for child in candidates {
            if collector.mark_bits.mark_if_new(child) {
                worklist.push(child);
            }
        }
    }

    ctx.stats.marked = collector.mark_bits.popcount();
}

/// 读单个对象 h 的子引用，返回所有应标记的 child handle 列表。
///
/// 移植自 runtime_heap.rs:620-748（children 收集逻辑）：
/// - proto_handle（若有效）
/// - 数组：elements（value::tag_needs_root 过滤）
/// - 对象：每属性的 value/getter/setter（tag_needs_root 过滤）
/// - function (TAG_FUNCTION low32 < obj_table_count)：作为对象 handle
///
/// 过滤：只收集 tag_needs_root 的值，避免标量污染。closure/native_callable 经
/// host 侧表追踪（roots 注入），不在此解析（避免侵入 runtime side-table 内部结构）。
fn collect_child_handles(
    data: &[u8],
    h: Handle,
    obj_table_ptr: usize,
    obj_table_count: usize,
) -> Vec<Handle> {
    let mut out: Vec<Handle> = Vec::new();
    let obj_ptr = match resolve_handle(data, h, obj_table_ptr, obj_table_count) {
        Some(p) => p,
        None => return out,
    };
    if obj_ptr + 16 > data.len() {
        return out;
    }

    // proto_handle（offset 0..4）
    let proto_handle = u32::from_le_bytes([
        data[obj_ptr],
        data[obj_ptr + 1],
        data[obj_ptr + 2],
        data[obj_ptr + 3],
    ]);
    if proto_handle != 0xFFFF_FFFF && (proto_handle as usize) < obj_table_count {
        out.push(proto_handle);
    }

    // type byte 决定数组还是对象
    let heap_type = data[obj_ptr + 4];
    if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
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
            push_value_handle(elem, obj_table_count, &mut out);
        }
    } else {
        // 对象：属性槽 [name_id(4) flags(4) value(8) getter(8) setter(8)] = 32B
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
            // value@+8, getter@+16, setter@+24
            for val_off in [8usize, 16, 24] {
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
                push_value_handle(v, obj_table_count, &mut out);
            }
        }
    }
    out
}

/// 从 NaN-boxed value 提取应标记的 handle（object/array/function），追加到 out。
/// 移植自 runtime_heap.rs:collect_child_from_value（516-573）。
fn push_value_handle(val: i64, obj_table_count: usize, out: &mut Vec<Handle>) {
    if !value::tag_needs_root(val) {
        return;
    }
    if value::is_object(val) || value::is_array(val) {
        let h = value::decode_object_handle(val);
        if (h as usize) < obj_table_count {
            out.push(h);
        }
        return;
    }
    if value::is_function(val) {
        // TAG_FUNCTION low32 是函数表索引，同时也是 obj_table 下标（0..num_ir_functions）
        let h = (val as u32) as Handle;
        if (h as usize) < obj_table_count {
            out.push(h);
        }
        return;
    }
    // closure/native_callable/bigint/symbol/regexp/proxy/scope_record/iterator/enumerator/
    // runtime_string/exception：经 host 侧表追踪（roots 注入），不在此解析。
    // 这些值要么是 host 侧表的索引（非 obj_table handle），要么需要解析 side-table。
    // P4 集成时由 RootProvider::for_each_host_table_root 覆盖。
}

/// 读 obj_table[h] → ptr（None = 越界/空槽）。
fn resolve_handle(data: &[u8], h: Handle, obj_table_ptr: usize, obj_table_count: usize) -> Option<usize> {
    if (h as usize) >= obj_table_count {
        return None;
    }
    let addr = obj_table_ptr + h as usize * 4;
    if addr + 4 > data.len() {
        return None;
    }
    let ptr = u32::from_le_bytes([data[addr], data[addr + 1], data[addr + 2], data[addr + 3]]) as usize;
    if ptr == 0 {
        None
    } else {
        Some(ptr)
    }
}

// ── 测试辅助：纯 buffer 上的 worklist drain（不依赖 wasmtime，验证 R8 不栈溢出）──

/// 在给定 memory buffer + obj_table 布局上跑完整 mark（seed + drain worklist）。
/// 用于单元测试验证 worklist 正确性 + 深对象图不栈溢出（R8）。
#[cfg(test)]
pub(crate) fn mark_drain_on_buffer(
    mark_bits: &mut crate::runtime_gc::mark_bitmap::MarkBitmap,
    data: &[u8],
    obj_table_ptr: usize,
    obj_table_count: usize,
    roots: &[Handle],
) {
    let mut worklist: Vec<Handle> = Vec::new();
    for &h in roots {
        if mark_bits.mark_if_new(h) {
            worklist.push(h);
        }
    }
    while let Some(h) = worklist.pop() {
        let candidates = collect_child_handles(data, h, obj_table_ptr, obj_table_count);
        for child in candidates {
            if mark_bits.mark_if_new(child) {
                worklist.push(child);
            }
        }
    }
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
        for &(_h, ptr, _proto, ref props) in objects {
            let end = ptr + 16 + props.len() * 32;
            size = size.max(end);
        }
        let mut buf = vec![0u8; size];
        // 写 obj_table
        for &(h, ptr, _, _) in objects {
            let addr = obj_table_ptr + h as usize * 4;
            buf[addr..addr + 4].copy_from_slice(&(ptr as u32).to_le_bytes());
        }
        // 写对象 header + props
        for &(_h, ptr, proto, ref props) in objects {
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
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 3, &[0]);
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
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 3, &[0]);
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
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, 2, &[0]);
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
            let props = if i + 1 < N { vec![enc_obj((i + 1) as u32)] } else { vec![] };
            objects.push((i as u32, ptr, 0xFFFF_FFFF, props));
        }
        let buf = build_object_buffer(obj_table_ptr, &objects, N);
        let mut bm = MarkBitmap::new();
        bm.reset(N);
        mark_drain_on_buffer(&mut bm, &buf, obj_table_ptr, N, &[0]);
        // 全部 marked
        assert_eq!(bm.popcount(), N);
        assert!(bm.is_marked((N - 1) as u32)); // 链尾
    }
}
