// Re-exported (not just `use`d) so integration tests can assert the Layer 3 GC
// decision directly via `GcAnalysis::call_may_trigger_gc` — see
// tests/compiler_gc_analysis_spill.rs for why the WAT-level signal is ambiguous.
pub use crate::compiler_gc_analysis::GcAnalysis;
use anyhow::{Context, Result, bail};
use std::borrow::Cow;
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, DataSection, ElementSection, Elements,
    EntityType, ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType,
    ImportSection, Instruction as WasmInstruction, MemArg, MemorySection, MemoryType, Module,
    NameMap, NameSection, RefType, TableSection, TableType, TypeSection, ValType,
};
use wjsm_ir::{
    BasicBlock, BinaryOp, Builtin, CompareOp, Constant, Function as IrFunction, HomeObject,
    Instruction, Module as IrModule, Program, Terminator, UnaryOp, ValueId, constants,
    is_module_entry_ir_function, value,
};

pub mod host_import_registry;
mod shared_types;
pub mod support_module;
pub use support_module::emit_support_module;

// ── Shadow Stack Constants ─────────────────────────────────────────────
use wjsm_ir::{SHADOW_STACK_HEAP_GUARD_CANARY, SHADOW_STACK_HEAP_GUARD_SIZE, SHADOW_STACK_SIZE};
const EVAL_VAR_MAP_RECORD_SIZE: u32 = 20;

// ── Public API ──────────────────────────────────────────────────────────

