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

pub mod host_import_registry;

// ── Shadow Stack Constants ─────────────────────────────────────────────
const SHADOW_STACK_SIZE: u32 = 65536; // 64KB = 8192 个 i64 槽位
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
    /// WASM local index for the base address of eval-visible variable storage.
    eval_var_base_local_idx: u32,
    /// WASM global index for alloc_counter (GC heuristic).
    alloc_counter_global_idx: u32,
    /// WASM global index for __object_heap_start (runtime GC heap base).
    object_heap_start_global_idx: u32,
    /// WASM global index for __num_ir_functions (runtime GC root set).
    num_ir_functions_global_idx: u32,
    /// WASM global index for __shadow_stack_end (shadow stack bounds check).
    shadow_stack_end_global_idx: u32,
    /// WASM global index for array prototype handle.
    array_proto_handle_global_idx: u32,
    /// Base table index for array prototype methods (Table[N+8])
    arr_proto_table_base: u32,
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
        Instruction::ConstructCall {
            callee,
            this_val,
            args,
        } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            callee.0.max(this_val.0).max(args_max)
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
        Instruction::IsException { dest, value } => dest.0.max(value.0),
        Instruction::EncodeException { dest, value } => dest.0.max(value.0),
        Instruction::ExceptionToObject { dest, value } => dest.0.max(value.0),
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
        Builtin::AsyncIteratorFrom => ("async_iterator.from", 1),
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
        Builtin::ArrayFrom => ("array.from", 1),
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
        Builtin::ObjectGroupBy => ("object.group_by", 2),
        Builtin::MapGroupBy => ("map.group_by", 2),
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
        // ── SharedArrayBuffer builtins ──
        Builtin::SharedArrayBufferConstructor => ("sharedarraybuffer_constructor", 1),
        Builtin::SharedArrayBufferProtoByteLength => ("sharedarraybuffer_proto_byte_length", 1),
        Builtin::SharedArrayBufferProtoSlice => ("sharedarraybuffer_proto_slice", 3),
        Builtin::SharedArrayBufferSpecies => ("sharedarraybuffer_species", 1),
        // ── Atomics builtins ──
        Builtin::AtomicsLoad => ("atomics_load", 2),
        Builtin::AtomicsStore => ("atomics_store", 3),
        Builtin::AtomicsAdd => ("atomics_add", 3),
        Builtin::AtomicsSub => ("atomics_sub", 3),
        Builtin::AtomicsAnd => ("atomics_and", 3),
        Builtin::AtomicsOr => ("atomics_or", 3),
        Builtin::AtomicsXor => ("atomics_xor", 3),
        Builtin::AtomicsExchange => ("atomics_exchange", 3),
        Builtin::AtomicsCompareExchange => ("atomics_compare_exchange", 4),
        Builtin::AtomicsIsLockFree => ("atomics_is_lock_free", 1),
        Builtin::AtomicsWait => ("atomics_wait", 4),
        Builtin::AtomicsNotify => ("atomics_notify", 3),
        Builtin::AtomicsWaitAsync => ("atomics_wait_async", 3),
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
        // ── TypedArray 新增构造器 ──
        Builtin::BigInt64ArrayConstructor => ("BigInt64Array", 3),
        Builtin::BigUint64ArrayConstructor => ("BigUint64Array", 3),
        // ── TypedArray 新增原型方法 ──
        Builtin::TypedArrayProtoFill => ("TypedArray.prototype.fill", 4),
        Builtin::TypedArrayProtoReverse => ("TypedArray.prototype.reverse", 1),
        Builtin::TypedArrayProtoIndexOf => ("TypedArray.prototype.indexOf", 3),
        Builtin::TypedArrayProtoLastIndexOf => ("TypedArray.prototype.lastIndexOf", 3),
        Builtin::TypedArrayProtoIncludes => ("TypedArray.prototype.includes", 3),
        Builtin::TypedArrayProtoJoin => ("TypedArray.prototype.join", 2),
        Builtin::TypedArrayProtoToString => ("TypedArray.prototype.toString", 1),
        Builtin::TypedArrayProtoCopyWithin => ("TypedArray.prototype.copyWithin", 4),
        Builtin::TypedArrayProtoAt => ("TypedArray.prototype.at", 2),
        Builtin::TypedArrayProtoForEach => ("TypedArray.prototype.forEach", 3),
        Builtin::TypedArrayProtoMap => ("TypedArray.prototype.map", 3),
        Builtin::TypedArrayProtoFilter => ("TypedArray.prototype.filter", 3),
        Builtin::TypedArrayProtoReduce => ("TypedArray.prototype.reduce", 4),
        Builtin::TypedArrayProtoReduceRight => ("TypedArray.prototype.reduceRight", 4),
        Builtin::TypedArrayProtoFind => ("TypedArray.prototype.find", 3),
        Builtin::TypedArrayProtoFindIndex => ("TypedArray.prototype.findIndex", 3),
        Builtin::TypedArrayProtoSome => ("TypedArray.prototype.some", 3),
        Builtin::TypedArrayProtoEvery => ("TypedArray.prototype.every", 3),
        Builtin::TypedArrayProtoSort => ("TypedArray.prototype.sort", 2),
        Builtin::TypedArrayProtoEntries => ("TypedArray.prototype.entries", 1),
        Builtin::TypedArrayProtoKeys => ("TypedArray.prototype.keys", 1),
        Builtin::TypedArrayProtoValues => ("TypedArray.prototype.values", 1),
        Builtin::GetBuiltinGlobal => ("get_builtin_global", 1),
        Builtin::CreateGlobalObject => ("create_global_object", 0),
        Builtin::CreateException => ("create_exception", 1),
        Builtin::ExceptionValue => ("exception_value", 1),
        Builtin::IsException => ("is_exception", 1),
        Builtin::NewTarget => ("new_target", 1),
        Builtin::CreateUnmappedArgumentsObject => ("create_unmapped_arguments_object", 2),
        Builtin::CreateMappedArgumentsObject => ("create_mapped_arguments_object", 3),
        // ── ScopeRecord eval bridge ──
        Builtin::ScopeRecordCreate => ("scope_record.create", 1),
        Builtin::ScopeRecordAddBinding => ("scope_record.add_binding", 5),
        Builtin::EvalGetBinding => ("eval.get_binding", 2),
        Builtin::EvalSetBinding => ("eval.set_binding", 3),
        Builtin::EvalHasBinding => ("eval.has_binding", 2),
        Builtin::EvalSuperBase => ("eval.super_base", 1),
        Builtin::ScopeRecordSetMeta => ("scope_record.set_meta", 3),
        Builtin::ScopeRecordDestroy => ("scope_record.destroy", 1),
        // ── WeakRef / FinalizationRegistry builtins ──
        Builtin::WeakRefConstructor => ("WeakRef", 1),
        Builtin::WeakRefProtoDeref => ("WeakRef.prototype.deref", 1),
        Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
        Builtin::FinalizationRegistryProtoRegister => {
            ("FinalizationRegistry.prototype.register", 4)
        }
        Builtin::FinalizationRegistryProtoUnregister => {
            ("FinalizationRegistry.prototype.unregister", 2)
        }
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
        assert!(program.dump_text().contains("fn @main"));
        Ok(())
    }
}
