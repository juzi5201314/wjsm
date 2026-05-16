use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction as WasmInstruction, MemArg, MemorySection, MemoryType, Module, RefType,
    TableSection, TableType, TypeSection, ValType,
};
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, Function as IrFunction,
    Instruction, Module as IrModule, Program, Terminator, UnaryOp, ValueId, constants, value,
};
// ── Shadow Stack Constants ─────────────────────────────────────────────
const SHADOW_STACK_SIZE: u32 = 65536; // 64KB = 8192 个 i64 槽位
const EVAL_VAR_MAP_RECORD_SIZE: u32 = 20;
const HOST_IMPORT_NAMES: [&str; 316] = [
    "console_log",
    "f64_mod",
    "f64_pow",
    "throw",
    "iterator_from",
    "iterator_next",
    "iterator_close",
    "iterator_value",
    "iterator_done",
    "enumerator_from",
    "enumerator_next",
    "enumerator_key",
    "enumerator_done",
    "typeof",
    "op_in",
    "op_instanceof",
    "string_concat",
    "string_concat_va",
    "define_property",
    "get_own_prop_desc",
    "abstract_eq",
    "abstract_compare",
    "gc_collect",
    "console_error",
    "console_warn",
    "console_info",
    "console_debug",
    "console_trace",
    "set_timeout",
    "clear_timeout",
    "set_interval",
    "clear_interval",
    "fetch",
    "json_stringify",
    "json_parse",
    "closure_create",
    "closure_get_func",
    "closure_get_env",
    "arr_push",
    "arr_pop",
    "arr_includes",
    "arr_index_of",
    "arr_join",
    "arr_concat",
    "arr_slice",
    "arr_fill",
    "arr_reverse",
    "arr_flat",
    "arr_init_length",
    "arr_get_length",
    "arr_proto_push",
    "arr_proto_pop",
    "arr_proto_includes",
    "arr_proto_index_of",
    "arr_proto_join",
    "arr_proto_concat",
    "arr_proto_slice",
    "arr_proto_fill",
    "arr_proto_reverse",
    "arr_proto_flat",
    "arr_proto_shift",
    "arr_proto_unshift",
    "arr_proto_sort",
    "arr_proto_at",
    "arr_proto_copy_within",
    "arr_proto_for_each",
    "arr_proto_map",
    "arr_proto_filter",
    "arr_proto_reduce",
    "arr_proto_reduce_right",
    "arr_proto_find",
    "arr_proto_find_index",
    "arr_proto_some",
    "arr_proto_every",
    "arr_proto_flat_map",
    "arr_proto_splice",
    "arr_proto_is_array",
    "abort_shadow_stack_overflow",
    "func_call",
    "func_apply",
    "func_bind",
    "object_rest",
    "obj_spread",
    "has_own_property",
    "obj_keys",
    "obj_values",
    "obj_entries",
    "obj_assign",
    "obj_create",
    "obj_get_proto_of",
    "obj_set_proto_of",
    "obj_get_own_prop_names",
    "obj_is",
    "obj_proto_to_string",
    "obj_proto_value_of",
    "bigint_from_literal",
    "bigint_add",
    "bigint_sub",
    "bigint_mul",
    "bigint_div",
    "bigint_mod",
    "bigint_pow",
    "bigint_neg",
    "bigint_eq",
    "bigint_cmp",
    "symbol_create",
    "symbol_for",
    "symbol_key_for",
    "symbol_well_known",
    "regex_create",
    "regex_test",
    "regex_exec",
    "string_match",
    "string_replace",
    "string_search",
    "string_split",
    "promise_create",
    "promise_instance_resolve",
    "promise_instance_reject",
    "promise_then",
    "promise_catch",
    "promise_finally",
    "promise_all",
    "promise_race",
    "promise_all_settled",
    "promise_any",
    "promise_resolve_static",
    "promise_reject_static",
    "is_promise",
    "queue_microtask",
    "drain_microtasks",
    "async_function_start",
    "async_function_resume",
    "async_function_suspend",
    "continuation_create",
    "continuation_save_var",
    "continuation_load_var",
    "async_generator_start",
    "async_generator_next",
    "async_generator_return",
    "async_generator_throw",
    "native_call",
    "promise_create_resolve_function",
    "promise_create_reject_function",
    "is_callable",
    "promise_with_resolvers",
    "register_module_namespace",
    "dynamic_import",
    "eval_direct",
    "eval_indirect",
    "jsx_create_element",
    "proxy_create",
    "proxy_revocable",
    "reflect_get",
    "reflect_set",
    "reflect_has",
    "reflect_delete_property",
    "reflect_apply",
    "reflect_construct",
    "reflect_get_prototype_of",
    "reflect_set_prototype_of",
    "reflect_is_extensible",
    "reflect_prevent_extensions",
    "reflect_get_own_property_descriptor",
    "reflect_define_property",
    "reflect_own_keys",
    "string_at",
    "string_char_at",
    "string_char_code_at",
    "string_code_point_at",
    "string_concat_proto",
    "string_ends_with",
    "string_includes",
    "string_index_of",
    "string_last_index_of",
    "string_match_all",
    "string_pad_end",
    "string_pad_start",
    "string_repeat",
    "string_replace_all",
    "string_slice",
    "string_starts_with",
    "string_substring",
    "string_to_lower_case",
    "string_to_upper_case",
    "string_trim",
    "string_trim_end",
    "string_trim_start",
    "string_to_string",
    "string_value_of",
    "string_iterator",
    "string_from_char_code",
    "string_from_code_point",
    "math_abs",
    "math_acos",
    "math_acosh",
    "math_asin",
    "math_asinh",
    "math_atan",
    "math_atanh",
    "math_atan2",
    "math_cbrt",
    "math_ceil",
    "math_clz32",
    "math_cos",
    "math_cosh",
    "math_exp",
    "math_expm1",
    "math_floor",
    "math_fround",
    "math_hypot",
    "math_imul",
    "math_log",
    "math_log1p",
    "math_log10",
    "math_log2",
    "math_max",
    "math_min",
    "math_pow",
    "math_random",
    "math_round",
    "math_sign",
    "math_sin",
    "math_sinh",
    "math_sqrt",
    "math_tan",
    "math_tanh",
    "math_trunc",
    "number_constructor",
    "number_is_nan",
    "number_is_finite",
    "number_is_integer",
    "number_is_safe_integer",
    "number_parse_int",
    "number_parse_float",
    "number_proto_to_string",
    "number_proto_value_of",
    "number_proto_to_fixed",
    "number_proto_to_exponential",
    "number_proto_to_precision",
    "boolean_constructor",
    "boolean_proto_to_string",
    "boolean_proto_value_of",
    "error_constructor",
    "type_error_constructor",
    "range_error_constructor",
    "syntax_error_constructor",
    "reference_error_constructor",
    "uri_error_constructor",
    "eval_error_constructor",
    "error_proto_to_string",
    // ── Map imports ──
    "map_constructor",
    "map_proto_set",
    "map_proto_get",
    // ── Set imports ──
    "set_constructor",
    "set_proto_add",
    // ── Map/Set shared imports ──
    "map_set_has",
    "map_set_delete",
    "map_set_clear",
    "map_set_get_size",
    "map_set_for_each",
    "map_set_keys",
    "map_set_values",
    "map_set_entries",
    // ── Date imports ──
    "date_constructor",
    "date_now",
    "date_parse",
    "date_utc",
    // ── WeakMap imports ──
    "weakmap_constructor",
    "weakmap_proto_set",
    "weakmap_proto_get",
    "weakmap_proto_has",
    "weakmap_proto_delete",
    // ── WeakSet imports ──
    "weakset_constructor",
    "weakset_proto_add",
    "weakset_proto_has",
    "weakset_proto_delete",
    // ── ArrayBuffer imports ──
    "arraybuffer_constructor",
    "arraybuffer_proto_byte_length",
    "arraybuffer_proto_slice",
    // ── DataView imports ──
    "dataview_constructor",
    "dataview_proto_get_float64",
    "dataview_proto_get_float32",
    "dataview_proto_get_int32",
    "dataview_proto_get_uint32",
    "dataview_proto_get_int16",
    "dataview_proto_get_uint16",
    "dataview_proto_get_int8",
    "dataview_proto_get_uint8",
    "dataview_proto_set_float64",
    "dataview_proto_set_float32",
    "dataview_proto_set_int32",
    "dataview_proto_set_uint32",
    "dataview_proto_set_int16",
    "dataview_proto_set_uint16",
    "dataview_proto_set_int8",
    "dataview_proto_set_uint8",
    // ── TypedArray constructor imports ──
    "int8array_constructor",
    "uint8array_constructor",
    "uint8clampedarray_constructor",
    "int16array_constructor",
    "uint16array_constructor",
    "int32array_constructor",
    "uint32array_constructor",
    "float32array_constructor",
    "float64array_constructor",
    // ── TypedArray prototype imports ──
    "typedarray_proto_length",
    "typedarray_proto_byte_length",
    "typedarray_proto_byte_offset",
    "typedarray_proto_set",
    "typedarray_proto_slice",
    "typedarray_proto_subarray",
    "get_builtin_global",
    "private_get",
    "private_set",
    "private_has",
];
// SHADOW_STACK_ALIGN: reserved for future use

