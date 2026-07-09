//! Build-time support module emitter.
//!
//! 产出"helper-only"的 wasm 模块，由 `wjsm-runtime-support/build.rs` 调用并
//! 用 `wasmtime::Engine::precompile_module` 预编译为 `support.cwasm`。
//!
//! ABI 边界来源：`wjsm-runtime-support::abi`（不直接依赖以避免循环）。
//!
//! - `obj_new`/`obj_get`/`obj_set`/`obj_delete`/`string_eq`/`to_int32`：✅ 真实 body
//! - `arr_new`/`elem_get`/`elem_set`/`get_proto_from_ctor`：✅ 真实 body（P2.4+P2.5 完成）
//! - `wjsm_bootstrap_once`/`wjsm_init_function_props`：占位 `unreachable`，待 P2.6 迁移
//!
//! support module 的 global 索引与 user wasm 完全对齐（0..26），
//! 使 helper body 移植时 GlobalGet/GlobalSet 索引无需修改。

use crate::shared_types::build_shared_type_section;
use anyhow::Result;
use wasm_encoder::{
    BlockType, CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    GlobalType, ImportSection, Instruction as WasmInstruction, MemArg, MemoryType, Module, RefType,
    TableType, ValType,
};
use wjsm_ir::constants;
use wjsm_ir::value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GcFlavor {
    MarkSweep,
    G1,
    Zgc,
}

impl GcFlavor {
    pub fn artifact_suffix(self) -> &'static str {
        match self {
            Self::MarkSweep => "mark_sweep",
            Self::G1 => "g1",
            Self::Zgc => "zgc",
        }
    }
}

// ── Type indices（与 user wasm compiler_core.rs 的 type section 完全一致）──
// support module 必须使用与 user wasm 相同的 type index，否则 wasmtime 的
// call_indirect 会因 type index 不匹配而 trap（即使签名相同）。
const TY_OBJ_NEW: u32 = 7; // (i32) -> i32
const TY_OBJ_GET: u32 = 8; // (i64, i32) -> i64
const TY_OBJ_SET: u32 = 9; // (i64, i32, i64) -> ()
const TY_STRING_EQ: u32 = 26; // (i32, i32) -> i32
const TY_TO_INT32: u32 = 10; // (i64) -> i32
const TY_GET_PROTO: u32 = 3; // (i64) -> i64
const TY_BOOTSTRAP: u32 = 4; // () -> i64
#[allow(dead_code)]
const TY_CALL_INDIRECT: u32 = crate::shared_types::JS_FUNC_TYPE_INDEX; // (i64, i64, i32, i32) -> i64

// ── Host function imports ─────────────────────────────────────────────
// support module 通过 `env` namespace import 它需要的 host 函数。
// wasmtime Linker 已为所有 `env.*` host 函数注册实现。
// 顺序决定 function import 索引（0..N-1），defined functions 从 N 开始。
const HOST_IMPORTS: &[(&str, u32)] = &[
    ("gc_safepoint_poll", 1),             // () -> ()
    ("gc_alloc_slow", 35),                // (i32, i32, i32) -> i32
    ("gc_take_freed_handle", 36),         // () -> i32
    ("closure_get_func", 14),             // (i32) -> i32
    ("closure_get_env", 15),              // (i32) -> i64
    ("native_call", 12),                  // (i64, i64, i32, i32) -> i64
    ("proxy_trap_get", 8),                // (i64, i32) -> i64
    ("proxy_trap_set", 9),                // (i64, i32, i64) -> ()
    ("proxy_trap_delete", 8),             // (i64, i32) -> i64
    ("native_callable_get_property", 8),  // (i64, i32) -> i64
    ("primitive_bigint_get_method", 8),   // (i64, i32) -> i64
    ("primitive_number_get_method", 8),   // (i64, i32) -> i64
    ("primitive_symbol_get_property", 8), // (i64, i32) -> i64
    ("symbol_property_key", 10),          // (i64) -> i32
    ("obj_get_by_index", 8),              // (i64, i32) -> i64
    ("typedarray_set_by_index", 32),      // (i64, i32, i64) -> i64
    ("array_set_length", 2),              // (i64, i64) -> i64
    ("array_named_get", 8),               // (i64, i32) -> i64
    ("array_named_set", 9),               // (i64, i32, i64) -> ()
    ("primitive_regexp_get_property", 8), // (i64, i32) -> i64
    ("primitive_regexp_set_property", 9), // (i64, i32, i64) -> ()
    ("primitive_string_get_property", 8), // (i64, i32) -> i64
    ("obj_get_runtime_key", 8),           // (i64, i32) -> i64
    ("obj_set_runtime_key", 9),           // (i64, i32, i64) -> ()
    ("obj_delete_runtime_key", 8),        // (i64, i32) -> i64
    ("gc_barrier_flush", 1),              // () -> ()
    ("gc_load_barrier_slow", 14),         // (i32) -> i32
];

