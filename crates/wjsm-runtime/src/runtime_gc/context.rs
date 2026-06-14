//! GcContext 桥接辅助 + HeapObjectQuery 运行时实现（spec §6/T3.3）。
//!
//! GcContext 本体定义在 api.rs（持 Caller + Memory，不持 slice，#9）。
//! 本文件提供：
//! - `HeapMeta`：从 memory 现场读对象 header 的辅助（object_size/object_ptr/heap_type）。
//! - `obj_table` global 读取辅助。
use crate::runtime_gc::api::{GcContext, Handle};
use wasmtime::{Caller, Global, Val};

/// 对象 header 常量（与 runtime_heap.rs / runtime_values.rs 一致）。
pub const HEADER_SIZE: usize = 16;
pub const OBJECT_ELEM_SIZE: usize = 32; // 属性槽 [name_id(4) flags(4) value(8) getter(8) setter(8)]
pub const ARRAY_ELEM_SIZE: usize = 8; // NaN-boxed element

/// 读取一个 wasmtime global（i32）。
/// 注：Global::get 需要 AsContextMut（&mut Caller），因 wasmtime 的 store 借用模型。
pub fn read_i32_global(caller: &mut Caller<'_, crate::RuntimeState>, name: &str) -> Option<i32> {
    let g = caller.get_export(name)?;
    if let wasmtime::Extern::Global(global) = g {
        // Global::get 接收 impl AsContextMut，返回 Val（非 Result）。
        if let Val::I32(v) = global.get(caller) {
            return Some(v);
        }
    }
    None
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
    let heap_type = data[ptr + 4];
    let (cap_off, elem_size) = if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
        (12usize, ARRAY_ELEM_SIZE)
    } else {
        (8usize, OBJECT_ELEM_SIZE)
    };
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
pub fn heap_type_from_memory(data: &[u8], ptr: usize) -> Option<u8> {
    data.get(ptr + 4).copied()
}

/// GcContext 上的堆元信息查询辅助。算法经 ctx.with_memory 调用这些方法。
impl<'a, 'b> GcContext<'a, 'b> {
    /// 读 obj_table_count global（__obj_table_count）。
    pub fn obj_table_count(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__obj_table_count")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 obj_table base ptr global（__obj_table_ptr）。
    pub fn obj_table_ptr(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__obj_table_ptr")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 heap_ptr global（__heap_ptr，下一个 bump 分配位置）。
    pub fn heap_ptr(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__heap_ptr")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 设置 heap_ptr global。
    pub fn set_heap_ptr(&mut self, val: usize) {
        if let Some(Extern::Global(g)) = self.caller.get_export("__heap_ptr") {
            let _ = g.set(&mut *self.caller, Val::I32(val as i32));
        }
    }

    /// 设置 obj_table_count global。
    pub fn set_obj_table_count(&mut self, val: usize) {
        if let Some(Extern::Global(g)) = self.caller.get_export("__obj_table_count") {
            let _ = g.set(&mut *self.caller, Val::I32(val as i32));
        }
    }

    /// 读 shadow_sp global（__shadow_sp）。
    pub fn shadow_sp(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__shadow_sp")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 shadow_stack_end global（__shadow_stack_end）。
    pub fn shadow_stack_end(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__shadow_stack_end")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 num_ir_functions global（__num_ir_functions）。
    pub fn num_ir_functions(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__num_ir_functions")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 object_heap_start global（__object_heap_start，堆基址）。
    pub fn object_heap_start(&mut self) -> usize {
        read_i32_global(&mut *self.caller, "__object_heap_start")
            .unwrap_or(0)
            .max(0) as usize
    }

    /// 读 obj_table[h] → ptr。返回 None 表示越界或空槽（ptr==0）。
    /// 注：需先读 obj_table_ptr（&mut self），再 with_memory 读 data（不可同时 &mut caller）。
    pub fn obj_table_slot(&mut self, data: &[u8], h: Handle) -> Option<usize> {
        let base = self.obj_table_ptr();
        let addr = base + h as usize * 4;
        if addr + 4 > data.len() {
            return None;
        }
        let ptr = u32::from_le_bytes([data[addr], data[addr + 1], data[addr + 2], data[addr + 3]])
            as usize;
        if ptr == 0 {
            None
        } else {
            Some(ptr)
        }
    }

    /// 写 obj_table[h] = ptr（INV-A：注册 handle）。
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

use wasmtime::Extern;
