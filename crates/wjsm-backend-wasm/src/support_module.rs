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

// ── Host function imports ─────────────────────────────────────────────
// support module 通过 `env` namespace import 它需要的 host 函数。
// wasmtime Linker 已为所有 `env.*` host 函数注册实现。
// 顺序决定 function import 索引（0..N-1），defined functions 从 N 开始。
// V2 support 仅 import 实际调用的 host：safepoint / free-handle / slow alloc / 对象堆 host helpers。
// 顺序决定 function import 索引；defined helpers 从 NUM_HOST_IMPORTS 起。
const HOST_IMPORTS: &[(&str, u32)] = &[
    ("gc_safepoint_poll", 1),     // () -> ()
    ("gc_take_freed_handle", 36), // () -> i32
];

// Host import function indices（support module function index space）
const HOST_GC_SAFEPOINT_POLL: u32 = 0;
const HOST_GC_TAKE_FREED_HANDLE: u32 = 1;
const HOST_GC_ALLOC_SLOW: u32 = 2;
const HOST_GC_OBJ_GET: u32 = 3;
const HOST_GC_OBJ_SET: u32 = 4;
const HOST_GC_OBJ_DELETE: u32 = 5;
const HOST_GC_ARR_NEW: u32 = 6;
const HOST_GC_ELEM_GET: u32 = 7;
const HOST_GC_ELEM_SET: u32 = 8;
const NUM_HOST_IMPORTS: u32 = 9;

// ── Global indices（与 user wasm 对齐；仅列出 V2 support body 实际引用的）──
const G_OBJ_TABLE_COUNT: u32 = 3;
const G_OBJECT_PROTO_HANDLE: u32 = 9;
const G_BOOTSTRAP_DONE: u32 = 12;
const G_FUNCTION_PROPS_DONE: u32 = 13;
const G_GC_ALLOC_BYTES: u32 = 21;
const G_GC_TRIGGER_BYTES: u32 = 22;
const G_V2_HEAP_ALLOC_PTR: u32 = 27;
const G_V2_HEAP_ALLOC_END: u32 = 28;

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

const V2_HEAP_GLOBAL_IMPORTS: &[(&str, ValType, bool)] = &[
    (wjsm_ir::HEAP_ALLOC_PTR_GLOBAL_NAME, ValType::I64, true),
    (wjsm_ir::HEAP_ALLOC_END_GLOBAL_NAME, ValType::I64, true),
    (wjsm_ir::HEAP_OBJECT_START_GLOBAL_NAME, ValType::I64, true),
    (wjsm_ir::HEAP_LIMIT_GLOBAL_NAME, ValType::I64, true),
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

/// 生成指定 GC flavor 的 support module wasm bytes（memory64 ManagedHeap ABI）。
pub fn emit_support_module(flavor: GcFlavor) -> Result<Vec<u8>> {
    let mut module = Module::new();
    let types = build_shared_type_section();
    module.section(&types);

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
        wjsm_ir::HEAP_MEMORY_NAME,
        EntityType::Memory(MemoryType {
            minimum: wjsm_ir::HEAP_MEMORY_MIN_PAGES,
            maximum: Some(wjsm_ir::HEAP_MEMORY_MAX_PAGES),
            memory64: true,
            shared: true,
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
    for (name, ty, mutable) in V2_HEAP_GLOBAL_IMPORTS {
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
    for (name, type_idx) in HOST_IMPORTS {
        imports.import("env", name, EntityType::Function(*type_idx));
    }
    imports.import("env", "gc_alloc_slow", EntityType::Function(38));
    for (name, type_index) in [
        ("gc_obj_get", 8),
        ("gc_obj_set", 9),
        ("gc_obj_delete", 8),
        ("gc_arr_new", 7),
        ("gc_elem_get", 8),
        ("gc_elem_set", 9),
    ] {
        imports.import("env", name, EntityType::Function(type_index));
    }
    module.section(&imports);

    let mut functions = FunctionSection::new();
    for &type_idx in HELPER_TYPE_INDICES {
        functions.function(type_idx);
    }
    module.section(&functions);

    let num_host_imports = NUM_HOST_IMPORTS;
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export(wjsm_ir::SHADOW_MEMORY_NAME, ExportKind::Memory, 1);
    exports.export(
        wjsm_ir::HEAP_MEMORY_NAME,
        ExportKind::Memory,
        wjsm_ir::HEAP_MEMORY_INDEX,
    );
    exports.export("__table", ExportKind::Table, 0);
    for (idx, (name, _, _)) in ENV_GLOBAL_IMPORTS.iter().enumerate() {
        exports.export(name, ExportKind::Global, idx as u32);
    }
    for (offset, (name, _, _)) in V2_HEAP_GLOBAL_IMPORTS.iter().enumerate() {
        exports.export(
            name,
            ExportKind::Global,
            ENV_GLOBAL_IMPORTS.len() as u32 + offset as u32,
        );
    }
    for (i, &name) in HELPER_EXPORT_NAMES.iter().enumerate() {
        exports.export(name, ExportKind::Func, num_host_imports + i as u32);
    }
    module.section(&exports);

    let (obj_new, obj_get, obj_set, obj_delete, arr_new, elem_get, elem_set) = (
        emit_obj_new_v2(flavor),
        emit_obj_get_v2(),
        emit_obj_set_v2(),
        emit_obj_delete_v2(),
        emit_arr_new_v2(),
        emit_elem_get_v2(),
        emit_elem_set_v2(),
    );
    let helper_bodies = vec![
        obj_new,
        obj_get,
        obj_set,
        obj_delete,
        arr_new,
        elem_get,
        elem_set,
        emit_string_eq(flavor),
        emit_to_int32(flavor),
        emit_get_proto_from_ctor(flavor, num_host_imports + 1),
        emit_stub_unreachable(flavor),
        emit_stub_unreachable(flavor),
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

/// V2 handle 表固定在 memory64 前缀，entry 为 8 字节。
/// candidate 为即将占用的 handle 序号；越界则 trap（不再使用 main memory 4-byte 表布局）。
fn emit_handle_table_alloc_check(func: &mut Function, candidate_local: u32) {
    // 2^28 handles × 8B = 2GiB，远小于 HANDLE_REGION_BYTES(32GiB)。
    const MAX_HANDLES: i32 = 1 << 28;
    func.instruction(&WasmInstruction::LocalGet(candidate_local));
    func.instruction(&WasmInstruction::I32Const(MAX_HANDLES));
    func.instruction(&WasmInstruction::I32GeU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
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

/// obj_new (param $capacity i32) (result i32)
fn emit_obj_new_v2(_flavor: GcFlavor) -> Function {
    // local 0 = capacity:i32, 1 = size:i64, 2 = ptr:i64, 3 = handle:i32, 4 = end:i64。
    let mut func = Function::new(vec![
        (2, ValType::I64),
        (1, ValType::I32),
        (1, ValType::I64),
    ]);
    emit_gc_safepoint_poll_if_due_support(&mut func);

    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I64ExtendI32U);
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64Mul);
    func.instruction(&WasmInstruction::I64Const(16));
    func.instruction(&WasmInstruction::I64Add);
    func.instruction(&WasmInstruction::LocalSet(1));

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

    func.instruction(&WasmInstruction::GlobalGet(G_V2_HEAP_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Add);
    func.instruction(&WasmInstruction::GlobalGet(G_V2_HEAP_ALLOC_END));
    func.instruction(&WasmInstruction::I64LeU);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&WasmInstruction::GlobalGet(G_V2_HEAP_ALLOC_PTR));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I64Add);
    func.instruction(&WasmInstruction::LocalTee(4));
    func.instruction(&WasmInstruction::GlobalSet(G_V2_HEAP_ALLOC_PTR));
    func.instruction(&WasmInstruction::GlobalGet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_GC_ALLOC_BYTES));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Else);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_OBJECT as i32));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::Call(HOST_GC_ALLOC_SLOW));
    func.instruction(&WasmInstruction::LocalTee(2));
    func.instruction(&WasmInstruction::I64Const(-1));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Unreachable);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalSet(2));

    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::GlobalGet(G_OBJECT_PROTO_HANDLE));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_OBJECT as i32));
    func.instruction(&WasmInstruction::I32Store8(MemArg {
        offset: 4,
        align: 0,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 8,
        align: 2,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }));

    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I64ExtendI32U);
    func.instruction(&WasmInstruction::I64Const(3));
    func.instruction(&WasmInstruction::I64Shl);
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I64Const(16));
    func.instruction(&WasmInstruction::I64Shl);
    func.instruction(&WasmInstruction::I64Const(1));
    func.instruction(&WasmInstruction::I64Or);
    func.instruction(&WasmInstruction::I64AtomicStore(MemArg {
        offset: 0,
        align: 3,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }));
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::End);
    func
}