// Host import function indices（在 support module 的 function index space 中）
const HOST_GC_SAFEPOINT_POLL: u32 = 0;
const HOST_GC_ALLOC_SLOW: u32 = 1;
const HOST_GC_TAKE_FREED_HANDLE: u32 = 2;
const HOST_CLOSURE_GET_FUNC: u32 = 3;
const HOST_CLOSURE_GET_ENV: u32 = 4;
const HOST_NATIVE_CALL: u32 = 5;
const HOST_PROXY_TRAP_GET: u32 = 6;
const HOST_PROXY_TRAP_SET: u32 = 7;
const HOST_PROXY_TRAP_DELETE: u32 = 8;
const HOST_NATIVE_CALLABLE_GET_PROPERTY: u32 = 9;
const HOST_PRIMITIVE_BIGINT_GET_METHOD: u32 = 10;
const HOST_PRIMITIVE_NUMBER_GET_METHOD: u32 = 11;
const HOST_PRIMITIVE_SYMBOL_GET_PROPERTY: u32 = 12;
const HOST_SYMBOL_PROPERTY_KEY: u32 = 13;
const HOST_OBJ_GET_BY_INDEX: u32 = 14;
const HOST_TYPEDARRAY_SET_BY_INDEX: u32 = 15;
const HOST_ARRAY_SET_LENGTH: u32 = 16;
const HOST_ARRAY_NAMED_GET: u32 = 17;
const HOST_ARRAY_NAMED_SET: u32 = 18;
const HOST_PRIMITIVE_REGEXP_GET_PROPERTY: u32 = 19;
const HOST_PRIMITIVE_REGEXP_SET_PROPERTY: u32 = 20;
const HOST_PRIMITIVE_STRING_GET_PROPERTY: u32 = 21;
const HOST_OBJ_GET_RUNTIME_KEY: u32 = 22;
const HOST_OBJ_SET_RUNTIME_KEY: u32 = 23;
const HOST_OBJ_DELETE_RUNTIME_KEY: u32 = 24;

const HOST_GC_BARRIER_FLUSH: u32 = 25;
const HOST_GC_LOAD_BARRIER_SLOW: u32 = 26;
const NUM_HOST_IMPORTS: u32 = 27;

// ── Defined function indices ──────────────────────────────────────────
// 顺序与 SUPPORT_EXPORTS 一致；通过 export/import 调用（Call），不经 element section。
#[allow(dead_code)]
const FN_OBJ_NEW: u32 = NUM_HOST_IMPORTS;
const FN_OBJ_GET: u32 = NUM_HOST_IMPORTS + 1;
const FN_OBJ_SET: u32 = NUM_HOST_IMPORTS + 2;
#[allow(dead_code)]
const FN_OBJ_DELETE: u32 = NUM_HOST_IMPORTS + 3;
#[allow(dead_code)]
const FN_ARR_NEW: u32 = NUM_HOST_IMPORTS + 4;
#[allow(dead_code)]
const FN_ELEM_GET: u32 = NUM_HOST_IMPORTS + 5;
#[allow(dead_code)]
const FN_ELEM_SET: u32 = NUM_HOST_IMPORTS + 6;
const FN_STRING_EQ: u32 = NUM_HOST_IMPORTS + 7;
#[allow(dead_code)]
const FN_TO_INT32: u32 = NUM_HOST_IMPORTS + 8;
#[allow(dead_code)]
const FN_GET_PROTO: u32 = NUM_HOST_IMPORTS + 9;
#[allow(dead_code)]
const FN_BOOTSTRAP: u32 = NUM_HOST_IMPORTS + 10;
#[allow(dead_code)]
const FN_INIT_FUNC_PROPS: u32 = NUM_HOST_IMPORTS + 11;

// ── Global indices (与 user wasm 0..26 对齐) ──────────────────────────
#[allow(dead_code)]
const G_FUNC_PROPS: u32 = 0;
const G_HEAP_PTR: u32 = 1;
const G_OBJ_TABLE_PTR: u32 = 2;
const G_OBJ_TABLE_COUNT: u32 = 3;
const G_SHADOW_SP: u32 = 4;
#[allow(dead_code)]
const G_OBJECT_HEAP_START: u32 = 5;
const G_NUM_IR_FUNCTIONS: u32 = 6;
#[allow(dead_code)]
const G_SHADOW_STACK_END: u32 = 7;
const G_ARRAY_PROTO_HANDLE: u32 = 8;
const G_OBJECT_PROTO_HANDLE: u32 = 9;
#[allow(dead_code)]
const G_EVAL_VAR_MAP_PTR: u32 = 10;
#[allow(dead_code)]
const G_EVAL_VAR_MAP_COUNT: u32 = 11;
#[allow(dead_code)]
const G_BOOTSTRAP_DONE: u32 = 12;
#[allow(dead_code)]
const G_FUNCTION_PROPS_DONE: u32 = 13;
const G_FUNCTION_PROPS_BASE: u32 = 14;
#[allow(dead_code)]
const G_ARR_PROTO_TABLE_BASE: u32 = 15;
#[allow(dead_code)]
const G_ARR_PROTO_TABLE_LEN: u32 = 16;
#[allow(dead_code)]
const G_ARR_PROTO_TABLE_HASH: u32 = 17;
#[allow(dead_code)]
const G_ALLOC_PTR: u32 = 19;
#[allow(dead_code)]
const G_ALLOC_END: u32 = 20;
#[allow(dead_code)]
const G_GC_ALLOC_BYTES: u32 = 21;
#[allow(dead_code)]
const G_GC_TRIGGER_BYTES: u32 = 22;
#[allow(dead_code)]
const G_GC_PHASE: u32 = 23;
#[allow(dead_code)]
const G_GOOD_COLOR: u32 = 24;
#[allow(dead_code)]
const G_BARRIER_BUF_PTR: u32 = 25;
#[allow(dead_code)]
const G_BARRIER_BUF_END: u32 = 26;

