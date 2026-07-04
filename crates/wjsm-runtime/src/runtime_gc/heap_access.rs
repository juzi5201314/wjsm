//! Host 侧 JS 堆读写统一入口（spec §13）。
//!
//! 本模块是 INV-C2 的 host owner：任何从 `obj_table` 解出的 raw ptr 都包在
//! `HeapPtr` 中，debug 构建会记录当前 `gc_epoch`，使用前确认期间没有 GC 点
//! 改写 `obj_table` 指针或颜色。写属性槽、元素槽和 proto header 时先读取旧值，
//! 再调用当前算法的 `on_host_write` hook，最后执行实际写入。

use wasmtime::AsContextMut;

use crate::RuntimeState;
use crate::wasm_env::WasmEnv;
use wjsm_ir::constants;
use wjsm_ir::value;

use super::api::{GcContext, Handle, Value};

const ZGC_COLOR_MASK: u32 = 0x3;
const PROTO_NULL_SENTINEL: u32 = 0xFFFF_FFFF;

/// 解引用后的 JS 堆指针。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapPtr {
    pub ptr: usize,
    #[cfg(debug_assertions)]
    epoch: u64,
}

impl HeapPtr {
    fn new(ptr: usize, ctx: &GcContext<'_>) -> Self {
        Self {
            ptr,
            #[cfg(debug_assertions)]
            epoch: ctx.gc_epoch(),
        }
    }

    /// 返回 raw ptr；debug 构建确认它没有跨越可能移动/重染色的 GC 点。
    pub fn get(&self, ctx: &mut GcContext<'_>) -> usize {
        #[cfg(debug_assertions)]
        debug_assert_eq!(self.epoch, ctx.gc_epoch(), "INV-C2: ptr crossed GC point");
        self.ptr
    }
}

/// 属性槽中承载 JS value 的三种 8B 子槽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotPart {
    Value,
    Getter,
    Setter,
}

impl SlotPart {
    fn offset(self) -> usize {
        match self {
            Self::Value => constants::PROP_SLOT_VALUE_OFFSET as usize,
            Self::Getter => constants::PROP_SLOT_GETTER_OFFSET as usize,
            Self::Setter => constants::PROP_SLOT_SETTER_OFFSET as usize,
        }
    }
}

/// 解 handle → ptr。ZGC relocate 期可由算法 hook 强制 heal。
pub fn resolve<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    h: Handle,
) -> Option<HeapPtr> {
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = GcContext::new(ctx, env, gc.name());
    let ptr = if let Some(ptr) = gc.on_host_resolve(&mut gc_ctx, h) {
        ptr
    } else {
        read_obj_table_ptr(&mut gc_ctx, h)?
    };
    Some(HeapPtr::new(ptr, &gc_ctx))
}

/// 写属性槽 value/getter/setter，统一经过算法写屏障 hook。
pub fn write_property_slot<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    h: Handle,
    slot_idx: usize,
    part: SlotPart,
    val: Value,
) -> Option<()> {
    let heap_ptr = resolve(ctx, env, h)?;
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = GcContext::new(ctx, env, gc.name());
    let ptr = heap_ptr.get(&mut gc_ctx);
    let slot_addr = ptr
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as usize)?
        .checked_add(slot_idx.checked_mul(constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize)?)?
        .checked_add(part.offset())?;
    let old = read_i64(&mut gc_ctx, slot_addr)?;
    gc.on_host_write(&mut gc_ctx, h, slot_addr, old, val);
    write_i64(&mut gc_ctx, slot_addr, val)
}

/// 写数组元素槽，统一经过算法写屏障 hook。
pub fn write_element<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    h: Handle,
    idx: usize,
    val: Value,
) -> Option<()> {
    let heap_ptr = resolve(ctx, env, h)?;
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = GcContext::new(ctx, env, gc.name());
    let ptr = heap_ptr.get(&mut gc_ctx);
    let slot_addr = ptr
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as usize)?
        .checked_add(idx.checked_mul(constants::HEAP_ARRAY_ELEMENT_SIZE as usize)?)?;
    let old = read_i64(&mut gc_ctx, slot_addr)?;
    gc.on_host_write(&mut gc_ctx, h, slot_addr, old, val);
    write_i64(&mut gc_ctx, slot_addr, val)
}