pub fn compile(program: &Program) -> Result<Vec<u8>> {
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
    /// Blocks whose code was already emitted by common_direct_jump optimization
    /// inside compile_branch_body_with_context. Prevents duplicate emission without
    /// affecting compile_structured's main loop break behavior.
    branch_inline_compiled: std::collections::HashSet<usize>,
    /// Next available WASM function index (starts after imports).
    _next_import_func: u32,
    /// Map builtin → WASM function index.
    builtin_func_indices: HashMap<Builtin, u32>,
    /// Map SpecialHostImport → WASM function index (position in host import section).
    special_host_import_indices: HashMap<host_import_registry::SpecialHostImport, u32>,
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
    /// IR function ID → WASM function index (bridge for FunctionRef → table position).
    function_id_to_wasm_idx: HashMap<u32, u32>,
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
    /// Whether the current function is main (for throw handling).
    current_func_is_main: bool,
    /// WASM func index where user functions begin (= import count + helper count)
    user_func_base_idx: u32,
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
    /// WASM local index for safepoint spill saved shadow_sp (i32, P2)。
    /// safepoint prologue 保存 spill 前 shadow_sp，epilogue 恢复。
    /// 独立于 shadow_sp_scratch_idx（Call arg-save 用），避免冲突。
    safepoint_sp_saved_idx: u32,
    /// WASM local index for computed get/set index scratch (i32)。
    /// emit_computed_get/emit_computed_set 内部按 key 类型分派时暂存规范数字索引。
    /// 独立于 safepoint_sp_saved_idx：GetElem/SetElem 现为 safepoint，
    /// safepoint_sp_saved_idx 被 spill prologue 占用，不可复用。
    computed_idx_scratch_idx: u32,
    /// WASM local index for the base address of eval-visible variable storage.
    eval_var_base_local_idx: u32,
    /// WASM global index for alloc_counter (GC heuristic).
    alloc_counter_global_idx: u32,
    /// WASM global index for __object_heap_start (runtime GC heap base).
    #[allow(dead_code)]
    object_heap_start_global_idx: u32,
    /// WASM global index for __num_ir_functions (runtime GC root set).
    num_ir_functions_global_idx: u32,
    /// WASM global index for __shadow_stack_end (shadow stack bounds check).
    shadow_stack_end_global_idx: u32,
    /// WASM global index for array prototype handle.
    array_proto_handle_global_idx: u32,
    arr_proto_table_base: u32,
    /// WASM global index for __heap_limit (controlled JS heap budget end).
    heap_limit_global_idx: u32,
    /// WASM global index for Array.prototype method table base.
    arr_proto_table_base_global_idx: u32,
    /// WASM global index for Array.prototype method table length.
    arr_proto_table_len_global_idx: u32,
    /// WASM global index for Array.prototype method table ABI hash.
    arr_proto_table_hash_global_idx: u32,
    get_proto_from_ctor_func_idx: u32,
    /// WASM function index for nul-terminated string equality helper.
    string_eq_func_idx: u32,
    /// WASM global index for Object.prototype handle.
    object_proto_handle_global_idx: u32,
    /// WASM global index for startup bootstrap completion flag.
    bootstrap_done_global_idx: u32,
    /// WASM global index for function-property initialization completion flag.
    function_props_done_global_idx: u32,
    /// WASM global index for first function-property handle.
    function_props_base_global_idx: u32,
    /// WASM function index for globals initialization (P2.2).
    init_globals_func_idx: u32,
    /// WASM function index for idempotent primordial bootstrap.
    bootstrap_func_idx: u32,
    /// WASM function index for current-module function property initialization.
    init_function_props_func_idx: u32,
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
    /// 当前正在编译的 JS 函数 [[HomeObject]]，用于生成 super base。
    current_home_object: Option<HomeObject>,
    /// 当前正在编译的 JS 函数 ID，用于派生构造器 super() 解析。
    current_function_id: Option<wjsm_ir::FunctionId>,
    mode: CompileMode,
    function_param_counts: Vec<u32>,
    function_names: Vec<String>,
    // ── GC safepoint spill（P2）──
    /// 当前函数的 per-instruction liveness（P1 已实现，wjsm_ir::liveness::compute_liveness）。
    /// compile_function 入口计算一次。`None` 表示当前函数无 liveness 数据（例如未调用 compile_function）。
    current_fn_liveness: Option<
        HashMap<wjsm_ir::BasicBlockId, HashMap<usize, std::collections::HashSet<wjsm_ir::ValueId>>>,
    >,
    /// 当前函数的 ValueTy（P1 已实现，crate::analysis_value_ty::infer_value_ty）。
    current_fn_value_ty: Option<HashMap<wjsm_ir::ValueId, ValueTy>>,
    /// 当前函数的变量活跃集（变量名粒度，crate::analysis_liveness::compute_var_liveness）。
    /// 供 GC safepoint 的变量 local spill：弥补 per-ValueId liveness 看不到变量存活的空洞。
    current_fn_var_liveness:
        Option<HashMap<wjsm_ir::BasicBlockId, HashMap<usize, std::collections::HashSet<String>>>>,
    /// 当前函数的变量类型（变量名 → ValueTy）。仅 spill 可能持有 handle 的变量。
    current_fn_var_ty: Option<HashMap<String, ValueTy>>,
    /// 当前 emit 位置的 IR block 索引（= block id，见 wjsm-ir block_by_id O(1) by index 约定）。
    current_emit_block_idx: usize,
    /// 当前 emit 位置在当前 block 内的指令下标。
    current_emit_instr_idx: usize,
    // ── Layer 3: callee no-GC 分析 ──
    /// 模块级 GC 分析结果。compile_module 入口计算一次，用于 Call safepoint 省略判断。
    gc_analysis: Option<GcAnalysis>,
    /// P2.2: Normal mode 下 globals 的编译期初始值，用于 main prologue 的 global.set 初始化。
    /// Eval mode 下为 None（globals 由父模块初始化）。
    normal_init_values: Option<NormalGlobalsInit>,
    /// 编译期 allocation site 表，运行时 `--trace-gc` 用它把分配聚合到函数/IR 位置。
    allocation_sites: Vec<AllocationSiteRecord>,
    next_allocation_site_id: u32,
}

/// Normal mode 下需要初始化的 globals 编译期值。
/// 这些值依赖编译期计算的 heap 布局，无法通过 import 的 ConstExpr 设置，
/// 必须在 main prologue 中用 global.set 写入。
#[derive(Debug, Clone, Copy)]
struct NormalGlobalsInit {
    heap_ptr: i32,
    obj_table_ptr: i32,
    shadow_sp: i32,
    object_heap_start: i32,
    num_ir_functions: i32,
    shadow_stack_end: i32,
    eval_var_map_ptr: i32,
    eval_var_map_count: i32,
    arr_proto_table_base: i32,
    arr_proto_table_len: i32,
    arr_proto_table_hash: i64,
}