fn emit_reference_barrier_event(
    func: &mut Function,
    flavor: GcFlavor,
    slot_addr_local: u32,
    slot_offset: u64,
    new_value_local: u32,
) {
    if !matches!(flavor, GcFlavor::G1 | GcFlavor::Zgc) {
        return;
    }

    if flavor == GcFlavor::Zgc {
        // ZGC 只需要 mark 期 SATB；idle/relocate 期不记录写屏障事件。
        func.instruction(&WasmInstruction::GlobalGet(G_GC_PHASE));
        func.instruction(&WasmInstruction::I32Const(1));
        func.instruction(&WasmInstruction::I32Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
    }

    // 缓冲区剩余不足 24B 时先 flush；flush 只 drain event，不触发 GC/move。
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::I32Const(
        constants::GC_BARRIER_EVENT_SIZE as i32,
    ));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_END));
    func.instruction(&WasmInstruction::I32GtU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Call(HOST_GC_BARRIER_FLUSH));
    func.instruction(&WasmInstruction::End);

    // flags:u32。G1 当前由 host 侧按 old/new 值精化；ZGC 标明 SATB old-value。
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::I32Const(if flavor == GcFlavor::Zgc {
        1
    } else {
        0
    }));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    // slot_addr:u32 指向实际 inline value slot。
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::LocalGet(slot_addr_local));
    if slot_offset != 0 {
        func.instruction(&WasmInstruction::I32Const(slot_offset as i32));
        func.instruction(&WasmInstruction::I32Add);
    }
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    // old_value:i64 必须取写前槽位的 NaN-boxed value。
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::LocalGet(slot_addr_local));
    func.instruction(&WasmInstruction::I64Load(MemArg {
        offset: slot_offset,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 8,
        align: 3,
        memory_index: 0,
    }));
    // new_value:i64 来自当前写入值，flush 不重读 slot。
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::LocalGet(new_value_local));
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 16,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::I32Const(
        constants::GC_BARRIER_EVENT_SIZE as i32,
    ));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_BARRIER_BUF_PTR));

    if flavor == GcFlavor::Zgc {
        func.instruction(&WasmInstruction::End);
    }
}

fn emit_resolve_handle_ptr(
    func: &mut Function,
    flavor: GcFlavor,
    handle_local: u32,
    ptr_local: u32,
) {
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_PTR));
    func.instruction(&WasmInstruction::LocalGet(handle_local));
    func.instruction(&WasmInstruction::I32Const(2));
    func.instruction(&WasmInstruction::I32Shl);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    if flavor == GcFlavor::Zgc {
        func.instruction(&WasmInstruction::LocalTee(ptr_local));
        func.instruction(&WasmInstruction::I32Const(3));
        func.instruction(&WasmInstruction::I32And);
        func.instruction(&WasmInstruction::GlobalGet(G_GOOD_COLOR));
        func.instruction(&WasmInstruction::I32Ne);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(handle_local));
        func.instruction(&WasmInstruction::Call(HOST_GC_LOAD_BARRIER_SLOW));
        func.instruction(&WasmInstruction::LocalSet(ptr_local));
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(ptr_local));
        func.instruction(&WasmInstruction::I32Const(-4));
        func.instruction(&WasmInstruction::I32And);
        func.instruction(&WasmInstruction::LocalSet(ptr_local));
    } else {
        func.instruction(&WasmInstruction::LocalSet(ptr_local));
    }
}

fn emit_obj_table_entry_value(func: &mut Function, flavor: GcFlavor, ptr_local: u32) {
    func.instruction(&WasmInstruction::LocalGet(ptr_local));
    if flavor == GcFlavor::Zgc {
        func.instruction(&WasmInstruction::GlobalGet(G_GOOD_COLOR));
        func.instruction(&WasmInstruction::I32Or);
    }
}

// Imported env globals — 与 abi::ENV_GLOBALS 同步：27 项。
const ENV_GLOBAL_IMPORTS: &[(&str, ValType, bool)] = &[
    ("__func_props", ValType::I32, true),
    ("__heap_ptr", ValType::I32, true),
    ("__obj_table_ptr", ValType::I32, true),
    ("__obj_table_count", ValType::I32, true),
    ("__shadow_sp", ValType::I32, true),
    ("__object_heap_start", ValType::I32, true),
    ("__num_ir_functions", ValType::I32, true),
    ("__shadow_stack_end", ValType::I32, true),
    ("__array_proto_handle", ValType::I32, true),
    ("__object_proto_handle", ValType::I32, true),
    ("__eval_var_map_ptr", ValType::I32, true),
    ("__eval_var_map_count", ValType::I32, true),
    ("__bootstrap_done", ValType::I32, true),
    ("__function_props_done", ValType::I32, true),
    ("__function_props_base", ValType::I32, true),
    ("__arr_proto_table_base", ValType::I32, true),
    ("__arr_proto_table_len", ValType::I32, true),
    ("__arr_proto_table_hash", ValType::I64, true),
    ("__heap_limit", ValType::I32, true),
    ("__alloc_ptr", ValType::I32, true),
    ("__alloc_end", ValType::I32, true),
    ("__gc_alloc_bytes", ValType::I32, true),
    ("__gc_trigger_bytes", ValType::I32, true),
    ("__gc_phase", ValType::I32, true),
    ("__good_color", ValType::I32, true),
    ("__barrier_buf_ptr", ValType::I32, true),
    ("__barrier_buf_end", ValType::I32, true),
];