// ── Public API ──────────────────────────────────────────────────────────

pub fn compile(program: &Program) -> Result<Vec<u8>> {
    debug_assert_eq!(
        HOST_IMPORT_NAMES.len(),
        316,
        "HOST_IMPORT_NAMES length must match expected import count"
    );
    let mut compiler = Compiler::new(CompileMode::Normal);
    compiler.compile_module(program)?;
    Ok(compiler.finish())
}

pub fn compile_eval(program: &Program) -> Result<Vec<u8>> {
    compile_eval_at_data_base(program, 0)
}

pub fn compile_eval_at_data_base(program: &Program, data_base: u32) -> Result<Vec<u8>> {
    let mut compiler = Compiler::new_with_data_base(CompileMode::Eval, data_base);
    compiler.compile_module(program)?;
    Ok(compiler.finish())
}

// ── Compiler ────────────────────────────────────────────────────────────

struct Compiler {
    module: Module,
    types: TypeSection,
    imports: ImportSection,
    functions: FunctionSection,
    exports: ExportSection,
    codes: CodeSection,
    memory: MemorySection,
    data: DataSection,
    table: TableSection,
    elements: ElementSection,
    globals: GlobalSection,
    current_func: Option<Function>,
    string_data: Vec<u8>,
    data_base: u32,
    data_offset: u32,
    /// Map variable name → WASM local index (for LoadVar / StoreVar).
    var_locals: HashMap<String, u32>,
    /// Map variable name → current eval frame byte offset.
    var_memory_offsets: HashMap<String, u32>,
    /// Next available WASM local index (after SSA temporaries).
    next_var_local: u32,
    /// Phi locals: mapping from Phi dest ValueId → WASM local index.
    phi_locals: HashMap<u32, u32>,
    /// Set of block indices already compiled (for dedup in structured compilation).
    compiled_blocks: std::collections::HashSet<usize>,
    /// Next available WASM function index (starts after imports).
    _next_import_func: u32,
    /// Map builtin → WASM function index.
    builtin_func_indices: HashMap<Builtin, u32>,
    /// 活跃循环栈，用于跟踪嵌套循环的 WASM 标签深度。
    loop_stack: Vec<LoopInfo>,
    /// if/else 嵌套深度，用于计算 br 指令的标签深度偏移。
    if_depth: u32,
    /// Function table: table index → WASM func index.
    function_table: Vec<u32>,
    /// Reverse lookup: WASM func index → table position.
    function_table_reverse: HashMap<u32, u32>,
    /// IR function name → WASM func index.
    function_name_to_wasm_idx: HashMap<String, u32>,
    /// WASM index of $obj_new helper.
    obj_new_func_idx: u32,
    /// WASM index of $obj_get helper.
    obj_get_func_idx: u32,
    /// WASM index of $obj_set helper.
    obj_set_func_idx: u32,
    /// WASM index of $obj_delete helper.
    obj_delete_func_idx: u32,
    /// WASM index of $arr_new helper.
    arr_new_func_idx: u32,
    /// WASM index of $elem_get helper.
    elem_get_func_idx: u32,
    /// WASM index of $elem_set helper.
    elem_set_func_idx: u32,
    /// WASM index of $to_int32 helper.
    to_int32_func_idx: u32,
    /// WASM global index for heap pointer.
    heap_ptr_global_idx: u32,
    /// WASM global index for function properties array pointer.
    func_props_global_idx: u32,
    /// WASM global index for object handle table base address.
    obj_table_global_idx: u32,
    /// WASM global index for next available handle table entry count.
    obj_table_count_global_idx: u32,
    /// Number of IR functions (for pre-allocation of function property objects).
    num_ir_functions: u32,
    /// Whether the current function returns a value (Type 6 JS functions = true).
    current_func_returns_value: bool,
    /// Base offset for SSA value WASM local indices (0 for main, 8 for Type 6 JS functions).
    ssa_local_base: u32,
    /// String ptr cache: maps string content → data segment offset.
    string_ptr_cache: HashMap<String, u32>,
    /// WASM local index for string_concat scratch variable.
    string_concat_scratch_idx: u32,
    /// WASM global index for shadow stack pointer.
    shadow_sp_global_idx: u32,
    /// WASM local index for shadow_sp scratch variable (i32, used during Call).
    shadow_sp_scratch_idx: u32,
    /// WASM local index for the base address of eval-visible variable storage.
    eval_var_base_local_idx: u32,
    /// WASM function index for gc_collect host function.
    gc_collect_func_idx: u32,
    /// WASM global index for alloc_counter (GC heuristic).
    alloc_counter_global_idx: u32,
    /// WASM global index for __object_heap_start (runtime GC heap base).
    object_heap_start_global_idx: u32,
    /// WASM global index for __num_ir_functions (runtime GC root set).
    num_ir_functions_global_idx: u32,
    /// WASM global index for __shadow_stack_end (shadow stack bounds check).
    shadow_stack_end_global_idx: u32,
    /// WASM function index for closure_create import.
    closure_create_func_idx: u32,
    /// WASM function index for closure_get_func import.
    closure_get_func_idx: u32,
    /// WASM function index for closure_get_env import.
    closure_get_env_idx: u32,
    /// WASM function index for native_call import.
    native_call_func_idx: u32,
    /// WASM global index for array prototype handle.
    array_proto_handle_global_idx: u32,
    /// Base table index for array prototype methods (Table[N+8])
    arr_proto_table_base: u32,
    /// WASM function index for $obj_spread helper.
    obj_spread_func_idx: u32,
    /// WASM function index for $get_prototype_from_constructor helper.
    get_proto_from_ctor_func_idx: u32,
    /// WASM function index for nul-terminated string equality helper.
    string_eq_func_idx: u32,
    /// WASM global index for Object.prototype handle.
    object_proto_handle_global_idx: u32,
    /// WASM global index for __eval_var_map_ptr.
    eval_var_map_ptr_global_idx: u32,
    /// WASM global index for __eval_var_map_count.
    eval_var_map_count_global_idx: u32,
    /// Encoded eval variable map metadata emitted into the data section.
    eval_var_map_records: Vec<EvalVarMapRecord>,
    eval_var_map_ptr: u32,
    eval_var_map_count: u32,
    /// WASM local index for continuation handle (used in async state machine functions).
    continuation_local_idx: u32,
    current_function_has_eval: bool,
    mode: CompileMode,
    function_param_counts: Vec<u32>,
    function_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvalVarMapRecord {
    function_name: String,
    var_name: String,
    offset: u32,
}
/// 循环元信息（编译前预扫描得到）。
#[derive(Debug, Clone)]
struct LoopInfo {
    /// 循环头 block 索引（back-edge 目标）。
    header_idx: usize,
    /// 循环出口 block 索引（break 目标）。
    exit_idx: usize,
}

#[derive(Debug, Clone)]
struct Cfg {
    successors: Vec<Vec<usize>>,
    predecessors: Vec<Vec<usize>>,
}

impl Cfg {
    fn from_function(function: &IrFunction) -> Self {
        let len = function.blocks().len();
        let mut successors = vec![Vec::new(); len];
        let mut predecessors = vec![Vec::new(); len];

        for (idx, block) in function.blocks().iter().enumerate() {
            let mut add_edge = |target: BasicBlockId| {
                let target_idx = target.0 as usize;
                if target_idx < len {
                    successors[idx].push(target_idx);
                    predecessors[target_idx].push(idx);
                }
            };

            match block.terminator() {
                Terminator::Jump { target } => add_edge(*target),
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    add_edge(*true_block);
                    add_edge(*false_block);
                }
                Terminator::Switch {
                    cases,
                    default_block,
                    exit_block,
                    ..
                } => {
                    for case in cases {
                        add_edge(case.target);
                    }
                    add_edge(*default_block);
                    add_edge(*exit_block);
                }
                Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {}
            }
        }

