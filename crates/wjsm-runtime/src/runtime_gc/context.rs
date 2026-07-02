//! GcContext 桥接辅助 + HeapObjectQuery 运行时实现（spec §6/T3.3）。
//!
//! GcContext 本体定义在 api.rs（持 StoreContextMut + WasmEnv，不持 slice，#9）。
//! 本文件提供：
//! - `HeapMeta`：从 memory 现场读对象 header 的辅助（object_size/object_ptr/heap_type）。
//! - `obj_table` global 读取辅助。
use crate::runtime_gc::api::{GcContext, Handle};
use wasmtime::Val;
use wjsm_ir::constants;

/// 对象 header 常量（与 runtime_heap.rs / runtime_values.rs 一致）。
pub const HEADER_SIZE: usize = constants::HEAP_OBJECT_HEADER_SIZE as usize;
pub const OBJECT_ELEM_SIZE: usize = constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
pub const ARRAY_ELEM_SIZE: usize = constants::HEAP_ARRAY_ELEMENT_SIZE as usize;

/// GC 已知的堆对象布局分类（issue #119：禁止把未知 tag 静默当成 OBJECT 而不告警）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GcHeapLayout {
    /// 数组：len@+8，元素 8B。
    Array,
    /// 普通对象或 Arguments（属性槽布局与 OBJECT 相同）。
    ObjectLike,
}

/// 将 header 中的 heap_type 映射为 GC 扫描/计大小用的布局；未知 tag 会 debug_assert 并在 release 打日志。
pub(crate) fn gc_heap_layout(heap_type: u8) -> GcHeapLayout {
    match heap_type {
        wjsm_ir::HEAP_TYPE_ARRAY => GcHeapLayout::Array,
        wjsm_ir::HEAP_TYPE_OBJECT | wjsm_ir::HEAP_TYPE_ARGUMENTS => GcHeapLayout::ObjectLike,
        tag => {
            debug_assert!(
                false,
                "GC: unhandled heap_type 0x{tag:02x}; extend gc_heap_layout / mark / sweep"
            );
            #[cfg(not(debug_assertions))]
            eprintln!("wjsm GC warning: unhandled heap_type 0x{tag:02x}, assuming OBJECT layout");
            GcHeapLayout::ObjectLike
        }
    }
}

fn object_cap_and_elem_size(heap_type: u8) -> (usize, usize) {
    match gc_heap_layout(heap_type) {
        GcHeapLayout::Array => (
            constants::HEAP_ARRAY_CAPACITY_OFFSET as usize,
            ARRAY_ELEM_SIZE,
        ),
        GcHeapLayout::ObjectLike => (
            constants::HEAP_OBJECT_CAPACITY_OFFSET as usize,
            OBJECT_ELEM_SIZE,
        ),
    }
}

/// 从 memory 现场读对象 header，算对象总大小（HEADER + payload）。
///
/// 对象布局：proto(4) heap_type(1) pad(3) capacity(4) num_props/len(4) [payload]。
/// - OBJECT: capacity@+8, elem_size=32 → size = 16 + cap*32
/// - ARRAY:  capacity@+12, elem_size=8  → size = 16 + cap*8
///
/// 返回 None 表示 ptr 越界或 header 不可读（调用方应跳过）。
pub fn object_size_from_memory(data: &[u8], ptr: usize) -> Option<usize> {
    if ptr + HEADER_SIZE > data.len() {
        return None;
    }
    let heap_type = data[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize];
    let (cap_off, elem_size) = object_cap_and_elem_size(heap_type);
    let capacity = u32::from_le_bytes([
        data[ptr + cap_off],
        data[ptr + cap_off + 1],
        data[ptr + cap_off + 2],
        data[ptr + cap_off + 3],
    ]) as usize;
    let payload = capacity.checked_mul(elem_size)?;
    HEADER_SIZE.checked_add(payload)
}