/// 必须与 `wjsm-runtime-support::abi::SUPPORT_TABLE_RESERVED_LEN` 一致。
pub const SUPPORT_TABLE_RESERVED_LEN: u32 = 64;

// 12 个 defined functions 的 type index（顺序与 SUPPORT_EXPORTS 一致）
const HELPER_TYPE_INDICES: &[u32] = &[
    TY_OBJ_NEW,   // obj_new
    TY_OBJ_GET,   // obj_get
    TY_OBJ_SET,   // obj_set
    TY_OBJ_GET,   // obj_delete (same sig as obj_get)
    TY_OBJ_NEW,   // arr_new (same sig as obj_new)
    TY_OBJ_GET,   // elem_get (same sig as obj_get)
    TY_OBJ_SET,   // elem_set (same sig as obj_set)
    TY_STRING_EQ, // string_eq
    TY_TO_INT32,  // to_int32
    TY_GET_PROTO, // get_proto_from_ctor
    TY_BOOTSTRAP, // wjsm_bootstrap_once
    TY_BOOTSTRAP, // wjsm_init_function_props
];

const HELPER_EXPORT_NAMES: &[&str] = &[
    "obj_new",
    "obj_get",
    "obj_set",
    "obj_delete",
    "arr_new",
    "elem_get",
    "elem_set",
    "string_eq",
    "to_int32",
    "get_proto_from_ctor",
    "wjsm_bootstrap_once",
    "wjsm_init_function_props",
];

/// 生成指定 GC flavor 的 support module wasm bytes。
pub fn emit_support_module(flavor: GcFlavor) -> Result<Vec<u8>> {
    let mut module = Module::new();

    // ── Type section ──
    // 使用与 user wasm 完全一致的 type section（shared_types::build_shared_type_section）。
    // wasmtime 的 call_indirect 要求 type index 一致，不能仅签名一致。
    let types = build_shared_type_section();
    module.section(&types);

    // ── Import section ──
    let mut imports = ImportSection::new();
    imports.import(
        "env",
        "memory",
        EntityType::Memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        }),
    );
    imports.import(
        "env",
        wjsm_ir::SHADOW_MEMORY_NAME,
        EntityType::Memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        }),
    );
    imports.import(
        "env",
        "__table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            minimum: SUPPORT_TABLE_RESERVED_LEN as u64,
            maximum: None,
            table64: false,
            shared: false,
        }),
    );
    // 27 env globals
    for (name, ty, mutable) in ENV_GLOBAL_IMPORTS {
        imports.import(
            "env",
            name,
            EntityType::Global(GlobalType {
                val_type: *ty,
                mutable: *mutable,
                shared: false,
            }),
        );
    }
    // 26 host function imports
    for (name, type_idx) in HOST_IMPORTS {
        imports.import("env", name, EntityType::Function(*type_idx));
    }
    module.section(&imports);

    // ── Function section ──
    let mut functions = FunctionSection::new();
    for &type_idx in HELPER_TYPE_INDICES {
        functions.function(type_idx);
    }
    module.section(&functions);

    // ── Export section ──
    let mut exports = ExportSection::new();
    // support module 发起的 host import 仍通过 Caller::get_export 恢复 WasmEnv。
    // 重新 export 共享 env 句柄，保证 support-origin callback 与 user wasm
    // callback 看到同一份 memory/table/global contract。
    exports.export("memory", ExportKind::Memory, 0);
    exports.export(wjsm_ir::SHADOW_MEMORY_NAME, ExportKind::Memory, 1);
    exports.export("__table", ExportKind::Table, 0);
    for (idx, (name, _, _)) in ENV_GLOBAL_IMPORTS.iter().enumerate() {
        exports.export(name, ExportKind::Global, idx as u32);
    }
    for (i, &name) in HELPER_EXPORT_NAMES.iter().enumerate() {
        exports.export(name, ExportKind::Func, NUM_HOST_IMPORTS + i as u32);
    }
    module.section(&exports);

    // 不生成 element section：support module 的 helpers 通过 export + import 调用（Call），
    // 不通过 call_indirect。call_indirect 只用于 getter/setter 调用，
    // 这些 getter/setter 是 user wasm 的 JS 函数，在 table[64+] 中由 user wasm 的 element section 填充。

    // ── Code section ──
    let helper_bodies = vec![
        emit_obj_new(flavor),
        emit_obj_get(flavor),
        emit_obj_set(flavor),
        emit_obj_delete(flavor),
        emit_arr_new(flavor),
        emit_elem_get(flavor),
        emit_elem_set(flavor),
        emit_string_eq(flavor),
        emit_to_int32(flavor),
        emit_get_proto_from_ctor(flavor),
        emit_stub_unreachable(flavor), // wjsm_bootstrap_once
        emit_stub_unreachable(flavor), // wjsm_init_function_props
    ];
    let mut codes = CodeSection::new();
    for body in &helper_bodies {
        codes.function(body);
    }
    module.section(&codes);

    Ok(module.finish())
}

