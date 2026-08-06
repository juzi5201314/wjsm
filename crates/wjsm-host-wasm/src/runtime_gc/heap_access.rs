//! Host 侧 JS 堆读写统一入口（V2-only）。
//!
//! 全部转发到 `HeapAccessV2`；无 main-memory obj_table 路径。
//!
//! 隐藏类重构后属性写入只按 `(handle, name_id)` 寻址——堆内不再有属性槽序号，
//! 值槽下标由宿主 `ShapeTable` 分配，所以这里没有「按槽号写子槽」的接口。

use wasmtime::AsContextMut;

use crate::RuntimeState;
use crate::wasm_env::WasmEnv;

use super::api::Handle;
use super::api::Value;

/// 写数组元素槽。
pub fn write_element<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
    idx: usize,
    val: Value,
) -> Option<()> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    let index = u32::try_from(idx).ok()?;
    access.set_element(h, index, val as u64).ok()
}

/// 写 proto header。
pub fn write_proto<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    _env: &WasmEnv,
    h: Handle,
    proto: u32,
) -> Option<()> {
    let access = ctx.as_context().data().heap_access_v2().clone();
    access.set_prototype(h, proto).ok()
}
