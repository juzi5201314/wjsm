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
    pub shadow_stack_end: Option<Global>,
    pub object_proto_handle: Global,
    pub array_proto_handle: Global,
    pub object_heap_start: Option<Global>,
    pub bootstrap_done: Option<Global>,
    pub function_props_done: Option<Global>,
    pub function_props_base: Option<Global>,
    #[allow(dead_code)]
    pub num_ir_functions: Option<Global>,
    pub arr_proto_table_base: Option<Global>,
    pub arr_proto_table_len: Option<Global>,
    pub arr_proto_table_hash: Option<Global>,
    pub heap_limit: Option<Global>,
    pub alloc_ptr: Option<Global>,
    pub alloc_end: Option<Global>,
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
            shadow_stack_end: caller
                .get_export("__shadow_stack_end")
                .and_then(Extern::into_global),
            object_proto_handle: caller.get_export("__object_proto_handle")?.into_global()?,
            array_proto_handle: caller.get_export("__array_proto_handle")?.into_global()?,
            object_heap_start: caller
                .get_export("__object_heap_start")
                .and_then(Extern::into_global),
            bootstrap_done: caller
                .get_export("__bootstrap_done")
                .and_then(Extern::into_global),
            function_props_done: caller
                .get_export("__function_props_done")
                .and_then(Extern::into_global),
            function_props_base: caller
                .get_export("__function_props_base")
                .and_then(Extern::into_global),
            num_ir_functions: caller
                .get_export("__num_ir_functions")
                .and_then(Extern::into_global),
            arr_proto_table_base: caller
                .get_export("__arr_proto_table_base")
                .and_then(Extern::into_global),
            arr_proto_table_len: caller
                .get_export("__arr_proto_table_len")
                .and_then(Extern::into_global),
            arr_proto_table_hash: caller
                .get_export("__arr_proto_table_hash")
                .and_then(Extern::into_global),
            heap_limit: caller
                .get_export("__heap_limit")
                .and_then(Extern::into_global),
            alloc_ptr: caller
                .get_export("__alloc_ptr")
                .and_then(Extern::into_global),
            alloc_end: caller
                .get_export("__alloc_end")
                .and_then(Extern::into_global),
        })
    }
}