// ── Helper body emitters ──────────────────────────────────────────────

fn emit_stub_unreachable(_flavor: GcFlavor) -> Function {
    let mut func = Function::new(std::iter::empty::<(u32, ValType)>());
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
    func
}

/// handle bounds check：若 handle_idx >= obj_table_count 则返回 sentinel。
/// 保留 handle_idx 在 handle_local（通过 LocalTee）。
fn emit_handle_bounds_check(func: &mut Function, handle_local: u32, sentinel: Option<i64>) {
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_COUNT));
    func.instruction(&WasmInstruction::I32GeU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    if let Some(val) = sentinel {
        func.instruction(&WasmInstruction::I64Const(val));
    }
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(handle_local));
}

/// 新 handle 分配前检查：candidate 槽位不得越过 handle 表（止于 barrier 基址）。
fn emit_handle_table_alloc_check(func: &mut Function, candidate_local: u32) {
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_PTR));
    func.instruction(&WasmInstruction::LocalGet(candidate_local));
    func.instruction(&WasmInstruction::I32Const(4));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::I32Const(4));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalGet(G_BARRIER_BUF_PTR));
    func.instruction(&WasmInstruction::I32GtU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
}

/// 属性名 ID 匹配：先比较整数相等，若不等且两者都不是 Symbol 则调用 string_eq。
/// left_local / right_local 持有 name_id（i32）。
/// 结果（i32）留在栈顶：1 = 匹配，0 = 不匹配。
fn emit_property_name_id_match(func: &mut Function, left_local: u32, right_local: u32) {
    func.instruction(&WasmInstruction::LocalGet(left_local));
    func.instruction(&WasmInstruction::LocalGet(right_local));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::LocalGet(left_local));
    func.instruction(&WasmInstruction::I32Const(
        constants::NAME_ID_SYMBOL_FLAG as i32,
    ));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::LocalGet(right_local));
    func.instruction(&WasmInstruction::I32Const(
        constants::NAME_ID_SYMBOL_FLAG as i32,
    ));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Or);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I32)));
    func.instruction(&WasmInstruction::LocalGet(left_local));
    func.instruction(&WasmInstruction::LocalGet(right_local));
    func.instruction(&WasmInstruction::Call(FN_STRING_EQ));
    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::I32Or);
}

fn emit_runtime_string_name_id_test(func: &mut Function, name_id_local: u32) {
    func.instruction(&WasmInstruction::LocalGet(name_id_local));
    func.instruction(&WasmInstruction::I32Const(
        constants::NAME_ID_RUNTIME_STRING_FLAG as i32,
    ));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Ne);
}

/// 解析 callable：若为 closure 则通过 host 获取 func_idx + env_obj，
/// 否则 func_idx = callee_low32，env_obj = undefined。
fn emit_resolve_callable_for_helper(
    func: &mut Function,
    callee_local: u32,
    func_idx_local: u32,
    env_obj_local: u32,
) {
    func.instruction(&WasmInstruction::LocalGet(callee_local));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));

    func.instruction(&WasmInstruction::LocalGet(callee_local));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::Call(HOST_CLOSURE_GET_FUNC));
    func.instruction(&WasmInstruction::LocalSet(func_idx_local));
    func.instruction(&WasmInstruction::LocalGet(callee_local));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::Call(HOST_CLOSURE_GET_ENV));
    func.instruction(&WasmInstruction::LocalSet(env_obj_local));

    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::LocalGet(callee_local));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalSet(func_idx_local));
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::LocalSet(env_obj_local));
    func.instruction(&WasmInstruction::End);
}

/// 对象扩容 bump：fast-path 使用 alloc window；失败走 gc_alloc_slow。
fn emit_heap_bump_for_object_resize_support(
    func: &mut Function,
    capacity_local: u32,
    size_scratch_local: u32,
    new_ptr_local: u32,
) {
    func.instruction(&WasmInstruction::LocalGet(capacity_local));
    func.instruction(&WasmInstruction::I32Const(32));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(size_scratch_local));

    func.instruction(&WasmInstruction::Block(BlockType::Empty));
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_END));
    func.instruction(&WasmInstruction::I32LeU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalTee(new_ptr_local));
    func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalTee(size_scratch_local));
    func.instruction(&WasmInstruction::GlobalSet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
    func.instruction(&WasmInstruction::GlobalSet(G_HEAP_PTR));
    func.instruction(&WasmInstruction::GlobalGet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
    func.instruction(&WasmInstruction::LocalGet(new_ptr_local));
    func.instruction(&WasmInstruction::I32Sub);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::Br(1));
    func.instruction(&WasmInstruction::End);

    func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_OBJECT as i32));
    func.instruction(&WasmInstruction::LocalGet(capacity_local));
    func.instruction(&WasmInstruction::Call(HOST_GC_ALLOC_SLOW));
    func.instruction(&WasmInstruction::LocalTee(new_ptr_local));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
}