        Self {
            successors,
            predecessors,
        }
    }
}

#[derive(Debug, Clone)]
enum Region {
    Linear { start_idx: usize },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SwitchCaseRegion {
    _case_idx: usize,
    _target_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompileMode {
    Normal,
    Eval,
}

fn import_eval_global(
    imports: &mut ImportSection,
    name: &'static str,
    val_type: ValType,
    mutable: bool,
) {
    imports.import(
        "env",
        name,
        EntityType::Global(GlobalType {
            val_type,
            mutable,
            shared: false,
        }),
    );
}

#[derive(Debug, Clone)]
struct RegionTree {
    root: Region,
}

#[derive(Debug, Clone)]
enum RegionTreeError {
    MissingEntry,
}

impl RegionTree {
    fn build(function: &IrFunction, cfg: &Cfg) -> Result<Self, RegionTreeError> {
        let _ = (cfg.successors.len(), cfg.predecessors.len());
        let start_idx = function.entry().0 as usize;
        if start_idx >= function.blocks().len() {
            return Err(RegionTreeError::MissingEntry);
        }
        Ok(Self {
            root: Region::Linear { start_idx },
        })
    }
}

mod compiler_array_helpers;
mod compiler_builtins;
mod compiler_control;
mod compiler_core;
mod compiler_data;
mod compiler_helpers;
mod compiler_instructions;
mod compiler_module;

// ── Value ID collection ─────────────────────────────────────────────────
fn block_has_suspend(block: &BasicBlock) -> bool {
    block
        .instructions()
        .iter()
        .any(|instruction| matches!(instruction, Instruction::Suspend { .. }))
}

/// 检测 CFG 中的循环（通过 back-edge 识别）。
/// 返回按 header_idx 排序的 LoopInfo 列表。
fn detect_loops(blocks: &[BasicBlock]) -> Vec<LoopInfo> {
    use std::collections::{HashMap, HashSet};
    let mut back_edges: HashMap<usize, Vec<usize>> = HashMap::new();

    for (i, block) in blocks.iter().enumerate() {
        match block.terminator() {
            Terminator::Jump { target } => {
                let t = target.0 as usize;
                if t <= i {
                    back_edges.entry(t).or_default().push(i);
                }
            }
            Terminator::Branch { true_block, .. } => {
                // do-while 模式：true → header（通过 Branch 实现的 back-edge）
                let t = true_block.0 as usize;
                if t <= i {
                    back_edges.entry(t).or_default().push(i);
                }
            }
            _ => {}
        }
    }

    let mut loops: Vec<LoopInfo> = Vec::new();
    // NOTE: 此处对每个 back-edge 做前向可达性分析以过滤无效循环。
    // 在大型 CFG 上可能有性能影响，未来可考虑使用 dominator tree 分析替代。
    'next_edge: for (header_idx, latches) in &back_edges {
        let mut reachable: HashSet<usize> = HashSet::new();
        let mut stack = vec![*header_idx];
        while let Some(idx) = stack.pop() {
            if reachable.contains(&idx) {
                continue;
            }
            reachable.insert(idx);
            if idx >= blocks.len() {
                continue;
            }
            match blocks[idx].terminator() {
                Terminator::Jump { target } => {
                    stack.push(target.0 as usize);
                }
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    stack.push(true_block.0 as usize);
                    stack.push(false_block.0 as usize);
                }
                _ => {}
            }
        }
        let mut any_latch_reachable = false;
        for latch in latches {
            if reachable.contains(latch) {
                any_latch_reachable = true;
                break;
            }
        }
        if !any_latch_reachable {
            continue 'next_edge;
        }

        let exit_idx = match blocks[*header_idx].terminator() {
            // while/for 模式：header 有 Branch，false 分支是出口
            Terminator::Branch { false_block, .. } => false_block.0 as usize,
            _ => {
                // do-while 模式：header 没有 Branch，找到指向 header 的 Branch
                let mut exit = *header_idx + 1;
                for block in blocks.iter() {
                    if let Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } = block.terminator()
                    {
                        if true_block.0 as usize == *header_idx {
                            exit = false_block.0 as usize;
                            break;
                        }
                    }
                }
                exit
            }
        };
        loops.push(LoopInfo {
            header_idx: *header_idx,
            exit_idx,
        });
    }

    loops.sort_by_key(|l| l.header_idx);
    loops
}