const ALLOCATION_SITES_SECTION: &str = "wjsm.gc.alloc_sites";
const ALLOCATION_SITES_MAGIC: &[u8; 8] = b"WJSMAS01";
const ALLOCATION_SITES_VERSION: u32 = 1;
const FIRST_ALLOCATION_SITE_ID: u32 = 2;

#[derive(Debug, Clone, Copy)]
enum AllocationSiteKind {
    Object = 1,
    Array = 2,
}

#[derive(Debug, Clone)]
struct AllocationSiteRecord {
    id: u32,
    function_id: Option<u32>,
    function_name: String,
    block: u32,
    instruction: u32,
    kind: AllocationSiteKind,
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

pub mod analysis_liveness;
pub mod analysis_value_ty;
pub use analysis_value_ty::{ValueTy, builtin_returns_scalar};
mod compiler_array_helpers;
mod compiler_builtins;
mod compiler_builtins_async_proxy;
mod compiler_builtins_collections;
mod compiler_builtins_core;
mod compiler_builtins_runtime;
mod compiler_builtins_string_math;
mod compiler_control;
mod compiler_core;
mod compiler_data;
mod compiler_gc_analysis;
mod compiler_helpers;
mod compiler_instructions;
mod compiler_module;
mod compiler_number_proto;

// ── Value ID collection ─────────────────────────────────────────────────
fn block_has_suspend(block: &BasicBlock) -> bool {
    block.instructions().iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
        )
    })
}