fn emit_gc_safepoint_poll_if_due_support(func: &mut Function) {
    // support helper 可在 bootstrap/function-props 构造期被调用；这些路径没有普通 IR
    // safepoint spill，因此必须等两段启动初始化完成后才允许增量 GC safepoint。
    func.instruction(&WasmInstruction::GlobalGet(G_BOOTSTRAP_DONE));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::GlobalGet(G_FUNCTION_PROPS_DONE));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::GlobalGet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::GlobalGet(G_GC_TRIGGER_BYTES));
    func.instruction(&WasmInstruction::I32GeU);
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Call(HOST_GC_SAFEPOINT_POLL));
    func.instruction(&WasmInstruction::End);
}

// ── obj_new (param $capacity i32) (result i32) — Type 0 ──
// 移植自 compiler_helpers.rs::compile_object_helpers obj_new 段。
fn emit_obj_new(flavor: GcFlavor) -> Function {
    // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx, local 4 = new_end
    let mut func = Function::new(vec![(4, ValType::I32)]);
    emit_gc_safepoint_poll_if_due_support(&mut func);

    // size = 16 + capacity * 32
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Const(32));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(1));

    // handle 复用：gc_take_freed_handle(); 若 == -1 则新分配
    func.instruction(&WasmInstruction::Call(HOST_GC_TAKE_FREED_HANDLE));
    func.instruction(&WasmInstruction::LocalTee(3));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_COUNT));
    func.instruction(&WasmInstruction::LocalTee(3));
    emit_handle_table_alloc_check(&mut func, 3);
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_OBJ_TABLE_COUNT));
    func.instruction(&WasmInstruction::End);

    // alloc window fast-path：alloc_ptr + size <= alloc_end
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_END));
    func.instruction(&WasmInstruction::I32LeU);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I32)));
    // fast-path
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalTee(4));
    func.instruction(&WasmInstruction::GlobalSet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(4));
    func.instruction(&WasmInstruction::GlobalSet(G_HEAP_PTR));
    func.instruction(&WasmInstruction::GlobalGet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Else);
    // slow-path：gc_alloc_slow(size, HEAP_TYPE_OBJECT, capacity)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_OBJECT as i32));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::Call(HOST_GC_ALLOC_SLOW));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalSet(2));

    // 初始化对象 header
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::GlobalGet(G_OBJECT_PROTO_HANDLE));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    // type byte HEAP_TYPE_OBJECT (0x00) at offset 4
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Store8(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    // Zero pad bytes 5-7
    for off in [5u64, 6, 7] {
        func.instruction(&WasmInstruction::LocalGet(2));
        func.instruction(&WasmInstruction::I32Const(0));
        func.instruction(&WasmInstruction::I32Store8(MemArg {
            offset: off,
            align: 0,
            memory_index: 0,
        }));
    }
    // capacity at offset 8
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    // num_props = 0 at offset 12
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));

    // obj_table[handle_idx] = ptr
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(4));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_PTR));
    func.instruction(&WasmInstruction::I32Add);
    emit_obj_table_entry_value(&mut func, flavor, 2);
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));

    // 返回 handle_idx
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::End);
    func
}

// obj_get / obj_set / obj_delete 由独立模块 emit_object_helpers_bodies.rs 提供，
// 避免单文件过长。以下 include! 将它们的 Function 返回值直接嵌入。
//
// 这些函数移植自 compiler_helpers.rs，所有 GlobalGet/GlobalSet 索引不变
// （与 user wasm 0..26 对齐），所有 host Call 替换为 support module 的
// host import 索引，string_eq Call 替换为 FN_STRING_EQ。

include!("support_object_helpers.rs");

// ── string_eq (param $a i32) (param $b i32) (result i32) — Type 3 ──
// 移植自 compiler_helpers.rs::compile_object_helpers str_eq 段。
fn emit_string_eq(_flavor: GcFlavor) -> Function {
    // local 0 = a, local 1 = b, local 2 = byte_a, local 3 = byte_b
    let mut func = Function::new(vec![(2, ValType::I32)]);
    func.instruction(&WasmInstruction::Block(BlockType::Empty));
    func.instruction(&WasmInstruction::Loop(BlockType::Empty));

    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalTee(3));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(1));
    func.instruction(&WasmInstruction::Br(0));

    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::End);
    func
}