fn is_eval_memory_var_name(name: &str) -> bool {
    !matches!(name, "$env" | "$this" | "$eval_env")
        && !name.ends_with(".$env")
        && !name.ends_with(".$this")
}

fn max_instruction_value_id(instruction: &Instruction) -> u32 {
    match instruction {
        Instruction::Const { dest, .. } => dest.0,
        Instruction::Binary { dest, lhs, rhs, .. } => dest.0.max(lhs.0).max(rhs.0),
        Instruction::Unary { dest, value, .. } => dest.0.max(value.0),
        Instruction::Compare { dest, lhs, rhs, .. } => dest.0.max(lhs.0).max(rhs.0),
        Instruction::Phi { dest, sources } => sources
            .iter()
            .map(|s| s.value.0)
            .max()
            .unwrap_or(0)
            .max(dest.0),
        Instruction::CallBuiltin { dest, args, .. } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            dest.map_or(args_max, |d| d.0.max(args_max))
        }
        Instruction::LoadVar { dest, .. } => dest.0,
        Instruction::StoreVar { value, .. } => value.0,
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
        } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            let max_val = callee.0.max(this_val.0).max(args_max);
            dest.map_or(max_val, |d| d.0.max(max_val))
        }
        Instruction::NewObject { dest, capacity: _ } => dest.0,
        Instruction::GetProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::SetProp { object, key, value } => object.0.max(key.0).max(value.0),
        Instruction::DeleteProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::SetProto { object, value } => object.0.max(value.0),
        Instruction::NewArray { dest, capacity: _ } => dest.0,
        Instruction::GetElem {
            dest,
            object,
            index,
        } => dest.0.max(object.0).max(index.0),
        Instruction::SetElem {
            object,
            index,
            value,
        } => object.0.max(index.0).max(value.0),
        Instruction::StringConcatVa { dest, parts } => {
            parts.iter().map(|v| v.0).max().unwrap_or(0).max(dest.0)
        }
        Instruction::OptionalGetProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::OptionalGetElem { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            let max_val = callee.0.max(this_val.0).max(args_max);
            dest.0.max(max_val)
        }
        Instruction::ObjectSpread { dest, source } => dest.0.max(source.0),
        Instruction::GetSuperBase { dest } => dest.0,
        Instruction::NewPromise { dest } => dest.0,
        Instruction::PromiseResolve { promise, value } => promise.0.max(value.0),
        Instruction::PromiseReject { promise, reason } => promise.0.max(reason.0),
        Instruction::Suspend { promise, .. } => promise.0,
        Instruction::CollectRestArgs { dest, .. } => dest.0,
    }
}