/// 检测 CFG 中的循环（通过 back-edge 识别）。
/// 返回按 header_idx 排序的 LoopInfo 列表。
fn detect_loops(blocks: &[BasicBlock]) -> Vec<LoopInfo> {
    use std::collections::HashMap;
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
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                for target in cases
                    .iter()
                    .map(|case| case.target)
                    .chain([*default_block, *exit_block])
                {
                    let t = target.0 as usize;
                    if t <= i {
                        back_edges.entry(t).or_default().push(i);
                    }
                }
            }
            _ => {}
        }
    }

    // 简单 CFG 可达性检查：从 start 出发，沿所有 CFG 边走，能否到达 target。
    fn can_reach(blocks: &[BasicBlock], start: usize, target: usize) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![start];
        while let Some(current) = stack.pop() {
            if current == target {
                return true;
            }
            if !visited.insert(current) {
                continue;
            }
            if let Some(block) = blocks.get(current) {
                match block.terminator() {
                    Terminator::Jump { target: t } => stack.push(t.0 as usize),
                    Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } => {
                        stack.push(false_block.0 as usize);
                        stack.push(true_block.0 as usize);
                    }
                    Terminator::Switch {
                        cases,
                        default_block,
                        exit_block,
                        ..
                    } => {
                        stack.extend(cases.iter().map(|case| case.target.0 as usize));
                        stack.push(default_block.0 as usize);
                        stack.push(exit_block.0 as usize);
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn can_reach_before(blocks: &[BasicBlock], start: usize, limit: usize) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![start];
        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }
            if let Some(block) = blocks.get(current) {
                match block.terminator() {
                    Terminator::Jump { target } => {
                        let target_idx = target.0 as usize;
                        if target_idx < limit {
                            return true;
                        }
                        stack.push(target_idx);
                    }
                    Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } => {
                        for target_idx in [true_block.0 as usize, false_block.0 as usize] {
                            if target_idx < limit {
                                return true;
                            }
                            stack.push(target_idx);
                        }
                    }
                    Terminator::Switch {
                        cases,
                        default_block,
                        exit_block,
                        ..
                    } => {
                        for target_idx in cases
                            .iter()
                            .map(|case| case.target.0 as usize)
                            .chain([default_block.0 as usize, exit_block.0 as usize])
                        {
                            if target_idx < limit {
                                return true;
                            }
                            stack.push(target_idx);
                        }
                    }
                    _ => {}
                }
            }
        }
        false
    }

    let mut loops: Vec<LoopInfo> = Vec::new();
    for header_idx in back_edges.keys() {
        let h = *header_idx;
        if let Terminator::Branch {
            true_block,
            false_block,
            ..
        } = blocks[h].terminator()
        {
            let true_idx = true_block.0 as usize;
            let false_idx = false_block.0 as usize;
            let sources = &back_edges[&h];
            let has_do_while_source = sources.iter().any(|&source_idx| {
                matches!(
                    blocks[source_idx].terminator(),
                    Terminator::Branch { true_block, .. } if true_block.0 as usize == h
                )
            });
            let reaches_backedge = sources
                .iter()
                .any(|&source_idx| can_reach(blocks, true_idx, source_idx));
            let reaches_exit = can_reach(blocks, true_idx, false_idx);
            let reaches_outer_target = can_reach_before(blocks, true_idx, h);
            if !(has_do_while_source || reaches_backedge || reaches_exit || reaches_outer_target) {
                continue;
            }
        }

        let exit_idx = if let Some(exit) =
            back_edges[&h]
                .iter()
                .find_map(|&source_idx| match blocks[source_idx].terminator() {
                    Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } if true_block.0 as usize == h => Some(false_block.0 as usize),
                    _ => None,
                }) {
            exit
        } else if let Terminator::Branch { false_block, .. } = blocks[h].terminator() {
            false_block.0 as usize
        } else {
            let mut exit = h + 1;
            for block in blocks.iter() {
                if let Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } = block.terminator()
                    && true_block.0 as usize == h
                {
                    exit = false_block.0 as usize;
                    break;
                }
            }
            exit
        };
        loops.push(LoopInfo {
            header_idx: h,
            exit_idx,
        });
    }

    loops.sort_by_key(|l| l.header_idx);
    let all_loops = loops.clone();
    loops.retain(|loop_info| {
        // 过滤掉终止符为 Jump 的"幻影循环"：只有 Branch 终止符才能是真正的循环头。
        // Jump 终止符的块是空块（如 for 循环增量），不应被当作独立循环。
        if !matches!(
            blocks[loop_info.header_idx].terminator(),
            Terminator::Branch { .. }
        ) {
            return false;
        }
        if let Terminator::Branch { true_block, .. } = blocks[loop_info.header_idx].terminator() {
            let true_idx = true_block.0 as usize;
            if true_idx < loop_info.header_idx {
                return !all_loops.iter().any(|outer| {
                    outer.header_idx == true_idx && outer.exit_idx == loop_info.exit_idx
                });
            }
        }
        true
    });
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
        Instruction::SuperCall {
            dest,
            callee,
            this_val,
            args,
            ..
        } => args.iter().fold(
            dest.map_or(callee.0.max(this_val.0), |d| {
                d.0.max(callee.0).max(this_val.0)
            }),
            |max, arg| max.max(arg.0),
        ),
        Instruction::ConstructCall {
            dest,
            callee,
            this_val,
            args,
        } => args.iter().fold(
            dest.map_or(callee.0.max(this_val.0), |d| {
                d.0.max(callee.0).max(this_val.0)
            }),
            |max, arg| max.max(arg.0),
        ),
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
        Instruction::GetSuperConstructor { dest } => dest.0,
        Instruction::NewPromise { dest } => dest.0,
        Instruction::PromiseResolve { promise, value } => promise.0.max(value.0),
        Instruction::PromiseReject { promise, reason } => promise.0.max(reason.0),
        Instruction::Suspend { promise, .. } => promise.0,
        Instruction::GeneratorSuspend { result, .. } => result.0,
        Instruction::IsException { dest, value } => dest.0.max(value.0),
        Instruction::EncodeException { dest, value } => dest.0.max(value.0),
        Instruction::ExceptionToObject { dest, value } => dest.0.max(value.0),
        Instruction::CollectRestArgs { dest, .. } => dest.0,
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::{compile, compile_eval};
    use anyhow::Result;
    use wasmparser::{Parser, Payload, Validator};

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module, false)?;
        compile(&program)
    }

    /// 与 compile_source 相同，但使用 eval 语义
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
        let program = wjsm_semantic::lower_module(module, false)?;
        assert!(program.dump_text().contains("fn @$module_main"));
        Ok(())
    }
}