// ── to_int32 (param $val i64) (result i32) — Type 4 ──
// 移植自 compiler_helpers.rs::compile_object_helpers to_int32 段。
fn emit_to_int32(_flavor: GcFlavor) -> Function {
    // local 0 = $val (i64, input), local 1 = f64 scratch
    let mut func = Function::new(vec![(1, ValType::F64)]);

    // is_f64: (val & BOX_BASE) != BOX_BASE
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
    func.instruction(&WasmInstruction::I64Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));

    // raw f64 path
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::F64ReinterpretI64);
    func.instruction(&WasmInstruction::LocalTee(1));

    // NaN check: f != f → 0
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // ±Inf check: |f| == inf → 0
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Abs);
    func.instruction(&WasmInstruction::F64Const(f64::INFINITY.into()));
    func.instruction(&WasmInstruction::F64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // |f| < 2^31 → safe i32.trunc_f64_s
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Abs);
    func.instruction(&WasmInstruction::F64Const(2_147_483_648.0_f64.into()));
    func.instruction(&WasmInstruction::F64Lt);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32TruncF64S);
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // |f| < 2^53 → i64 trunc + low 32 bits
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Abs);
    func.instruction(&WasmInstruction::F64Const(
        9_007_199_254_740_992.0_f64.into(),
    ));
    func.instruction(&WasmInstruction::F64Lt);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64TruncF64S);
    func.instruction(&WasmInstruction::I64Const(0xFFFF_FFFF));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // 大数 path：mod = f - trunc(f / 2^32) * 2^32，负数加 2^32
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Const(4_294_967_296.0_f64.into()));
    func.instruction(&WasmInstruction::F64Div);
    func.instruction(&WasmInstruction::F64Trunc);
    func.instruction(&WasmInstruction::F64Const(4_294_967_296.0_f64.into()));
    func.instruction(&WasmInstruction::F64Mul);
    func.instruction(&WasmInstruction::F64Neg);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Add);
    func.instruction(&WasmInstruction::LocalTee(1));

    // mod < 0 → +2^32
    func.instruction(&WasmInstruction::F64Const(0.0_f64.into()));
    func.instruction(&WasmInstruction::F64Lt);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64Const(4_294_967_296.0_f64.into()));
    func.instruction(&WasmInstruction::F64Add);
    func.instruction(&WasmInstruction::LocalSet(1));
    func.instruction(&WasmInstruction::End);

    // mod ∈ [0, 2^32) → 无符号截断
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32TruncF64U);
    func.instruction(&WasmInstruction::Return);

    func.instruction(&WasmInstruction::End); // end of "is raw f64" if

    // 非 raw number（sentinel）→ 0
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::End); // function end
    func
}

// ── arr_new (param $capacity i32) (result i32) — Type 7 ──
// 移植自 compiler_array_helpers.rs::compile_array_helpers arr_new 段。
// 数组内存布局: [proto(4), type(1), pad(3), length(4), capacity(4), elements(capacity*8)]
fn emit_arr_new(flavor: GcFlavor) -> Function {
    // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx, local 4 = new_end
    let mut func = Function::new(vec![(4, ValType::I32)]);
    emit_gc_safepoint_poll_if_due_support(&mut func);

    // size = 16 + capacity * 8
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(1));

    // ── handle 复用 ──
    func.instruction(&WasmInstruction::Call(HOST_GC_TAKE_FREED_HANDLE));
    func.instruction(&WasmInstruction::LocalTee(3));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_COUNT));
    func.instruction(&WasmInstruction::LocalTee(3));
    emit_handle_table_alloc_check(&mut func, 3);
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_OBJ_TABLE_COUNT));
    func.instruction(&WasmInstruction::End);

    // ── alloc window fast-path ──
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_END));
    func.instruction(&WasmInstruction::I32LeU);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I32)));
    // fast-path
    func.instruction(&WasmInstruction::GlobalGet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalTee(4));
    func.instruction(&WasmInstruction::GlobalSet(G_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(4));
    func.instruction(&WasmInstruction::GlobalSet(G_HEAP_PTR));
    func.instruction(&WasmInstruction::GlobalGet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Else);
    // slow-path：gc_alloc_slow(size, HEAP_TYPE_ARRAY, capacity)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::Call(HOST_GC_ALLOC_SLOW));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalSet(2));

    // ── 初始化数组 header ──
    // proto = array_proto_handle at offset 0
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::GlobalGet(G_ARRAY_PROTO_HANDLE));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: constants::HEAP_OBJECT_PROTO_OFFSET as u64,
        align: 2,
        memory_index: 0,
    }));
    // type byte HEAP_TYPE_ARRAY at layout-defined offset
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
    func.instruction(&WasmInstruction::I32Store8(MemArg {
        offset: constants::HEAP_OBJECT_TYPE_OFFSET as u64,
        align: 0,
        memory_index: 0,
    }));
    // Zero pad bytes
    for off in constants::HEAP_OBJECT_HEADER_PAD_START..constants::HEAP_OBJECT_HEADER_PAD_END {
        func.instruction(&WasmInstruction::LocalGet(2));
        func.instruction(&WasmInstruction::I32Const(0));
        func.instruction(&WasmInstruction::I32Store8(MemArg {
            offset: off as u64,
            align: 0,
            memory_index: 0,
        }));
    }
    // length = 0
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
        align: 2,
        memory_index: 0,
    }));
    // capacity = capacity (param 0)
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: constants::HEAP_ARRAY_CAPACITY_OFFSET as u64,
        align: 2,
        memory_index: 0,
    }));

    // ── obj_table[handle_idx] = ptr ──
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(4));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_PTR));
    func.instruction(&WasmInstruction::I32Add);
    emit_obj_table_entry_value(&mut func, flavor, 2);
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));

    // 返回 handle_idx
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::End);
    func
}