/// 旧 host API 只持短生命周期 ptr 时使用：先由 obj_table 反查 handle，再走 canonical 写入口。
pub fn write_element_at_ptr<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    idx: usize,
    val: Value,
) -> Option<()> {
    let h = handle_for_ptr(ctx, env, ptr)?;
    write_element(ctx, env, h, idx, val)
}

/// 写 proto header。proto 是对象 handle 或 `0xFFFF_FFFF` null 哨兵。
pub fn write_proto<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    h: Handle,
    proto: u32,
) -> Option<()> {
    let heap_ptr = resolve(ctx, env, h)?;
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = GcContext::new(ctx, env, gc.name());
    let ptr = heap_ptr.get(&mut gc_ctx);
    let slot_addr = ptr.checked_add(constants::HEAP_OBJECT_PROTO_OFFSET as usize)?;
    let old_proto = read_u32(&mut gc_ctx, slot_addr)?;
    let old_val = proto_handle_to_value(old_proto);
    let new_val = proto_handle_to_value(proto);
    gc.on_host_write(&mut gc_ctx, h, slot_addr, old_val, new_val);
    write_u32(&mut gc_ctx, slot_addr, proto)
}

/// 初始化尚未发布给 mutator 的对象 proto header；不触发 barrier。
pub fn init_proto_at_ptr<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
    proto: u32,
) -> Option<()> {
    let mut gc_ctx = GcContext::new(ctx, env, "heap-access-init");
    let slot_addr = ptr.checked_add(constants::HEAP_OBJECT_PROTO_OFFSET as usize)?;
    write_u32(&mut gc_ctx, slot_addr, proto)
}

fn handle_for_ptr<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    ptr: usize,
) -> Option<Handle> {
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let data = env.memory.data(&*ctx);
    for h in 0..obj_table_count {
        let slot = obj_table_ptr
            .checked_add(h.checked_mul(constants::HANDLE_TABLE_ENTRY_SIZE as usize)?)?;
        let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
        let entry = u32::from_le_bytes(bytes);
        if entry != 0 && (entry & !ZGC_COLOR_MASK) as usize == ptr {
            return Some(h as Handle);
        }
    }
    None
}

fn read_obj_table_ptr(ctx: &mut GcContext<'_>, h: Handle) -> Option<usize> {
    let obj_table_ptr = ctx.obj_table_ptr();
    let slot_addr =
        obj_table_ptr.checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)?;
    let entry = read_u32(ctx, slot_addr)?;
    if entry == 0 {
        return None;
    }
    Some((entry & !ZGC_COLOR_MASK) as usize)
}

fn proto_handle_to_value(proto: u32) -> Value {
    if proto == PROTO_NULL_SENTINEL {
        value::encode_null()
    } else if proto & 0x8000_0000 != 0 {
        value::encode_proxy_handle(proto & 0x7FFF_FFFF)
    } else {
        value::encode_object_handle(proto)
    }
}

fn read_u32(ctx: &mut GcContext<'_>, addr: usize) -> Option<u32> {
    ctx.with_memory(|data| {
        let bytes: [u8; 4] = data.get(addr..addr + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes))
    })
}

fn write_u32(ctx: &mut GcContext<'_>, addr: usize, val: u32) -> Option<()> {
    ctx.with_memory_mut(|data| {
        data.get_mut(addr..addr + 4)?
            .copy_from_slice(&val.to_le_bytes());
        Some(())
    })
}

fn read_i64(ctx: &mut GcContext<'_>, addr: usize) -> Option<Value> {
    ctx.with_memory(|data| {
        let bytes: [u8; 8] = data.get(addr..addr + 8)?.try_into().ok()?;
        Some(i64::from_le_bytes(bytes))
    })
}

fn write_i64(ctx: &mut GcContext<'_>, addr: usize, val: Value) -> Option<()> {
    ctx.with_memory_mut(|data| {
        data.get_mut(addr..addr + 8)?
            .copy_from_slice(&val.to_le_bytes());
        Some(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_part_offsets_match_property_slot_layout() {
        assert_eq!(SlotPart::Value.offset(), 8);
        assert_eq!(SlotPart::Getter.offset(), 16);
        assert_eq!(SlotPart::Setter.offset(), 24);
    }

    #[test]
    fn proto_handle_conversion_preserves_null_and_handle() {
        assert!(value::is_null(proto_handle_to_value(PROTO_NULL_SENTINEL)));
        assert_eq!(proto_handle_to_value(7), value::encode_object_handle(7));
    }
}