pub fn builtin_arity(builtin: &Builtin) -> (&'static str, usize) {
    match builtin {
        Builtin::ConsoleLog => ("console.log", 1),
        Builtin::ConsoleError => ("console.error", 1),
        Builtin::ConsoleWarn => ("console.warn", 1),
        Builtin::ConsoleInfo => ("console.info", 1),
        Builtin::ConsoleDebug => ("console.debug", 1),
        Builtin::ConsoleTrace => ("console.trace", 1),
        Builtin::Debugger => ("debugger", 0),
        Builtin::Throw => ("throw", 1),
        Builtin::AbortShadowStackOverflow => ("abort_shadow_stack_overflow", 3),
        Builtin::F64Mod => ("f64.mod", 2),
        Builtin::F64Exp => ("f64.exp", 2),
        Builtin::IteratorFrom => ("iterator.from", 1),
        Builtin::IteratorNext => ("iterator.next", 1),
        Builtin::IteratorClose => ("iterator.close", 1),
        Builtin::IteratorValue => ("iterator.value", 1),
        Builtin::IteratorDone => ("iterator.done", 1),
        Builtin::EnumeratorFrom => ("enumerator.from", 1),
        Builtin::EnumeratorNext => ("enumerator.next", 1),
        Builtin::EnumeratorKey => ("enumerator.key", 1),
        Builtin::EnumeratorDone => ("enumerator.done", 1),
        Builtin::TypeOf => ("typeof", 1),
        Builtin::In => ("op_in", 2),
        Builtin::InstanceOf => ("op_instanceof", 2),
        Builtin::AbstractEq => ("abstract_eq", 2),
        Builtin::AbstractCompare => ("abstract_compare", 2),
        Builtin::DefineProperty => ("define_property", 3),
        Builtin::GetOwnPropDesc => ("get_own_prop_desc", 2),
        Builtin::SetTimeout => ("setTimeout", 2),
        Builtin::ClearTimeout => ("clearTimeout", 1),
        Builtin::SetInterval => ("setInterval", 2),
        Builtin::ClearInterval => ("clearInterval", 1),
        Builtin::Fetch => ("fetch", 1),
        Builtin::Eval => ("eval", 2),
        Builtin::EvalIndirect => ("eval.indirect", 1),
        Builtin::EvalResult => ("eval.result", 1),
        Builtin::JsonStringify => ("JSON.stringify", 1),
        Builtin::JsonParse => ("JSON.parse", 1),
        Builtin::CreateClosure => ("create_closure", 2),
        Builtin::ArrayPush => ("array.push", 2),
        Builtin::ArrayPop => ("array.pop", 1),
        Builtin::ArrayIncludes => ("array.includes", 2),
        Builtin::ArrayIndexOf => ("array.index_of", 3),
        Builtin::ArrayJoin => ("array.join", 2),
        Builtin::ArrayConcat => ("array.concat", 2),
        Builtin::ArraySlice => ("array.slice", 3),
        Builtin::ArrayFill => ("array.fill", 4),
        Builtin::ArrayReverse => ("array.reverse", 1),
        Builtin::ArrayFlat => ("array.flat", 2),
        Builtin::ArrayInitLength => ("array.init_length", 2),
        Builtin::ArrayGetLength => ("array.get_length", 1),
        Builtin::ArrayShift => ("array.shift", 1),
        Builtin::ArrayUnshiftVa => ("array.unshift", 1),
        Builtin::ArraySort => ("array.sort", 1),
        Builtin::ArrayAt => ("array.at", 2),
        Builtin::ArrayCopyWithin => ("array.copy_within", 1),
        Builtin::ArrayForEach => ("array.for_each", 1),
        Builtin::ArrayMap => ("array.map", 1),
        Builtin::ArrayFilter => ("array.filter", 1),
        Builtin::ArrayReduce => ("array.reduce", 1),
        Builtin::ArrayReduceRight => ("array.reduce_right", 1),
        Builtin::ArrayFind => ("array.find", 1),
        Builtin::ArrayFindIndex => ("array.find_index", 1),
        Builtin::ArraySome => ("array.some", 1),
        Builtin::ArrayEvery => ("array.every", 1),
        Builtin::ArrayFlatMap => ("array.flat_map", 1),
        Builtin::ArraySpliceVa => ("array.splice_va", 1),
        Builtin::ArrayIsArray => ("array.is_array", 1),
        Builtin::ArrayConcatVa => ("array.concat_va", 1),
        Builtin::FuncCall => ("func_call", 1),
        Builtin::FuncApply => ("func_apply", 3),
        Builtin::FuncBind => ("func_bind", 1),
        Builtin::ObjectRest => ("object_rest", 2),
        Builtin::GetPrototypeFromConstructor => ("get_prototype_from_constructor", 1),
        Builtin::HasOwnProperty => ("has_own_property", 2),
        Builtin::PrivateGet => ("private_get", 2),
        Builtin::PrivateSet => ("private_set", 3),
        Builtin::PrivateHas => ("private_has", 2),
        Builtin::ObjectProtoToString => ("object_proto_to_string", 1),
        Builtin::ObjectProtoValueOf => ("object_proto_value_of", 1),
        Builtin::ObjectKeys => ("object.keys", 1),
        Builtin::ObjectValues => ("object.values", 1),
        Builtin::ObjectEntries => ("object.entries", 1),
        Builtin::ObjectAssign => ("object.assign", 1),
        Builtin::ObjectCreate => ("object.create", 2),
        Builtin::ObjectGetPrototypeOf => ("object.get_prototype_of", 1),
        Builtin::ObjectSetPrototypeOf => ("object.set_prototype_of", 2),
        Builtin::ObjectGetOwnPropertyNames => ("object.get_own_property_names", 1),
        Builtin::ObjectIs => ("object.is", 2),
        Builtin::BigIntFromLiteral => ("bigint.from_literal", 2),
        Builtin::BigIntAdd => ("bigint.add", 2),
        Builtin::BigIntSub => ("bigint.sub", 2),
        Builtin::BigIntMul => ("bigint.mul", 2),
        Builtin::BigIntDiv => ("bigint.div", 2),
        Builtin::BigIntMod => ("bigint.mod", 2),
        Builtin::BigIntPow => ("bigint.pow", 2),
        Builtin::BigIntNeg => ("bigint.neg", 1),
        Builtin::BigIntEq => ("bigint.eq", 2),
        Builtin::BigIntCmp => ("bigint.cmp", 2),
        Builtin::SymbolCreate => ("symbol.create", 1),
        Builtin::SymbolFor => ("symbol.for", 1),
        Builtin::SymbolKeyFor => ("symbol.key_for", 1),
        Builtin::SymbolWellKnown => ("symbol.well_known", 1),
        Builtin::RegExpCreate => ("regexp.create", 4),
        Builtin::RegExpTest => ("regexp.test", 2),
        Builtin::RegExpExec => ("regexp.exec", 2),
        Builtin::StringMatch => ("string.match", 2),
        Builtin::StringReplace => ("string.replace", 3),
        Builtin::StringSearch => ("string.search", 2),
        Builtin::StringSplit => ("string.split", 3),
        Builtin::PromiseCreate => ("promise.create", 0),
        Builtin::PromiseInstanceResolve => ("promise.instance_resolve", 2),
        Builtin::PromiseInstanceReject => ("promise.instance_reject", 2),
        Builtin::PromiseCreateResolveFunction => ("promise.create_resolve_function", 1),
        Builtin::PromiseCreateRejectFunction => ("promise.create_reject_function", 1),
        Builtin::PromiseThen => ("promise.then", 3),
        Builtin::PromiseCatch => ("promise.catch", 2),
        Builtin::PromiseFinally => ("promise.finally", 2),
        Builtin::PromiseAll => ("promise.all", 2),
        Builtin::PromiseRace => ("promise.race", 2),
        Builtin::PromiseAllSettled => ("promise.all_settled", 2),
        Builtin::PromiseAny => ("promise.any", 2),
        Builtin::PromiseResolveStatic => ("promise.resolve_static", 2),
        Builtin::PromiseRejectStatic => ("promise.reject_static", 2),
        Builtin::IsPromise => ("is_promise", 1),
        Builtin::QueueMicrotask => ("queue_microtask", 1),
        Builtin::DrainMicrotasks => ("drain_microtasks", 0),
        Builtin::AsyncFunctionStart => ("async_function.start", 1),
        Builtin::AsyncFunctionResume => ("async_function.resume", 5),
        Builtin::AsyncFunctionSuspend => ("async_function.suspend", 3),
        Builtin::ContinuationCreate => ("continuation.create", 3),
        Builtin::ContinuationSaveVar => ("continuation.save_var", 3),
        Builtin::ContinuationLoadVar => ("continuation.load_var", 2),
        Builtin::AsyncGeneratorStart => ("async_generator.start", 1),
        Builtin::AsyncGeneratorNext => ("async_generator.next", 2),
        Builtin::AsyncGeneratorReturn => ("async_generator.return", 2),
        Builtin::PromiseWithResolvers => ("promise.with_resolvers", 1),
        Builtin::IsCallable => ("is_callable", 1),
        Builtin::AsyncGeneratorThrow => ("async_generator.throw", 2),
        // ── 动态 import builtins ──
        Builtin::DynamicImport => ("dynamic_import", 1),
        Builtin::RegisterModuleNamespace => ("register_module_namespace", 2),
        Builtin::JsxCreateElement => ("jsx.create_element", 3),
        Builtin::ProxyCreate => ("proxy.create", 2),
        Builtin::ProxyRevocable => ("proxy.revocable", 2),
        Builtin::ReflectGet => ("reflect.get", 3),
        Builtin::ReflectSet => ("reflect.set", 4),
        Builtin::ReflectHas => ("reflect.has", 2),
        Builtin::ReflectDeleteProperty => ("reflect.delete_property", 2),
        Builtin::ReflectApply => ("reflect.apply", 3),
        Builtin::ReflectConstruct => ("reflect.construct", 3),
        Builtin::ReflectGetPrototypeOf => ("reflect.get_prototype_of", 1),
        Builtin::ReflectSetPrototypeOf => ("reflect.set_prototype_of", 2),
        Builtin::ReflectIsExtensible => ("reflect.is_extensible", 1),
        Builtin::ReflectPreventExtensions => ("reflect.prevent_extensions", 1),
        Builtin::ReflectGetOwnPropertyDescriptor => ("reflect.get_own_property_descriptor", 2),
        Builtin::ReflectDefineProperty => ("reflect.define_property", 3),
        Builtin::ReflectOwnKeys => ("reflect.own_keys", 1),
        Builtin::StringAt => ("string.at", 2),
        Builtin::StringCharAt => ("string.char_at", 2),
        Builtin::StringCharCodeAt => ("string.char_code_at", 2),
        Builtin::StringCodePointAt => ("string.code_point_at", 2),
        Builtin::StringConcatVa => ("string.concat", 1),
        Builtin::StringEndsWith => ("string.ends_with", 2),
        Builtin::StringIncludes => ("string.includes", 2),
        Builtin::StringIndexOf => ("string.index_of", 2),
        Builtin::StringLastIndexOf => ("string.last_index_of", 2),
        Builtin::StringMatchAll => ("string.match_all", 1),
        Builtin::StringPadEnd => ("string.pad_end", 2),
        Builtin::StringPadStart => ("string.pad_start", 2),
        Builtin::StringRepeat => ("string.repeat", 2),
        Builtin::StringReplaceAll => ("string.replace_all", 3),
        Builtin::StringSlice => ("string.slice", 2),
        Builtin::StringStartsWith => ("string.starts_with", 2),
        Builtin::StringSubstring => ("string.substring", 2),
        Builtin::StringToLowerCase => ("string.to_lower_case", 1),
        Builtin::StringToUpperCase => ("string.to_upper_case", 1),
        Builtin::StringTrim => ("string.trim", 1),
        Builtin::StringTrimEnd => ("string.trim_end", 1),
        Builtin::StringTrimStart => ("string.trim_start", 1),
        Builtin::StringToString => ("string.to_string", 1),
        Builtin::StringValueOf => ("string.value_of", 1),
        Builtin::StringIterator => ("string.iterator", 1),
        Builtin::StringFromCharCode => ("string.from_char_code", 1),
        Builtin::StringFromCodePoint => ("string.from_code_point", 1),
        // ── Math builtins ──
        Builtin::MathAbs => ("Math.abs", 1),
        Builtin::MathAcos => ("Math.acos", 1),
        Builtin::MathAcosh => ("Math.acosh", 1),
        Builtin::MathAsin => ("Math.asin", 1),
        Builtin::MathAsinh => ("Math.asinh", 1),
        Builtin::MathAtan => ("Math.atan", 1),
        Builtin::MathAtanh => ("Math.atanh", 1),
        Builtin::MathAtan2 => ("Math.atan2", 2),
        Builtin::MathCbrt => ("Math.cbrt", 1),
        Builtin::MathCeil => ("Math.ceil", 1),
        Builtin::MathClz32 => ("Math.clz32", 1),
        Builtin::MathCos => ("Math.cos", 1),
        Builtin::MathCosh => ("Math.cosh", 1),
        Builtin::MathExp => ("Math.exp", 1),
        Builtin::MathExpm1 => ("Math.expm1", 1),
        Builtin::MathFloor => ("Math.floor", 1),
        Builtin::MathFround => ("Math.fround", 1),
        Builtin::MathHypot => ("Math.hypot", 0),
        Builtin::MathImul => ("Math.imul", 2),
        Builtin::MathLog => ("Math.log", 1),
        Builtin::MathLog1p => ("Math.log1p", 1),
        Builtin::MathLog10 => ("Math.log10", 1),
        Builtin::MathLog2 => ("Math.log2", 1),
        Builtin::MathMax => ("Math.max", 1),
        Builtin::MathMin => ("Math.min", 1),
        Builtin::MathPow => ("Math.pow", 2),
        Builtin::MathRandom => ("Math.random", 0),
        Builtin::MathRound => ("Math.round", 1),
        Builtin::MathSign => ("Math.sign", 1),
        Builtin::MathSin => ("Math.sin", 1),
        Builtin::MathSinh => ("Math.sinh", 1),
        Builtin::MathSqrt => ("Math.sqrt", 1),
        Builtin::MathTan => ("Math.tan", 1),
        Builtin::MathTanh => ("Math.tanh", 1),
        Builtin::MathTrunc => ("Math.trunc", 1),
        // ── Number builtins ──
        Builtin::NumberConstructor => ("Number", 1),
        Builtin::NumberIsNaN => ("Number.isNaN", 1),
        Builtin::NumberIsFinite => ("Number.isFinite", 1),
        Builtin::NumberIsInteger => ("Number.isInteger", 1),
        Builtin::NumberIsSafeInteger => ("Number.isSafeInteger", 1),
        Builtin::NumberParseInt => ("Number.parseInt", 2),
        Builtin::NumberParseFloat => ("Number.parseFloat", 1),
        Builtin::NumberProtoToString => ("Number.prototype.toString", 1),
        Builtin::NumberProtoValueOf => ("Number.prototype.valueOf", 1),
        Builtin::NumberProtoToFixed => ("Number.prototype.toFixed", 1),
        Builtin::NumberProtoToExponential => ("Number.prototype.toExponential", 1),
        Builtin::NumberProtoToPrecision => ("Number.prototype.toPrecision", 1),
        // ── Boolean builtins ──
        Builtin::BooleanConstructor => ("Boolean", 1),
        Builtin::BooleanProtoToString => ("Boolean.prototype.toString", 1),
        Builtin::BooleanProtoValueOf => ("Boolean.prototype.valueOf", 1),
        // ── Error builtins ──
        Builtin::ErrorConstructor => ("Error", 1),
        Builtin::TypeErrorConstructor => ("TypeError", 1),
        Builtin::RangeErrorConstructor => ("RangeError", 1),
        Builtin::SyntaxErrorConstructor => ("SyntaxError", 1),
        Builtin::ReferenceErrorConstructor => ("ReferenceError", 1),
        Builtin::URIErrorConstructor => ("URIError", 1),
        Builtin::EvalErrorConstructor => ("EvalError", 1),
        Builtin::ErrorProtoToString => ("Error.prototype.toString", 1),
        // ── Map builtins ──
        Builtin::MapConstructor => ("Map", 1),
        Builtin::MapProtoSet => ("Map.prototype.set", 3),
        Builtin::MapProtoGet => ("Map.prototype.get", 2),
        // ── Set builtins ──
        Builtin::SetConstructor => ("Set", 1),
        Builtin::SetProtoAdd => ("Set.prototype.add", 2),
        // ── Map/Set shared builtins ──
        Builtin::MapSetHas => ("MapSet.has", 2),
        Builtin::MapSetDelete => ("MapSet.delete", 2),
        Builtin::MapSetClear => ("MapSet.clear", 1),
        Builtin::MapSetGetSize => ("MapSet.size", 1),
        Builtin::MapSetForEach => ("MapSet.forEach", 1),
        Builtin::MapSetKeys => ("MapSet.keys", 1),
        Builtin::MapSetValues => ("MapSet.values", 1),
        Builtin::MapSetEntries => ("MapSet.entries", 1),
        // ── Date builtins ──
        Builtin::DateConstructor => ("Date", 1),
        Builtin::DateNow => ("Date.now", 0),
        Builtin::DateParse => ("Date.parse", 1),
        Builtin::DateUTC => ("Date.UTC", 1),
        // ── WeakMap builtins ──
        Builtin::WeakMapConstructor => ("WeakMap", 1),
        Builtin::WeakMapProtoSet => ("WeakMap.prototype.set", 3),
        Builtin::WeakMapProtoGet => ("WeakMap.prototype.get", 2),
        Builtin::WeakMapProtoHas => ("WeakMap.prototype.has", 2),
        Builtin::WeakMapProtoDelete => ("WeakMap.prototype.delete", 2),
        // ── WeakSet builtins ──
        Builtin::WeakSetConstructor => ("WeakSet", 1),
        Builtin::WeakSetProtoAdd => ("WeakSet.prototype.add", 2),
        Builtin::WeakSetProtoHas => ("WeakSet.prototype.has", 2),
        Builtin::WeakSetProtoDelete => ("WeakSet.prototype.delete", 2),
        // ── ArrayBuffer builtins ──
        Builtin::ArrayBufferConstructor => ("ArrayBuffer", 1),
        Builtin::ArrayBufferProtoByteLength => ("ArrayBuffer.prototype.byteLength", 1),
        Builtin::ArrayBufferProtoSlice => ("ArrayBuffer.prototype.slice", 3),
        // ── DataView builtins ──
        Builtin::DataViewConstructor => ("DataView", 3),
        Builtin::DataViewProtoGetFloat64 => ("DataView.prototype.getFloat64", 2),
        Builtin::DataViewProtoGetFloat32 => ("DataView.prototype.getFloat32", 2),
        Builtin::DataViewProtoGetInt32 => ("DataView.prototype.getInt32", 2),
        Builtin::DataViewProtoGetUint32 => ("DataView.prototype.getUint32", 2),
        Builtin::DataViewProtoGetInt16 => ("DataView.prototype.getInt16", 2),
        Builtin::DataViewProtoGetUint16 => ("DataView.prototype.getUint16", 2),
        Builtin::DataViewProtoGetInt8 => ("DataView.prototype.getInt8", 2),
        Builtin::DataViewProtoGetUint8 => ("DataView.prototype.getUint8", 2),
        Builtin::DataViewProtoSetFloat64 => ("DataView.prototype.setFloat64", 3),
        Builtin::DataViewProtoSetFloat32 => ("DataView.prototype.setFloat32", 3),
        Builtin::DataViewProtoSetInt32 => ("DataView.prototype.setInt32", 3),
        Builtin::DataViewProtoSetUint32 => ("DataView.prototype.setUint32", 3),
        Builtin::DataViewProtoSetInt16 => ("DataView.prototype.setInt16", 3),
        Builtin::DataViewProtoSetUint16 => ("DataView.prototype.setUint16", 3),
        Builtin::DataViewProtoSetInt8 => ("DataView.prototype.setInt8", 3),
        Builtin::DataViewProtoSetUint8 => ("DataView.prototype.setUint8", 3),
        // ── TypedArray constructors ──
        Builtin::Int8ArrayConstructor => ("Int8Array", 3),
        Builtin::Uint8ArrayConstructor => ("Uint8Array", 3),
        Builtin::Uint8ClampedArrayConstructor => ("Uint8ClampedArray", 3),
        Builtin::Int16ArrayConstructor => ("Int16Array", 3),
        Builtin::Uint16ArrayConstructor => ("Uint16Array", 3),
        Builtin::Int32ArrayConstructor => ("Int32Array", 3),
        Builtin::Uint32ArrayConstructor => ("Uint32Array", 3),
        Builtin::Float32ArrayConstructor => ("Float32Array", 3),
        Builtin::Float64ArrayConstructor => ("Float64Array", 3),
        // ── TypedArray prototype methods ──
        Builtin::TypedArrayProtoLength => ("TypedArray.prototype.length", 1),
        Builtin::TypedArrayProtoByteLength => ("TypedArray.prototype.byteLength", 1),
        Builtin::TypedArrayProtoByteOffset => ("TypedArray.prototype.byteOffset", 1),
        Builtin::TypedArrayProtoSet => ("TypedArray.prototype.set", 3),
        Builtin::TypedArrayProtoSlice => ("TypedArray.prototype.slice", 3),
        Builtin::TypedArrayProtoSubarray => ("TypedArray.prototype.subarray", 3),
        Builtin::GetBuiltinGlobal => ("get_builtin_global", 1),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{compile, compile_eval};
    use anyhow::Result;
    use wasmparser::{Parser, Payload, Validator};

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        compile(&program)
    }

    fn compile_eval_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_script_as_module(source)?;
        let program = wjsm_semantic::lower_eval_module(module)?;
        compile_eval(&program)
    }

    #[test]
    fn compile_exports_runtime_contract() -> Result<()> {
        let wasm_bytes = compile_source(r#"console.log("hello");"#)?;

        assert!(
            wasm_bytes
                .windows("console_log".len())
                .any(|window| window == b"console_log"),
            "wasm module should import env.console_log"
        );
        assert!(
            wasm_bytes
                .windows("main".len())
                .any(|window| window == b"main"),
            "wasm module should export main"
        );
        assert!(
            wasm_bytes
                .windows("memory".len())
                .any(|window| window == b"memory"),
            "wasm module should export memory"
        );

        Ok(())
    }

    #[test]
    fn compile_embeds_string_data_segment() -> Result<()> {
        let wasm_bytes = compile_source(r#"console.log("Hello, Backend!");"#)?;

        assert!(
            wasm_bytes
                .windows("Hello, Backend!\0".len())
                .any(|window| window == b"Hello, Backend!\0"),
            "wasm module should embed nul-terminated string data"
        );

        Ok(())
    }

    #[test]
    fn compile_encodes_undefined_constant() -> Result<()> {
        let wasm_bytes = compile_source("let x; console.log(x);")?;
        assert!(!wasm_bytes.is_empty());
        Ok(())
    }

    #[test]
    fn compile_eval_exports_entry_and_imports_runtime_state() -> Result<()> {
        let wasm_bytes = compile_eval_source("1 + 2")?;

        Validator::new().validate_all(&wasm_bytes)?;

        let mut imports = Vec::new();
        let mut exports = Vec::new();
        for payload in Parser::new(0).parse_all(&wasm_bytes) {
            match payload? {
                Payload::ImportSection(section) => {
                    for import in section.into_imports() {
                        let import = import?;
                        imports.push((import.module.to_string(), import.name.to_string()));
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export?;
                        exports.push(export.name.to_string());
                    }
                }
                _ => {}
            }
        }

        assert!(
            imports
                .iter()
                .any(|(module, name)| module == "env" && name == "memory"),
            "eval module should import parent memory"
        );
        assert!(
            imports
                .iter()
                .any(|(module, name)| module == "env" && name == "__heap_ptr"),
            "eval module should import parent heap pointer"
        );
        assert!(
            exports.iter().any(|name| name == "__eval_entry"),
            "eval module should export __eval_entry"
        );
        assert!(
            !exports.iter().any(|name| name == "main"),
            "eval module should not export main"
        );
        Ok(())
    }

    #[test]
    fn compile_direct_eval_exports_var_map_metadata() -> Result<()> {
        let wasm_bytes = compile_source(r#"var x = 1; eval("x");"#)?;

        Validator::new().validate_all(&wasm_bytes)?;

        let mut exports = Vec::new();
        for payload in Parser::new(0).parse_all(&wasm_bytes) {
            if let Payload::ExportSection(section) = payload? {
                for export in section {
                    exports.push(export?.name.to_string());
                }
            }
        }

        assert!(
            exports.iter().any(|name| name == "__eval_var_map_ptr"),
            "module should export eval variable map pointer"
        );
        assert!(
            exports.iter().any(|name| name == "__eval_var_map_count"),
            "module should export eval variable map count"
        );
        assert!(
            wasm_bytes
                .windows("$0.x\0".len())
                .any(|window| window == b"$0.x\0"),
            "eval variable map should embed scoped variable names"
        );
        Ok(())
    }

    #[test]
    fn dump_if_else_ir() -> Result<()> {
        let source = "if (true) { console.log(\"yes\"); } else { console.log(\"no\"); }";
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        assert!(program.dump_text().contains("fn @main"));
        Ok(())
    }
}
