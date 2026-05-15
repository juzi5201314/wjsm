use anyhow::Result;
use std::collections::HashMap;
use wasm_encoder::{
    CodeSection, DataSection, ElementSection, EntityType, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, ImportSection, MemorySection, Module,
    TableSection, TypeSection, ValType,
};
use wjsm_ir::{BasicBlockId, Builtin, Function as IrFunction, Terminator};


pub(crate) const SHADOW_STACK_SIZE: u32 = 65536;
pub(crate) const EVAL_VAR_MAP_RECORD_SIZE: u32 = 20;

pub(crate) struct Compiler {
    pub(crate) module: Module,
    pub(crate) types: TypeSection,
    pub(crate) imports: ImportSection,
    pub(crate) functions: FunctionSection,
    pub(crate) exports: ExportSection,
    pub(crate) codes: CodeSection,
    pub(crate) memory: MemorySection,
    pub(crate) data: DataSection,
    pub(crate) table: TableSection,
    pub(crate) elements: ElementSection,
    pub(crate) globals: GlobalSection,
    pub(crate) current_func: Option<Function>,
    pub(crate) string_data: Vec<u8>,
    pub(crate) data_base: u32,
    pub(crate) data_offset: u32,
    /// Map variable name → WASM local index (for LoadVar / StoreVar).
    pub(crate) var_locals: HashMap<String, u32>,
    /// Map variable name → current eval frame byte offset.
    pub(crate) var_memory_offsets: HashMap<String, u32>,
    /// Next available WASM local index (after SSA temporaries).
    pub(crate) next_var_local: u32,
    /// Phi locals: mapping from Phi dest ValueId → WASM local index.
    pub(crate) phi_locals: HashMap<u32, u32>,
    /// Set of block indices already compiled (for dedup in structured compilation).
    pub(crate) compiled_blocks: std::collections::HashSet<usize>,
    /// Next available WASM function index (starts after imports).
    pub(crate) _next_import_func: u32,
    /// Map builtin → WASM function index.
    pub(crate) builtin_func_indices: HashMap<Builtin, u32>,
    /// 活跃循环栈，用于跟踪嵌套循环的 WASM 标签深度。
    pub(crate) loop_stack: Vec<LoopInfo>,
    /// if/else 嵌套深度，用于计算 br 指令的标签深度偏移。
    pub(crate) if_depth: u32,
    /// Function table: table index → WASM func index.
    pub(crate) function_table: Vec<u32>,
    /// Reverse lookup: WASM func index → table position.
    pub(crate) function_table_reverse: HashMap<u32, u32>,
    /// IR function name → WASM func index.
    pub(crate) function_name_to_wasm_idx: HashMap<String, u32>,
    /// WASM index of $obj_new helper.
    pub(crate) obj_new_func_idx: u32,
    /// WASM index of $obj_get helper.
    pub(crate) obj_get_func_idx: u32,
    /// WASM index of $obj_set helper.
    pub(crate) obj_set_func_idx: u32,
    /// WASM index of $obj_delete helper.
    pub(crate) obj_delete_func_idx: u32,
    /// WASM index of $arr_new helper.
    pub(crate) arr_new_func_idx: u32,
    /// WASM index of $elem_get helper.
    pub(crate) elem_get_func_idx: u32,
    /// WASM index of $elem_set helper.
    pub(crate) elem_set_func_idx: u32,
    /// WASM index of $to_int32 helper.
    pub(crate) to_int32_func_idx: u32,
    /// WASM global index for heap pointer.
    pub(crate) heap_ptr_global_idx: u32,
    /// WASM global index for function properties array pointer.
    pub(crate) func_props_global_idx: u32,
    /// WASM global index for object handle table base address.
    pub(crate) obj_table_global_idx: u32,
    /// WASM global index for next available handle table entry count.
    pub(crate) obj_table_count_global_idx: u32,
    /// Number of IR functions (for pre-allocation of function property objects).
    pub(crate) num_ir_functions: u32,
    /// Whether the current function returns a value (Type 6 JS functions = true).
    pub(crate) current_func_returns_value: bool,
    /// Base offset for SSA value WASM local indices (0 for main, 8 for Type 6 JS functions).
    pub(crate) ssa_local_base: u32,
    /// String ptr cache: maps string content → data segment offset.
    pub(crate) string_ptr_cache: HashMap<String, u32>,
    /// WASM local index for string_concat scratch variable.
    pub(crate) string_concat_scratch_idx: u32,
    /// WASM global index for shadow stack pointer.
    pub(crate) shadow_sp_global_idx: u32,
    /// WASM local index for shadow_sp scratch variable (i32, used during Call).
    pub(crate) shadow_sp_scratch_idx: u32,
    /// WASM local index for the base address of eval-visible variable storage.
    pub(crate) eval_var_base_local_idx: u32,
    /// WASM function index for gc_collect host function.
    pub(crate) gc_collect_func_idx: u32,
    /// WASM global index for alloc_counter (GC heuristic).
    pub(crate) alloc_counter_global_idx: u32,
    /// WASM global index for __object_heap_start (runtime GC heap base).
    pub(crate) object_heap_start_global_idx: u32,
    /// WASM global index for __num_ir_functions (runtime GC root set).
    pub(crate) num_ir_functions_global_idx: u32,
    /// WASM global index for __shadow_stack_end (shadow stack bounds check).
    pub(crate) shadow_stack_end_global_idx: u32,
    /// WASM function index for closure_create import.
    pub(crate) closure_create_func_idx: u32,
    /// WASM function index for closure_get_func import.
    pub(crate) closure_get_func_idx: u32,
    /// WASM function index for closure_get_env import.
    pub(crate) closure_get_env_idx: u32,
    /// WASM function index for native_call import.
    pub(crate) native_call_func_idx: u32,
    /// WASM global index for array prototype handle.
    pub(crate) array_proto_handle_global_idx: u32,
    /// Base table index for array prototype methods (Table[N+8])
    pub(crate) arr_proto_table_base: u32,
    /// WASM function index for $obj_spread helper.
    pub(crate) obj_spread_func_idx: u32,
    /// WASM function index for $get_prototype_from_constructor helper.
    pub(crate) get_proto_from_ctor_func_idx: u32,
    /// WASM function index for nul-terminated string equality helper.
    pub(crate) string_eq_func_idx: u32,
    /// WASM global index for Object.prototype handle.
    pub(crate) object_proto_handle_global_idx: u32,
    /// WASM global index for __eval_var_map_ptr.
    pub(crate) eval_var_map_ptr_global_idx: u32,
    /// WASM global index for __eval_var_map_count.
    pub(crate) eval_var_map_count_global_idx: u32,
    /// Encoded eval variable map metadata emitted into the data section.
    pub(crate) eval_var_map_records: Vec<EvalVarMapRecord>,
    pub(crate) eval_var_map_ptr: u32,
    pub(crate) eval_var_map_count: u32,
    /// WASM local index for continuation handle (used in async state machine functions).
    pub(crate) continuation_local_idx: u32,
    pub(crate) current_function_has_eval: bool,
    pub(crate) mode: CompileMode,
    pub(crate) function_param_counts: Vec<u32>,
    pub(crate) function_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvalVarMapRecord {
    pub(crate) function_name: String,
    pub(crate) var_name: String,
    pub(crate) offset: u32,
}

/// 循环元信息（编译前预扫描得到）。
#[derive(Debug, Clone)]
pub(crate) struct LoopInfo {
    /// 循环头 block 索引（back-edge 目标）。
    pub(crate) header_idx: usize,
    /// 循环出口 block 索引（break 目标）。
    pub(crate) exit_idx: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Cfg {
    pub(crate) successors: Vec<Vec<usize>>,
    pub(crate) predecessors: Vec<Vec<usize>>,
}

impl Cfg {
    pub(crate) fn from_function(function: &IrFunction) -> Self {
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
pub(crate) enum Region {
    Linear { start_idx: usize },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct SwitchCaseRegion {
    pub(crate) _case_idx: usize,
    pub(crate) _target_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompileMode {
    Normal,
    Eval,
}

pub(crate) fn import_eval_global(
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
pub(crate) struct RegionTree {
    pub(crate) root: Region,
}

#[derive(Debug, Clone)]
pub(crate) enum RegionTreeError {
    MissingEntry,
}

impl RegionTree {
    pub(crate) fn build(function: &IrFunction, cfg: &Cfg) -> Result<Self, RegionTreeError> {
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