/// 读对象 heap_type byte。
#[allow(dead_code)]
pub fn heap_type_from_memory(data: &[u8], ptr: usize) -> Option<u8> {
    data.get(ptr + 4).copied()
}

/// GcContext 上的堆元信息查询辅助。算法经 ctx.with_memory 调用这些方法。
impl<'a> GcContext<'a> {
    /// 读 obj_table_count global。
    pub fn obj_table_count(&mut self) -> usize {
        self.env
            .obj_table_count
            .get(&mut self.store)
            .i32()
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 obj_table base ptr global。
    pub fn obj_table_ptr(&mut self) -> usize {
        self.env
            .obj_table_ptr
            .get(&mut self.store)
            .i32()
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 heap_ptr global（下一个 bump 分配位置）。
    pub fn heap_ptr(&mut self) -> usize {
        self.env
            .heap_ptr
            .get(&mut self.store)
            .i32()
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 设置 heap_ptr global。
    pub fn set_heap_ptr(&mut self, val: usize) {
        let _ = self.env.heap_ptr.set(&mut self.store, Val::I32(val as i32));
    }

    /// 设置 obj_table_count global。
    #[allow(dead_code)]
    pub fn set_obj_table_count(&mut self, val: usize) {
        let _ = self
            .env
            .obj_table_count
            .set(&mut self.store, Val::I32(val as i32));
    }

    /// 读 shadow_sp global。
    pub fn shadow_sp(&mut self) -> usize {
        self.env
            .shadow_sp
            .get(&mut self.store)
            .i32()
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 shadow_stack_end global。
    pub fn shadow_stack_end(&mut self) -> usize {
        self.env
            .shadow_stack_end
            .and_then(|g| g.get(&mut self.store).i32())
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 num_ir_functions global。
    pub fn num_ir_functions(&mut self) -> usize {
        self.env
            .num_ir_functions
            .and_then(|g| g.get(&mut self.store).i32())
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 object_heap_start global（堆基址）。
    #[allow(dead_code)]
    pub fn object_heap_start(&mut self) -> usize {
        self.env
            .object_heap_start
            .and_then(|g| g.get(&mut self.store).i32())
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 function_props_base global（函数属性对象起始 handle）。
    pub fn function_props_base(&mut self) -> usize {
        self.env
            .function_props_base
            .and_then(|g| g.get(&mut self.store).i32())
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 array_proto_handle global。返回 None 表未初始化（-1）或不可读。
    pub fn array_proto_handle(&mut self) -> Option<Handle> {
        let h = self.env.array_proto_handle.get(&mut self.store).i32()?;
        if h < 0 { None } else { Some(h as Handle) }
    }

    /// 读 object_proto_handle global。
    pub fn object_proto_handle(&mut self) -> Option<Handle> {
        let h = self.env.object_proto_handle.get(&mut self.store).i32()?;
        if h < 0 { None } else { Some(h as Handle) }
    }

    /// 读 obj_table[h] → ptr。返回 None 表示越界或空槽（ptr==0）。
    #[allow(dead_code)]
    pub fn obj_table_slot(&mut self, data: &[u8], h: Handle) -> Option<usize> {
        let base = self.obj_table_ptr();
        let addr = base + h as usize * 4;
        if addr + 4 > data.len() {
            return None;
        }
        let ptr = u32::from_le_bytes([data[addr], data[addr + 1], data[addr + 2], data[addr + 3]])
            as usize;
        if ptr == 0 { None } else { Some(ptr) }
    }

    /// 写 obj_table[h] = ptr（INV-A：注册 handle）。
    #[allow(dead_code)]
    pub fn write_obj_table_slot(&mut self, h: Handle, ptr: usize) {
        let base = self.obj_table_ptr();
        let bytes = (ptr as u32).to_le_bytes();
        self.with_memory_mut(|data| {
            let addr = base + h as usize * 4;
            if addr + 4 <= data.len() {
                data[addr..addr + 4].copy_from_slice(&bytes);
            }
        });
    }
}