fn emit_obj_get_v2() -> Function {
    // dispatch（function/closure/bound/proxy/native callable/object）由 host 侧
    // `gc_obj_get` 统一完成；support 层只做透传。
    emit_v2_binary_host_helper(HOST_GC_OBJ_GET)
}

fn emit_obj_set_v2() -> Function {
    emit_v2_ternary_host_helper(HOST_GC_OBJ_SET)
}

fn emit_obj_delete_v2() -> Function {
    emit_v2_binary_host_helper(HOST_GC_OBJ_DELETE)
}

fn emit_arr_new_v2() -> Function {
    let mut func = Function::new(Vec::new());
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::Call(HOST_GC_ARR_NEW));
    func.instruction(&WasmInstruction::End);
    func
}

fn emit_elem_get_v2() -> Function {
    emit_v2_binary_host_helper(HOST_GC_ELEM_GET)
}

fn emit_elem_set_v2() -> Function {
    emit_v2_ternary_host_helper(HOST_GC_ELEM_SET)
}

fn emit_v2_binary_host_helper(host_index: u32) -> Function {
    let mut func = Function::new(Vec::new());
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(host_index));
    func.instruction(&WasmInstruction::End);
    func
}

fn emit_v2_ternary_host_helper(host_index: u32) -> Function {
    let mut func = Function::new(Vec::new());
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(host_index));
    func.instruction(&WasmInstruction::End);
    func
}

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

fn emit_get_proto_from_ctor(_flavor: GcFlavor, obj_get_func_index: u32) -> Function {
    // local 0 = $ctor (i64), local 1 = $proto (i64)
    let mut func = Function::new(vec![(1, ValType::I64)]);
    // 调用 obj_get(ctor, "prototype") — "prototype" 的 name_id 在 support module
    // 是硬编码的 primordial 偏移 236（PRIMORDIAL_PROTOTYPE_OFFSET）
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32Const(
        constants::PRIMORDIAL_PROTOTYPE_OFFSET as i32,
    ));
    func.instruction(&WasmInstruction::Call(obj_get_func_index));
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
        assert_eq!(HOST_IMPORTS.len(), 2);
    }
}
