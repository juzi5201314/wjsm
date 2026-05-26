use super::RuntimeState;
use wasmtime::*;

/// 打包所有 WASM 导出句柄（Memory / Table / Global 都是 wasmtime Copy 类型）。
/// 用于消除 _from_caller / _from_store 的代码重复。
#[derive(Clone, Copy)]
pub(crate) struct WasmEnv {
    pub memory: Memory,
    pub func_table: Table,
    pub shadow_sp: Global,
    pub heap_ptr: Global,
    pub obj_table_ptr: Global,
    pub obj_table_count: Global,
    pub object_proto_handle: Global,
}

impl WasmEnv {
    /// 从 Caller 上下文中一次性提取所有导出句柄。
    pub fn from_caller(caller: &mut Caller<'_, RuntimeState>) -> Option<Self> {
        Some(Self {
            memory: caller.get_export("memory")?.into_memory()?,
            func_table: caller.get_export("__table")?.into_table()?,
            shadow_sp: caller.get_export("__shadow_sp")?.into_global()?,
            heap_ptr: caller.get_export("__heap_ptr")?.into_global()?,
            obj_table_ptr: caller.get_export("__obj_table_ptr")?.into_global()?,
            obj_table_count: caller.get_export("__obj_table_count")?.into_global()?,
            object_proto_handle: caller.get_export("__object_proto_handle")?.into_global()?,
        })
    }
}
