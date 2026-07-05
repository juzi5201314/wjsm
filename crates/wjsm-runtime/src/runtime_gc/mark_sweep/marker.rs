//! Mark phase（spec §8.1，IMPL-6 worklist 不递归）。
//!
//! 对象引用槽扫描由 `runtime_gc::object_walker` 单一 owner 提供；本模块只保留
//! mark-sweep 的 worklist / bitmap 驱动，避免 G1/ZGC 再复制对象布局解析逻辑。

use crate::runtime_gc::api::{GcContext, Handle};
use crate::runtime_gc::mark_sweep::MarkSweepCollector;
use crate::runtime_gc::object_walker::ObjectWalker;

/// 标记 roots 并 drain worklist。
pub fn mark_roots_and_drain(
    collector: &mut MarkSweepCollector,
    ctx: &mut GcContext,
    roots: &mut dyn Iterator<Item = Handle>,
) {
    let mut worklist: Vec<Handle> = Vec::new();

    for h in roots {
        if collector.mark_bits.mark_if_new(h) {
            worklist.push(h);
        }
    }

    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    let mut walker = ObjectWalker::new();

    while let Some(h) = worklist.pop() {
        walker.visit_object_children(ctx, h, obj_table_ptr, obj_table_count, &mut |child| {
            if collector.mark_bits.mark_if_new(child) {
                worklist.push(child);
            }
        });
    }

    ctx.stats.marked = collector.mark_bits.popcount();
}

#[cfg(test)]
mod tests {
    use crate::runtime_gc::mark_bitmap::MarkBitmap;
    use crate::runtime_gc::object_walker::mark_drain_on_buffer;
    use wjsm_ir::{constants, value};

    fn build_object_buffer(
        obj_table_ptr: usize,
        objects: &[(u32, usize, u32, Vec<i64>)],
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

    fn enc_obj(h: u32) -> i64 {
        value::encode_object_handle(h)
    }

    #[test]
    fn marker_uses_shared_object_walker_for_linear_chain() {
        let obj_table_ptr = 1000;
        let objects = vec![
            (0u32, 2000, 0xFFFF_FFFF, vec![enc_obj(1)]),
            (1u32, 3000, 0xFFFF_FFFF, vec![enc_obj(2)]),
            (2u32, 4000, 0xFFFF_FFFF, vec![]),
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
    fn marker_shared_walker_keeps_deep_chain_iterative() {
        const N: usize = 10_000;
        let obj_table_ptr = 1000;
        let mut objects = Vec::with_capacity(N);
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

        assert_eq!(bm.popcount(), N);
        assert!(bm.is_marked((N - 1) as u32));
    }
}
