use super::RuntimeState;
use wasmtime::*;

/// 打包所有 WASM 导出句柄（Memory / Table / Global 都是 wasmtime Copy 类型）。
/// 用于消除 _from_caller / _from_store 的代码重复。
#[derive(Clone, Copy)]
pub(crate) struct WasmEnv {
    pub memory: Memory,
    /// 独立影子栈线性内存（`env.__shadow_memory`）。
    pub shadow_memory: Memory,
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
    pub gc_alloc_bytes: Option<Global>,
    pub gc_trigger_bytes: Option<Global>,
    #[allow(dead_code)]
    pub gc_phase: Option<Global>,
    #[allow(dead_code)]
    pub good_color: Option<Global>,
    pub barrier_buf_ptr: Option<Global>,
    #[allow(dead_code)]
    pub barrier_buf_end: Option<Global>,
}

impl WasmEnv {
    /// 从 Caller 上下文中一次性提取所有导出句柄。
    ///
    /// 优先读取 `RuntimeState::cached_wasm_env`：该缓存在 instantiate 之后、任何
    /// host 调用之前由 `extract_wasm_env` 写入，且一个 Store 只有一份 env
    /// （动态加载的运行时模块经 `define_runtime_imports` 复用 caller 的
    /// memory / `__table` / env globals，句柄完全相同）。
    /// 回退到导出扫描仅覆盖缓存尚未安装的启动窗口——`get_export` 每个名字都要
    /// 走一次字符串哈希 + IndexMap 查找，28 个名字乘以每次回调，热路径不得付费。
    #[inline]
    pub fn from_caller(caller: &mut Caller<'_, RuntimeState>) -> Option<Self> {
        if let Some(env) = caller.data().cached_wasm_env {
            return Some(env);
        }
        Self::from_caller_exports(caller)
    }

    fn from_caller_exports(caller: &mut Caller<'_, RuntimeState>) -> Option<Self> {
        Some(Self {
            memory: caller.get_export("memory")?.into_memory()?,
            shadow_memory: caller
                .get_export(wjsm_ir::SHADOW_MEMORY_NAME)?
                .into_memory()?,
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
            gc_alloc_bytes: caller
                .get_export("__gc_alloc_bytes")
                .and_then(Extern::into_global),
            gc_trigger_bytes: caller
                .get_export("__gc_trigger_bytes")
                .and_then(Extern::into_global),
            gc_phase: caller
                .get_export("__gc_phase")
                .and_then(Extern::into_global),
            good_color: caller
                .get_export("__good_color")
                .and_then(Extern::into_global),
            barrier_buf_ptr: caller
                .get_export("__barrier_buf_ptr")
                .and_then(Extern::into_global),
            barrier_buf_end: caller
                .get_export("__barrier_buf_end")
                .and_then(Extern::into_global),
        })
    }
}