// ── elem_get (param $boxed i64) (param $index i32) (result i64) — Type 8 ──
// 移植自 compiler_array_helpers.rs::compile_array_helpers elem_get 段。
fn emit_elem_get(flavor: GcFlavor) -> Function {
    let mut func = Function::new(vec![(2, ValType::I32)]);
    // 检查是否为 TAG_ARRAY
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
    // Array path: resolve handle → ptr
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalSet(3));
    emit_resolve_handle_ptr(&mut func, flavor, 3, 2);
    func.instruction(&WasmInstruction::LocalGet(2));
    // ptr == 0 → return undefined
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::Else);
    // 读取 length (offset 8)
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(3));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32LtU);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
    // 读取 elements[index] at ptr + 16 + index * 8
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::I64Load(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::Else);
    // 不是 TAG_ARRAY → 委托给 obj_get_by_index
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_OBJ_GET_BY_INDEX));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func
}

// ── elem_set (param $boxed i64) (param $index i32) (param $value i64) — Type 9 ──
// 移植自 compiler_array_helpers.rs::compile_array_helpers elem_set 段。
fn emit_elem_set(flavor: GcFlavor) -> Function {
    let mut func = Function::new(vec![(3, ValType::I32)]);
    // 检查 TAG_ARRAY
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    // Array path: resolve handle → ptr
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalSet(4));
    emit_resolve_handle_ptr(&mut func, flavor, 4, 3);
    func.instruction(&WasmInstruction::LocalGet(3));
    // ptr == 0 → no-op
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 读取 length (offset 8)
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(4));
    // 写入 elements[index] = value at ptr + 16 + index * 8
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(5));
    emit_reference_barrier_event(&mut func, flavor, 5, 0, 2);
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
    // 更新 length 如果 index >= length
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(4));
    func.instruction(&WasmInstruction::I32GeU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::Else);
    // 不是 TAG_ARRAY → TypedArray 数字索引由宿主处理
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(HOST_TYPEDARRAY_SET_BY_INDEX));
    func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Else);
    // 普通对象的数字 key：symbol_property_key → obj_set
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::F64ConvertI32U);
    func.instruction(&WasmInstruction::I64ReinterpretF64);
    func.instruction(&WasmInstruction::Call(HOST_SYMBOL_PROPERTY_KEY));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(FN_OBJ_SET));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func
}

// ── get_proto_from_ctor (param $ctor i64) (result i64) — Type 3 ──
// GetPrototypeFromConstructor(F): 读取 F.prototype，若非 Object 类型则回退到 Object.prototype
fn emit_get_proto_from_ctor(_flavor: GcFlavor) -> Function {
    // local 0 = $ctor (i64), local 1 = $proto (i64)
    let mut func = Function::new(vec![(1, ValType::I64)]);
    // 调用 obj_get(ctor, "prototype") — "prototype" 的 name_id 在 support module
    // 是硬编码的 primordial 偏移 236（PRIMORDIAL_PROTOTYPE_OFFSET）
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Const(
        constants::PRIMORDIAL_PROTOTYPE_OFFSET as i32,
    ));
    func.instruction(&WasmInstruction::Call(FN_OBJ_GET));
    func.instruction(&WasmInstruction::LocalSet(1));
    // 检查 TAG_OBJECT (0x8)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_OBJECT as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 检查 TAG_FUNCTION (0x9)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 检查 TAG_CLOSURE (0xA)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 检查 TAG_ARRAY (0xB)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 检查 TAG_BOUND (0xC)
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::TAG_BOUND as i64));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 回退：返回 Object.prototype (Global 10)
    func.instruction(&WasmInstruction::GlobalGet(G_OBJECT_PROTO_HANDLE));
    func.instruction(&WasmInstruction::I64ExtendI32U);
    let box_base = value::BOX_BASE as i64;
    let tag_object = (value::TAG_OBJECT << 32) as i64;
    func.instruction(&WasmInstruction::I64Const(box_base | tag_object));
    func.instruction(&WasmInstruction::I64Or);
    func.instruction(&WasmInstruction::End);
    func
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_mark_sweep_support_module_produces_valid_wasm() {
        let bytes = emit_support_module(GcFlavor::MarkSweep).expect("emit");
        assert_eq!(&bytes[0..4], b"\0asm");
        assert_eq!(&bytes[4..8], &[0x01, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn emit_mark_sweep_support_module_passes_wasmparser_validation() {
        let bytes = emit_support_module(GcFlavor::MarkSweep).expect("emit");
        wasmparser::validate(&bytes).expect("support.wasm must validate");
    }

    #[test]
    fn g1_support_module_passes_wasmparser_validation() {
        let bytes = emit_support_module(GcFlavor::G1).expect("emit");
        wasmparser::validate(&bytes).expect("g1 support.wasm must validate");
    }

    #[test]
    fn zgc_support_module_passes_wasmparser_validation() {
        let bytes = emit_support_module(GcFlavor::Zgc).expect("emit");
        wasmparser::validate(&bytes).expect("zgc support.wasm must validate");
    }

    #[test]
    fn support_helper_signatures_count_locked() {
        assert_eq!(HELPER_TYPE_INDICES.len(), 12);
        assert_eq!(HELPER_EXPORT_NAMES.len(), 12);
    }

    #[test]
    fn env_global_imports_count_locked() {
        assert_eq!(ENV_GLOBAL_IMPORTS.len(), 27);
    }

    #[test]
    fn host_imports_count_locked() {
        assert_eq!(HOST_IMPORTS.len(), 27);
    }
}
