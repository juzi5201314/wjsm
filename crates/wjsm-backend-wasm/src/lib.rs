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
// SHADOW_STACK_ALIGN: reserved for future use

// ── Public API ──────────────────────────────────────────────────────────

pub fn compile(program: &Program) -> Result<Vec<u8>> {
    let mut compiler = Compiler::new();
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
    data_offset: u32,
    /// Map variable name → WASM local index (for LoadVar / StoreVar).
    var_locals: HashMap<String, u32>,
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

impl Compiler {
    fn new() -> Self {
        let mut types = TypeSection::new();
        // Type 0: (i64) -> ()  — console_log
        types.ty().function(vec![ValType::I64], vec![]);
        // Type 1: () -> ()  — main
        types.ty().function(vec![], vec![]);
        // Type 2: (i64, i64) -> (i64)  — f64_mod, f64_pow
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 3: (i64) -> (i64)  — iterator/enumerator helpers
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
        // Type 4: () -> (i64)  — (unused placeholder)
        types.ty().function(vec![], vec![ValType::I64]);
        // Type 5: (i64, i64) -> () — unused (was begin_try, now removed)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![]);
        // Type 6: (i64, i32, i32) -> (i64)  — JS function signature (shadow stack)
        //   param 0 = this_val (i64), param 1 = args_base_ptr (i32), param 2 = args_count (i32)
        types.ty().function(
            vec![ValType::I64, ValType::I32, ValType::I32],
            vec![ValType::I64],
        );
        // Type 7: (i32) -> (i32)  — $obj_new, $alloc
        types.ty().function(vec![ValType::I32], vec![ValType::I32]);
        // Type 8: (i64, i32) -> (i64)  — $obj_get (boxed object + key → value)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I32], vec![ValType::I64]);
        // Type 9: (i64, i32, i64) -> ()  — $obj_set (boxed object + key + value)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I32, ValType::I64], vec![]);
        // Type 10: (i64) -> (i32)  — $to_int32
        types.ty().function(vec![ValType::I64], vec![ValType::I32]);
        // Type 11: (i64, i64) -> (i64)  — string_concat
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 12: (i64, i64, i32, i32) -> (i64) — JS 函数签名（含 env_obj）
        //   param 0 = env_obj (i64), param 1 = this_val (i64), param 2 = args_base_ptr (i32), param 3 = args_count (i32)
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I32, ValType::I32],
            vec![ValType::I64],
        );
        // Type 13: (i32, i64) -> (i64) — closure_create(func_idx, env_obj)
        types
            .ty()
            .function(vec![ValType::I32, ValType::I64], vec![ValType::I64]);
        // Type 14: (i32) -> (i32) — closure_get_func(closure_idx)
        types.ty().function(vec![ValType::I32], vec![ValType::I32]);
        // Type 15: (i32) -> (i64) — closure_get_env(closure_idx)
        types.ty().function(vec![ValType::I32], vec![ValType::I64]);
        let mut imports = ImportSection::new();
        // Import index 0: console_log: (i64) -> ()
        imports.import("env", "console_log", EntityType::Function(0));
        // Import index 1: f64_mod: (i64, i64) -> (i64)
        imports.import("env", "f64_mod", EntityType::Function(2));
        // Import index 2: f64_pow: (i64, i64) -> (i64)
        imports.import("env", "f64_pow", EntityType::Function(2));
        // Import index 3: throw: (i64) -> ()
        imports.import("env", "throw", EntityType::Function(0));
        // Import index 4: iterator_from: (i64) -> (i64)
        imports.import("env", "iterator_from", EntityType::Function(3));
        // Import index 5: iterator_next: (i64) -> (i64)
        imports.import("env", "iterator_next", EntityType::Function(3));
        // Import index 6: iterator_close: (i64) -> ()
        imports.import("env", "iterator_close", EntityType::Function(0));
        // Import index 7: iterator_value: (i64) -> (i64)
        imports.import("env", "iterator_value", EntityType::Function(3));
        // Import index 8: iterator_done: (i64) -> (i64)
        imports.import("env", "iterator_done", EntityType::Function(3));
        // Import index 9: enumerator_from: (i64) -> (i64)
        imports.import("env", "enumerator_from", EntityType::Function(3));
        // Import index 10: enumerator_next: (i64) -> (i64)
        imports.import("env", "enumerator_next", EntityType::Function(3));
        // Import index 11: enumerator_key: (i64) -> (i64)
        imports.import("env", "enumerator_key", EntityType::Function(3));
        // Import index 12: enumerator_done: (i64) -> (i64)
        imports.import("env", "enumerator_done", EntityType::Function(3));
        // Import index 13: typeof: (i64) -> (i64)
        imports.import("env", "typeof", EntityType::Function(3));
        // Import index 14: op_in: (i64, i64) -> (i64)
        imports.import("env", "op_in", EntityType::Function(2));
        // Import index 15: op_instanceof: (i64, i64) -> (i64)
        imports.import("env", "op_instanceof", EntityType::Function(2));
        // Import index 16: string_concat: (i64, i64) -> (i64)
        imports.import("env", "string_concat", EntityType::Function(11));
        // Import index 17: define_property: (i64, i32, i64) -> ()
        imports.import("env", "define_property", EntityType::Function(9));
        // Import index 18: get_own_prop_desc: (i64, i32) -> (i64)
        imports.import("env", "get_own_prop_desc", EntityType::Function(8));
        // Import index 19: abstract_eq: (i64, i64) -> (i64)
        imports.import("env", "abstract_eq", EntityType::Function(2));
        // Import index 20: abstract_compare: (i64, i64) -> (i64)
        imports.import("env", "abstract_compare", EntityType::Function(2));
        // Import index 21: gc_collect: (i32) -> (i32)
        imports.import("env", "gc_collect", EntityType::Function(7)); // Type 7 = (i32) -> i32
        // Import index 22: console_error: (i64) -> ()
        imports.import("env", "console_error", EntityType::Function(0));
        // Import index 23: console_warn: (i64) -> ()
        imports.import("env", "console_warn", EntityType::Function(0));
        // Import index 24: console_info: (i64) -> ()
        imports.import("env", "console_info", EntityType::Function(0));
        // Import index 25: console_debug: (i64) -> ()
        imports.import("env", "console_debug", EntityType::Function(0));
        // Import index 26: console_trace: (i64) -> ()
        imports.import("env", "console_trace", EntityType::Function(0));
        // Import index 27: set_timeout: (i64, i64) -> (i64)
        imports.import("env", "set_timeout", EntityType::Function(2));
        // Import index 28: clear_timeout: (i64) -> ()
        imports.import("env", "clear_timeout", EntityType::Function(0));
        // Import index 29: set_interval: (i64, i64) -> (i64)
        imports.import("env", "set_interval", EntityType::Function(2));
        // Import index 30: clear_interval: (i64) -> ()
        imports.import("env", "clear_interval", EntityType::Function(0));
        // Import index 31: fetch: (i64) -> (i64)
        imports.import("env", "fetch", EntityType::Function(3));
        // Import index 32: json_stringify: (i64) -> (i64)
        imports.import("env", "json_stringify", EntityType::Function(3));
        // Import index 33: json_parse: (i64) -> (i64)
        imports.import("env", "json_parse", EntityType::Function(3));
        // Import index 34: closure_create: (i32, i64) -> (i64)
        imports.import("env", "closure_create", EntityType::Function(13));
        // Import index 35: closure_get_func: (i32) -> (i32)
        imports.import("env", "closure_get_func", EntityType::Function(14));
        // Import index 36: closure_get_env: (i32) -> (i64)
        imports.import("env", "closure_get_env", EntityType::Function(15));
        let mut builtin_func_indices = HashMap::new();
        builtin_func_indices.insert(Builtin::ConsoleLog, 0);
        builtin_func_indices.insert(Builtin::ConsoleError, 22);
        builtin_func_indices.insert(Builtin::ConsoleWarn, 23);
        builtin_func_indices.insert(Builtin::ConsoleInfo, 24);
        builtin_func_indices.insert(Builtin::ConsoleDebug, 25);
        builtin_func_indices.insert(Builtin::ConsoleTrace, 26);
        builtin_func_indices.insert(Builtin::F64Mod, 1);
        builtin_func_indices.insert(Builtin::F64Exp, 2);
        builtin_func_indices.insert(Builtin::Throw, 3);
        builtin_func_indices.insert(Builtin::IteratorFrom, 4);
        builtin_func_indices.insert(Builtin::IteratorNext, 5);
        builtin_func_indices.insert(Builtin::IteratorClose, 6);
        builtin_func_indices.insert(Builtin::IteratorValue, 7);
        builtin_func_indices.insert(Builtin::IteratorDone, 8);
        builtin_func_indices.insert(Builtin::EnumeratorFrom, 9);
        builtin_func_indices.insert(Builtin::EnumeratorNext, 10);
        builtin_func_indices.insert(Builtin::EnumeratorKey, 11);
        builtin_func_indices.insert(Builtin::EnumeratorDone, 12);
        builtin_func_indices.insert(Builtin::TypeOf, 13);
        builtin_func_indices.insert(Builtin::In, 14);
        builtin_func_indices.insert(Builtin::InstanceOf, 15);
        builtin_func_indices.insert(Builtin::DefineProperty, 17);
        builtin_func_indices.insert(Builtin::GetOwnPropDesc, 18);
        builtin_func_indices.insert(Builtin::AbstractEq, 19);
        builtin_func_indices.insert(Builtin::AbstractCompare, 20);
        builtin_func_indices.insert(Builtin::SetTimeout, 27);
        builtin_func_indices.insert(Builtin::ClearTimeout, 28);
        builtin_func_indices.insert(Builtin::SetInterval, 29);
        builtin_func_indices.insert(Builtin::ClearInterval, 30);
        builtin_func_indices.insert(Builtin::Fetch, 31);
        builtin_func_indices.insert(Builtin::JsonStringify, 32);
        builtin_func_indices.insert(Builtin::JsonParse, 33);

        let functions = FunctionSection::new();

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);

        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 2, // 2 pages (128KB) to accommodate shadow stack
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });

        Self {
            module: Module::new(),
            types,
            imports,
            functions,
            exports,
            codes: CodeSection::new(),
            memory,
            data: DataSection::new(),
            table: TableSection::new(),
            elements: ElementSection::new(),
            globals: GlobalSection::new(),
            current_func: None,
            string_data: Vec::new(),
            data_offset: 0,
            var_locals: HashMap::new(),
            next_var_local: 0,
            phi_locals: HashMap::new(),
            compiled_blocks: std::collections::HashSet::new(),
            loop_stack: Vec::new(),
            if_depth: 0,
            _next_import_func: 37, // 37 imports (0-36)
            builtin_func_indices,
            function_table: Vec::new(),
            function_name_to_wasm_idx: HashMap::new(),
            obj_new_func_idx: 0,
            obj_get_func_idx: 0,
            obj_set_func_idx: 0,
            obj_delete_func_idx: 0,
            to_int32_func_idx: 0,
            current_func_returns_value: false,
            heap_ptr_global_idx: 0,
            func_props_global_idx: 0,
            obj_table_global_idx: 0,
            obj_table_count_global_idx: 0,
            num_ir_functions: 0,
            ssa_local_base: 0,
            string_ptr_cache: HashMap::new(),
            string_concat_scratch_idx: 0,
            shadow_sp_global_idx: 0,
            shadow_sp_scratch_idx: 0,
            gc_collect_func_idx: 21,
            alloc_counter_global_idx: 0,
            object_heap_start_global_idx: 6,
            num_ir_functions_global_idx: 7,
            shadow_stack_end_global_idx: 8,
            closure_create_func_idx: 34,
            closure_get_func_idx: 35,
            closure_get_env_idx: 36,
        }
    }
    /// Convert an IR ValueId to a WASM local index, accounting for ssa_local_base.
    fn local_idx(&self, val_id: u32) -> u32 {
        val_id + self.ssa_local_base
    }

    /// call_func_idx scratch local (i32) — 存放解析后的函数表索引
    fn call_func_idx_scratch(&self) -> u32 {
        self.shadow_sp_scratch_idx + 1
    }

    /// call_env_obj scratch local (i64) — 存放解析后的闭包环境对象
    fn call_env_obj_scratch(&self) -> u32 {
        self.string_concat_scratch_idx + 1
    }

    fn emit_resolve_callable_for_helper(
        &self,
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
        func.instruction(&WasmInstruction::Call(self.closure_get_func_idx));
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(self.closure_get_env_idx));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));

        func.instruction(&WasmInstruction::Else);
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));
        func.instruction(&WasmInstruction::End);
    }

    fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // Pass 1: Register all IR functions as WASM functions.
        let mut main_wasm_idx: Option<u32> = None;
        for function in module.functions() {
            let wasm_idx = self._next_import_func;
            self.function_name_to_wasm_idx
                .insert(function.name().to_string(), wasm_idx);

            if function.name() == "main" {
                // main: Type 1 = () -> ()
                self.functions.function(1);
                main_wasm_idx = Some(wasm_idx);
            } else {
                // JS functions: Type 6 = (i64×8) -> i64
                // JS functions: Type 12 = (i64, i64, i32, i32) -> i64 (含 env_obj)
                self.functions.function(12);
            }

            self.function_table.push(wasm_idx);
            self._next_import_func += 1;
        }

        // Add main export (must be known now).
        let main_idx = main_wasm_idx.context("backend-wasm expects lowered `main` function")?;
        self.exports.export("main", ExportKind::Func, main_idx);

        // Reserve indices for object helper functions (so they're known during user function compilation).
        self.obj_new_func_idx = self._next_import_func;
        self.functions.function(7);
        self.function_table.push(self._next_import_func);
        self._next_import_func += 1;

        self.obj_get_func_idx = self._next_import_func;
        self.functions.function(8);
        self.function_table.push(self._next_import_func);
        self._next_import_func += 1;

        self.obj_set_func_idx = self._next_import_func;
        self.functions.function(9);
        self.function_table.push(self._next_import_func);
        self._next_import_func += 1;

        self.obj_delete_func_idx = self._next_import_func;
        self.functions.function(8); // Type 8: (i64, i32) -> (i64)
        self.function_table.push(self._next_import_func);
        self._next_import_func += 1;

        self.to_int32_func_idx = self._next_import_func;
        self.functions.function(10); // Type 10: (i64) -> (i32)
        self.function_table.push(self._next_import_func);
        self._next_import_func += 1;

        // Pre-write typeof type strings to data segment start (nul-terminated)
        // 必须在编译用户函数之前设置，否则 encode_constant 会从 offset 0 开始分配字符串，
        // 随后 typeof 字符串会覆盖用户字符串数据。
        let typeof_strings: &[(u32, &str)] = &[
            (constants::TYPEOF_UNDEFINED_OFFSET, "undefined"),
            (constants::TYPEOF_OBJECT_OFFSET, "object"),
            (constants::TYPEOF_BOOLEAN_OFFSET, "boolean"),
            (constants::TYPEOF_STRING_OFFSET, "string"),
            (constants::TYPEOF_FUNCTION_OFFSET, "function"),
            (constants::TYPEOF_NUMBER_OFFSET, "number"),
        ];
        for &(offset, s) in typeof_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache.insert(s.to_string(), offset);
        }

        // Pre-write property descriptor strings after typeof strings
        // 用于 Object.getOwnPropertyDescriptor 返回的描述符对象
        let prop_desc_strings: &[(u32, &str)] = &[
            (constants::PROP_DESC_VALUE_OFFSET, "value"),
            (constants::PROP_DESC_WRITABLE_OFFSET, "writable"),
            (constants::PROP_DESC_ENUMERABLE_OFFSET, "enumerable"),
            (constants::PROP_DESC_CONFIGURABLE_OFFSET, "configurable"),
            (constants::PROP_DESC_GET_OFFSET, "get"),
            (constants::PROP_DESC_SET_OFFSET, "set"),
        ];
        for &(offset, s) in prop_desc_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache.insert(s.to_string(), offset);
        }

        self.data_offset = constants::USER_STRING_START;
        // 填充 string_data 到 data_offset，确保后续用户字符串追加到正确偏移量
        self.string_data.resize(self.data_offset as usize, 0);

        // Assign global indices before compile_object_helpers needs them.
        self.func_props_global_idx = 0;
        self.heap_ptr_global_idx = 1;
        self.obj_table_global_idx = 2;
        self.obj_table_count_global_idx = 3;
        self.num_ir_functions = module.functions().len() as u32;
        self.shadow_sp_global_idx = 4;
        self.alloc_counter_global_idx = 5;

        for function in module.functions() {
            if function.name() == "main" {
                self.compile_function(module, function)?;
            } else {
                self.compile_js_function(module, function)?;
            }
        }

        // Pass 3: Compile object helper functions.
        self.compile_object_helpers();

        // Pass 4: Build function table and emit data sections.
        self.table.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: self.function_table.len() as u64,
            maximum: None,
            table64: false,
            shared: false,
        });
        self.exports.export("__table", ExportKind::Table, 0);

        self.elements.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
        );

        // Allocate handle table at start of heap.
        // Handle table replaces func_props: maps handle_index → object ptr (i32).
        // Function property objects are stored at indices 0..num_functions-1.
        // Runtime objects are stored at indices num_functions..capacity.
        let heap_start = (self.data_offset + 7) & !7; // align to 8 bytes
        let num_functions = self.num_ir_functions;
        let handle_table_entries = std::cmp::max(256, num_functions * 2);
        let handle_table_size = handle_table_entries * 4;

        // Global 0: func_props_ptr (deprecated, set to 0)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
        // Global 1: heap_ptr (starts after handle table + shadow stack, mutable)
        let shadow_stack_base = heap_start + handle_table_size;
        let object_heap_start = shadow_stack_base + SHADOW_STACK_SIZE;
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(object_heap_start as i32),
        );
        self.heap_ptr_global_idx = 1;
        // Global 2: obj_table_ptr (immutable, points to handle table base)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(heap_start as i32),
        );
        self.obj_table_global_idx = 2;
        // Global 3: obj_table_count (mutable, starts at 0, incremented by $obj_new)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
        self.obj_table_count_global_idx = 3;
        // Global 4: shadow_sp (mutable, starts at shadow_stack_base)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(shadow_stack_base as i32),
        );
        self.shadow_sp_global_idx = 4;
        // Global 5: alloc_counter (mutable i32, initial 0, for GC heuristic)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
        self.alloc_counter_global_idx = 5;
        // Export alloc_counter for runtime debugging
        self.exports
            .export("__alloc_counter", ExportKind::Global, 5);
        // Export globals for runtime access
        self.exports
            .export("__obj_table_ptr", ExportKind::Global, 2);
        self.exports.export("__heap_ptr", ExportKind::Global, 1);
        self.exports
            .export("__obj_table_count", ExportKind::Global, 3);
        self.exports.export("__shadow_sp", ExportKind::Global, 4);
        // Global 6: __object_heap_start (immutable, for runtime GC heap base)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(object_heap_start as i32),
        );
        // Global 7: __num_ir_functions (immutable, for runtime GC root set)
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(num_functions as i32),
        );
        self.object_heap_start_global_idx = 6;
        self.num_ir_functions_global_idx = 7;
        self.exports
            .export("__object_heap_start", ExportKind::Global, 6);
        self.exports
            .export("__num_ir_functions", ExportKind::Global, 7);
        // Global 8: __shadow_stack_end (immutable, for shadow stack bounds check)
        let shadow_stack_end = shadow_stack_base + SHADOW_STACK_SIZE;
        self.globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(shadow_stack_end as i32),
        );
        self.exports
            .export("__shadow_stack_end", ExportKind::Global, 8);
        if !self.string_data.is_empty() {
            self.data
                .active(0, &ConstExpr::i32_const(0), self.string_data.clone());
        }
        Ok(())
    }

    fn compile_function(&mut self, module: &IrModule, function: &IrFunction) -> Result<()> {
        self.current_func_returns_value = false;
        self.ssa_local_base = 0;
        // Pass 1: assign WASM local indices to all variable names.
        self.assign_var_locals(function);

        // Pass 2: lower Phi to dedicated locals after variable locals to avoid index overlap.
        self.lower_phi_to_locals(function);

        let local_count = self.required_local_count(function);
        // scratch locals: i64 在前, i32 在后
        // string_concat (i64) at local_count
        // call_env_obj (i64) at local_count+1
        // shadow_sp (i32) at local_count+2
        // call_func_idx (i32) at local_count+3
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        let total_i64_locals = local_count + 2; // string_concat + call_env_obj
        let locals = if total_i64_locals == 0 && 2 == 0 {
            Vec::new()
        } else {
            vec![(total_i64_locals, ValType::I64), (2, ValType::I32)]
        };
        self.current_func = Some(Function::new(locals));

        // 预分配函数属性对象：为每个 IR 函数调用 $obj_new(8)，将返回的 handle_idx
        // 对应 obj_table[0..num_functions-1]，存储函数属性对象的 ptr。
        // 这样后续 GetProp/SetProp 可以通过 obj_table 统一查找。
        if function.name() == "main" {
            for _ in 0..self.num_ir_functions {
                self.emit(WasmInstruction::I32Const(8)); // capacity
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
                self.emit(WasmInstruction::Drop); // 丢弃返回的 handle_idx
            }
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function body — emit end directly.
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.phi_locals.clear();

        Ok(())
    }

    fn compile_js_function(&mut self, module: &IrModule, function: &IrFunction) -> Result<()> {
        self.current_func_returns_value = true;
        // Type 12 signature: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64
        // WASM params: local 0 = env_obj (i64), local 1 = this_val (i64),
        //              local 2 = args_base_ptr (i32), local 3 = args_count (i32)

        // Map $env/$this to WASM params (both bare and scoped names)
        self.var_locals.clear();
        self.var_locals.insert("$env".to_string(), 0);
        self.var_locals.insert("$this".to_string(), 1);

        // Count declared params (excluding $env/$this in both bare and scoped forms)
        let declared_params: Vec<&String> = function
            .params()
            .iter()
            .filter(|p| {
                let s = p.as_str();
                s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
            })
            .collect();

        // Allocate locals for declared params starting at local 4 (after env, this, args_base, args_count)
        // These will be loaded from shadow stack in the prologue
        let mut param_local_idx = 4;
        for param_name in &declared_params {
            self.var_locals
                .insert((*param_name).clone(), param_local_idx);
            param_local_idx += 1;
        }
        // Map scoped $env/$this param names to the same locals as bare names
        for p in function.params() {
            if p.ends_with(".$env") {
                self.var_locals.insert(p.clone(), 0);
            } else if p.ends_with(".$this") {
                self.var_locals.insert(p.clone(), 1);
            }
        }
        self.ssa_local_base = param_local_idx;
        // Variable locals start after param locals
        self.next_var_local = param_local_idx;
        // Assign variable locals for LoadVar/StoreVar.
        for block in function.blocks() {
            for instruction in block.instructions() {
                let name = match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                    _ => continue,
                };
                self.var_locals.entry(name.clone()).or_insert_with(|| {
                    let idx = self.next_var_local;
                    self.next_var_local += 1;
                    idx
                });
            }
        }
        self.lower_phi_to_locals(function);

        // 计算实际需要的 local 数量
        // SSA 值从 ssa_local_base 开始分配，需要 ssa_local_base + max_ssa 个 locals
        // 但 var_locals 已经包含了声明的参数，其索引也是从 ssa_local_base 开始
        // 所以实际需要的 locals 数量 = max_ssa (SSA 值数量)
        // 而不是 ssa_local_base + max_ssa (因为 params 是 WASM 参数，不是声明的 locals)
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        // 总 local 数量：SSA 值需要 ssa_local_base + max_ssa 个位置，
        // 或者 var/phi locals 的最大索引+1
        let total_locals = (max_ssa + self.ssa_local_base)
            .max(self.next_var_local)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1));

        // scratch locals: 所有 i64 在前，然后所有 i32（WASM locals 按 type 分组）
        // string_concat (i64) at total_locals
        // call_env_obj (i64) at total_locals+1
        // shadow_sp (i32) at total_locals+2
        // call_func_idx (i32) at total_locals+3
        self.string_concat_scratch_idx = total_locals;
        // call_env_obj scratch = string_concat + 1 (i64), computed by call_env_obj_scratch()
        self.shadow_sp_scratch_idx = total_locals + 2;
        // call_func_idx = shadow_sp + 1 (i32), computed by call_func_idx_scratch()
        let total_i64_locals = total_locals.saturating_sub(4) + 2; // string_concat + call_env_obj

        let locals = if total_i64_locals == 0 && 2 == 0 {
            Vec::new()
        } else {
            vec![(total_i64_locals, ValType::I64), (2, ValType::I32)]
        };
        self.current_func = Some(Function::new(locals));

        // ── Prologue: Load declared params from shadow stack ──
        // args_base_ptr is at local 2, args_count is at local 3
        for (i, param_name) in declared_params.iter().enumerate() {
            let param_local = *self.var_locals.get(*param_name).unwrap();

            // if i < args_count: load from shadow stack
            // else: set to undefined
            self.emit(WasmInstruction::I32Const(i as i32)); // i
            self.emit(WasmInstruction::LocalGet(3)); // args_count
            self.emit(WasmInstruction::I32LtU); // i < args_count (unsigned)

            self.emit(WasmInstruction::If(BlockType::Empty));
            // Load from shadow stack: memory[args_base_ptr + i*8]
            self.emit(WasmInstruction::LocalGet(2)); // args_base_ptr
            self.emit(WasmInstruction::I32Const((i * 8) as i32));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::I64Load(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit(WasmInstruction::LocalSet(param_local));
            self.emit(WasmInstruction::Else);
            // Out of bounds: set to undefined
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::LocalSet(param_local));
            self.emit(WasmInstruction::End);
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function — return undefined.
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.phi_locals.clear();

        Ok(())
    }

    fn compile_object_helpers(&mut self) {
        let heap_global = self.heap_ptr_global_idx;
        let obj_table_global = self.obj_table_global_idx;
        let obj_table_count_global = self.obj_table_count_global_idx;

        // ── $obj_new (param $capacity i32) (result i32) — Type 7 ──
        // 分配对象到堆上，将 ptr 存入 handle 表，返回 handle_idx。
        // 属性槽格式: [name_id(4), flags(4), value(8), getter(8), setter(8)] = 32 字节
        // GC 检查：如果 heap_ptr + size > memory.size * 64KB，调用 gc_collect
        {
            // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx
            let mut func = Function::new(vec![(3, ValType::I32)]);
            let gc_collect_idx = self.gc_collect_func_idx;

            // size = 12 + capacity * 32
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(1));

            // ── GC 检查 ──
            // 检查: heap_ptr + size > memory.size * 65536
            // 如果 true，调用 gc_collect(size)

            // 计算 heap_ptr + size
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);

            // 计算 memory.size * 65536 (使用 i64 避免溢出)
            func.instruction(&WasmInstruction::MemorySize(0));
            func.instruction(&WasmInstruction::I64ExtendI32U);
            func.instruction(&WasmInstruction::I64Const(65536));
            func.instruction(&WasmInstruction::I64Mul);
            func.instruction(&WasmInstruction::I32WrapI64);

            // 比较: heap_ptr + size > memory_limit
            func.instruction(&WasmInstruction::I32GtU);

            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 需要 GC - 调用 gc_collect(size)
            func.instruction(&WasmInstruction::LocalGet(1)); // size
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            // 检查返回值是否为 0（失败）
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // OOM - unreachable
            func.instruction(&WasmInstruction::Unreachable);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // ── Proactive GC: check alloc_counter threshold ──
            // 每 1000 次分配触发一次 gc_collect(0)
            func.instruction(&WasmInstruction::GlobalGet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(3)); // reuse handle_idx local as tmp
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            // Re-load counter value for comparison (consumed by GlobalSet)
            func.instruction(&WasmInstruction::LocalGet(3));
            // Check if counter >= GC_THRESHOLD (1000)
            func.instruction(&WasmInstruction::I32Const(1000));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // Call gc_collect(0) — proactive collection
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            func.instruction(&WasmInstruction::Drop); // ignore result
            // Reset alloc_counter
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::End);

            // ptr = heap_ptr; heap_ptr += size
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(-1)); // proto sentinel (0xFFFFFFFF)
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            // handle_idx = obj_table_count
            func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
            func.instruction(&WasmInstruction::LocalTee(3));
            // obj_table[handle_idx] = ptr
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // obj_table_count++
            func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(obj_table_count_global));
            // 返回 handle_idx
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $obj_get (param $boxed i64) (param $name_id i32) (result i64) — Type 8 ──
        // 通过 handle 表解析 boxed value，搜索属性（含原型链）。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32)
            // local 2 = num_props (i32), local 3 = i (i32), local 4 = slot_addr (i32)
            // local 5 = resolved ptr (i32), local 6 = flags (i32), local 7 = getter (i64)
            // local 8 = getter env_obj (i64), local 9 = getter func_idx (i32)
            let mut func = Function::new(vec![
                (5, ValType::I32),
                (2, ValType::I64),
                (1, ValType::I32),
            ]);

            // ── 通过 handle 表解析 ptr ──
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(5));

            // ptr == 0 → return undefined
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // ── 原型链遍历 ──
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));

            // 搜索当前对象的属性
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(3));

            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));
            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 找到！检查是否为访问器属性
            // 加载 flags (offset 4)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(6));
            // 检查 is_accessor 位
            func.instruction(&WasmInstruction::I32Const(constants::FLAG_IS_ACCESSOR));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 是访问器属性，加载 getter (offset 16)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(7));
            // 检查 getter 是否为 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // getter 是 undefined，返回 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 调用 getter: Type 12 签名 (env_obj, this_val, args_base, args_count) -> i64
            self.emit_resolve_callable_for_helper(&mut func, 7, 9, 8);
            // this_val = local 0, args_base = 0 (no args), args_count = 0
            func.instruction(&WasmInstruction::LocalGet(8)); // env_obj
            func.instruction(&WasmInstruction::LocalGet(0)); // this_val
            func.instruction(&WasmInstruction::I32Const(0)); // args_base (doesn't matter, no args)
            func.instruction(&WasmInstruction::I32Const(0)); // args_count
            func.instruction(&WasmInstruction::LocalGet(9)); // func_idx
            // call_indirect type 12, table 0
            func.instruction(&WasmInstruction::CallIndirect {
                type_index: 12,
                table_index: 0,
            });
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 是数据属性，返回 value (offset 8)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 跟随 __proto__（现在存储的是 handle_idx）
            // 读取 proto_handle = obj[0]
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(3)); // 暂存 proto_handle 到 local 3
            // 如果 proto_handle == -1 (哨兵)，退出循环
            func.instruction(&WasmInstruction::I32Const(-1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::BrIf(1));
            // 通过 handle 表解析 proto_handle → proto_ptr
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(5)); // 更新 ptr 为 proto_ptr
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 未找到
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $obj_set (param $boxed i64) (param $name_id i32) (param $value i64) — Type 9 ──
        // 通过 handle 表解析 boxed value，设置属性。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32), local 2 = $value (i64)
            // local 3 = (unused pad)
            // local 4 = num_props (i32), local 5 = i (i32), local 6 = slot_addr (i32), local 7 = capacity (i32)
            // local 8 = resolved ptr (i32), local 9 = handle_idx (i32), local 10 = flags (i32), local 11 = setter (i64)
            // local 12 = shadow_sp_scratch (i32), local 13 = setter func_idx (i32), local 15 = setter env_obj (i64)
            let mut func = Function::new(vec![
                (8, ValType::I32),
                (1, ValType::I64),
                (3, ValType::I32),
                (1, ValType::I64),
            ]);

            // ── 通过 handle 表解析 ptr ──
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::LocalTee(9)); // save handle_idx
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(8));

            // ── 搜索已有属性 ──
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(4));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));
            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 找到！检查是否为访问器属性
            // 加载 flags (offset 4)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(10));
            // 检查 is_accessor 位
            func.instruction(&WasmInstruction::I32Const(constants::FLAG_IS_ACCESSOR));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 是访问器属性，加载 setter (offset 24)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(11));
            // 检查 setter 是否为 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // setter 是 undefined，直接返回（静默失败）
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 调用 setter: Type 12 签名 (env_obj, this_val, args_base, args_count) -> i64
            self.emit_resolve_callable_for_helper(&mut func, 11, 13, 15);
            // 需要将 value (local 2) 写入影子栈
            // 保存 shadow_sp 到 local 12
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::LocalSet(12));
            // 写入 value 到影子栈
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::LocalGet(2)); // value
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            // shadow_sp += 8 (虽然这里只有1个参数，但保持一致性)
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::I32Const(8));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
            // 推入参数: env_obj, this_val (local 0), args_base (local 12), args_count (1)
            func.instruction(&WasmInstruction::LocalGet(15)); // env_obj
            func.instruction(&WasmInstruction::LocalGet(0)); // this_val
            func.instruction(&WasmInstruction::LocalGet(12)); // args_base
            func.instruction(&WasmInstruction::I32Const(1)); // args_count
            func.instruction(&WasmInstruction::LocalGet(13)); // func_idx
            // call_indirect type 12, table 0
            func.instruction(&WasmInstruction::CallIndirect {
                type_index: 12,
                table_index: 0,
            });
            // 恢复 shadow_sp
            func.instruction(&WasmInstruction::LocalGet(12));
            func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::Drop); // 丢弃返回值
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 是数据属性，更新 value (offset 8)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // ── 未找到 → 检查是否需要扩容 ──
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(7));

            // 如果 num_props >= capacity，需要扩容
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // 保存旧 ptr
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalSet(6)); // old_ptr

            // new_capacity = capacity * 2
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Const(2));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::LocalSet(7));

            // new_ptr = heap_ptr
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalSet(8));

            // heap_ptr += 12 + new_capacity * 32
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(heap_global));

            // 拷贝旧数据到新内存
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(5)); // copy_offset = 0
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            // copy_offset >= 12 + num_props * 32?
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1)); // break
            // new_ptr[copy_offset] = old_ptr[copy_offset] (i32)
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // copy_offset += 4
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End); // end loop
            func.instruction(&WasmInstruction::End); // end block

            // 更新 handle 表：obj_table[handle_idx] = new_ptr
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::LocalGet(9));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));

            // 更新 header 中的 capacity
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));

            func.instruction(&WasmInstruction::End); // end if reallocation

            // 添加新属性（无论是否扩容）
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            // 默认 flags: configurable | enumerable | writable
            func.instruction(&WasmInstruction::I32Const(
                constants::FLAG_CONFIGURABLE
                    | constants::FLAG_ENUMERABLE
                    | constants::FLAG_WRITABLE,
            ));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            // 初始化 getter 和 setter 为 undefined（防御性）
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            // num_props++
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));

            func.instruction(&WasmInstruction::End); // end function
            self.codes.function(&func);
        }

        // ── $obj_delete (param $boxed i64) (param $name_id i32) (result i64) — Type 8 ──
        // 通过 handle 表解析 boxed value，删除属性。返回 NaN-boxed bool。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32)
            // local 2 = num_props (i32), local 3 = i (i32), local 4 = slot_addr (i32)
            // local 5 = resolved ptr (i32), local 6 = last_slot_addr (i32)
            let mut func = Function::new(vec![(5, ValType::I32)]);

            // ── 通过 handle 表解析 ptr ──
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(5));

            // ptr == 0 → return false
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 搜索属性
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));

            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // 检查 configurable 标志 (flags bit 0)
            func.instruction(&WasmInstruction::LocalGet(4)); // slot_addr
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            })); // flags
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32And); // flags & 1
            func.instruction(&WasmInstruction::I32Eqz); // (flags & 1) == 0 → not configurable
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 找到！执行 swap-remove
            // num_props--
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Sub);
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));

            // 如果 i < num_props（减后），将最后一个槽复制到当前位置
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32LtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // last_slot_addr = ptr + 12 + num_props * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(12));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(6));

            // 复制 name_id
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));

            // 复制 flags
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));

            // 复制 value
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));

            // 复制 getter
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));

            // 复制 setter
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::End);

            // 返回 true
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 继续搜索
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 未找到 - 返回 false
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $to_int32 (param $val i64) (result i32) — Type 10 ──
        // Proper JS ToInt32: NaN/±Inf/sentinels → 0; numbers → ToInt32(wrap mod 2³²)
        {
            // local 0 = $val (i64, input), local 1 = f64 scratch
            let mut func = Function::new(vec![(1, ValType::F64)]);

            // Check: is this a raw f64 (not a NaN-box sentinel)?
            // is_f64: (val & 0x7FF8000000000000) != 0x7FF8000000000000
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(0x7FF8000000000000u64 as i64));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I64Const(0x7FF8000000000000u64 as i64));
            func.instruction(&WasmInstruction::I64Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // Raw f64 path — convert to f64
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::F64ReinterpretI64);
            func.instruction(&WasmInstruction::LocalTee(1));

            // NaN check: f != f
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // ±Inf check: abs(f) == inf
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(f64::INFINITY.into()));
            func.instruction(&WasmInstruction::F64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Fast path: |f| < 2^31 → safe i32.trunc_f64_s
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(2147483648.0f64.into())); // 2^31
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32TruncF64S);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Medium path: |f| < 2^53 → i64.trunc_f64_s + mask 32 bits
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(9007199254740992.0f64.into())); // 2^53
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I64TruncF64S);
            func.instruction(&WasmInstruction::I64Const(0xFFFFFFFF));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Large value path: manual modulo 2^32
            // mod = f - trunc(f / 2^32) * 2^32, then adjust if negative
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into())); // 2^32
            func.instruction(&WasmInstruction::F64Div);
            func.instruction(&WasmInstruction::F64Trunc);
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into()));
            func.instruction(&WasmInstruction::F64Mul);
            func.instruction(&WasmInstruction::F64Neg);
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Add); // mod = f - trunc(f/2^32)*2^32
            func.instruction(&WasmInstruction::LocalTee(1));

            // If mod < 0: add 2^32
            func.instruction(&WasmInstruction::F64Const(0.0.into()));
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into()));
            func.instruction(&WasmInstruction::F64Add);
            func.instruction(&WasmInstruction::LocalSet(1));
            func.instruction(&WasmInstruction::End);

            // Now mod in [0, 2^32) — use unsigned truncation
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32TruncF64U);
            func.instruction(&WasmInstruction::Return);

            func.instruction(&WasmInstruction::End); // end raw f64 if

            // Not a raw number (sentinel) -> return 0
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }
    }

    fn compile_region_tree(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        region_tree: &RegionTree,
    ) -> Result<()> {
        match &region_tree.root {
            Region::Linear { start_idx } => self.compile_structured(module, function, *start_idx),
        }
    }
    /// Phi lowering pass: for each Phi instruction, allocate a WASM local for its dest,
    /// and schedule moves from source values in predecessor blocks.
    fn lower_phi_to_locals(&mut self, function: &IrFunction) {
        self.phi_locals.clear();
        let mut next_local = self.next_var_local;

        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Phi { dest, .. } = instruction {
                    self.phi_locals.insert(dest.0, next_local);
                    next_local += 1;
                }
            }
        }
        self.next_var_local = next_local;
    }

    fn assign_var_locals(&mut self, function: &IrFunction) {
        self.var_locals.clear();
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        self.next_var_local = max_ssa;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let name = match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                    _ => continue,
                };
                self.var_locals.entry(name.clone()).or_insert_with(|| {
                    let idx = self.next_var_local;
                    self.next_var_local += 1;
                    idx
                });
            }
        }
    }

    /// 结构化编译：按顺序处理 block，处理 Branch 为 WASM if/else，处理循环为 block/loop。
    fn compile_structured(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        start_idx: usize,
    ) -> Result<()> {
        let blocks = function.blocks();
        let loops = detect_loops(blocks);
        self.loop_stack.clear();
        self.if_depth = 0;
        let mut idx = start_idx;

        while idx < blocks.len() {
            // 关闭已到达出口的循环
            while let Some(top) = self.loop_stack.last() {
                if idx >= top.exit_idx {
                    self.emit(WasmInstruction::End); // loop end
                    self.emit(WasmInstruction::End); // block end
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            // 在循环头打开 block/loop
            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty)); // break target
                self.emit(WasmInstruction::Loop(BlockType::Empty)); // continue target
                self.loop_stack.push(loop_info.clone());
            }

            if self.compiled_blocks.contains(&idx) {
                break;
            }
            self.compiled_blocks.insert(idx);

            let block = &blocks[idx];

            // 编译指令
            for instruction in block.instructions() {
                self.compile_instruction(module, instruction)?;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    idx += 1;
                }
                Terminator::Unreachable => {
                    // 死代码块 — 跳过
                    idx += 1;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // back-edge：continue 循环
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // 跳到循环出口：break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转到非相邻 block（如 try/catch 跳过 finally_entry）
                        // 中间的 block 是不可达的，直接跳到目标
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转但不是循环 — 不应发生
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    }
                }
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    let true_idx = true_block.0 as usize;
                    let false_idx = false_block.0 as usize;

                    // 循环头条件（while/for 模式）：
                    // true → body, false → exit
                    // 发射：condition → i32.eqz → br_if (break if falsy)
                    if self
                        .loop_stack
                        .last()
                        .map_or(false, |l| l.header_idx == idx)
                    {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::I32Eqz);
                        let break_depth = self.loop_break_depth(false_idx).unwrap_or(1);
                        self.emit(WasmInstruction::BrIf(break_depth));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）：
                    // true → header (continue), false → exit
                    // 发射：condition → br_if (continue if truthy)
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::BrIf(depth));
                        idx = false_idx;
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    // 判断哪些 block 是 merge（应在 if/else/end 之后编译）。
                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                    // 编译 true 分支；若 true 直接通向 merge，也必须在该路径执行 Phi move。
                    if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?;
                    }

                    // 编译 false 分支；false 直达 merge 时用 else 分支承载 Phi move。
                    if false_is_merge {
                        self.emit(WasmInstruction::Else);
                        self.emit_phi_moves(blocks, idx, false_idx);
                    } else {
                        self.emit(WasmInstruction::Else);
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?;
                    }

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    // 继续到 merge block
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

                    // 如果 merge 已编译（循环 back-edge 情况），跳到下一个未编译块
                    if self.compiled_blocks.contains(&merge) {
                        let mut next = true_idx.max(false_idx) + 1;
                        while next < blocks.len() && self.compiled_blocks.contains(&next) {
                            next += 1;
                        }
                        idx = next;
                    } else {
                        idx = merge;
                    }
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block,
                } => {
                    // 构建 switch entry 列表（含 default），按 block index 排序以还原源码顺序
                    // 这样 fallthrough（如 default → 下一个 case）可以正确工作
                    let exit_idx = exit_block.0 as usize;
                    self.compiled_blocks.insert(idx);

                    struct SwitchEntry {
                        is_default: bool,
                        constant_idx: Option<u32>,
                        target_idx: usize,
                    }

                    let mut entries: Vec<SwitchEntry> = Vec::new();
                    for case in cases.iter() {
                        entries.push(SwitchEntry {
                            is_default: false,
                            constant_idx: Some(case.constant.0),
                            target_idx: case.target.0 as usize,
                        });
                    }
                    let default_idx = default_block.0 as usize;
                    entries.push(SwitchEntry {
                        is_default: true,
                        constant_idx: None,
                        target_idx: default_idx,
                    });

                    // 按 target block index 排序，还原源码中的声明顺序
                    entries.sort_by_key(|e| e.target_idx);

                    let num_entries = entries.len();
                    let default_pos = entries.iter().position(|e| e.is_default).unwrap();

                    // 发射 switch exit block（最外层）
                    self.emit(WasmInstruction::Block(BlockType::Empty));

                    // 发射 entry blocks（反序嵌套，entries[0] 最内层）
                    for _ in 0..num_entries {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    // 发射比较链（跳过 default entry）
                    for (i, entry) in entries.iter().enumerate() {
                        if entry.is_default {
                            continue;
                        }
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        let const_val = self.encode_constant(
                            &module.constants()[entry.constant_idx.unwrap() as usize],
                            module,
                        )?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    // br 到 default（fallback）
                    self.emit(WasmInstruction::Br(default_pos as u32));

                    // 按嵌套顺序编译 case body（从内到外 = 源码顺序）
                    for i in 0..num_entries {
                        self.emit(WasmInstruction::End); // 关闭 entry block
                        let entry_target = entries[i].target_idx;
                        let switch_break_depth = (num_entries - i - 1) as u32;
                        let extra_depth = (num_entries - i) as u32;
                        self.compile_switch_case(
                            module,
                            blocks,
                            entry_target,
                            exit_idx,
                            switch_break_depth,
                            extra_depth,
                            &loops,
                        )?;
                    }

                    // 关闭 exit block
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(exit_idx);

                    idx = exit_idx;
                }
                Terminator::Throw { value } => {
                    // 调用 runtime throw host function，然后 trap
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::Throw)
                        .copied()
                        .unwrap_or(3);
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Unreachable);
                    idx += 1;
                }
            }
        }

        // 关闭所有剩余的循环
        while self.loop_stack.pop().is_some() {
            self.emit(WasmInstruction::End); // loop end
            self.emit(WasmInstruction::End); // block end
        }

        Ok(())
    }

    /// 编译 switch case body。支持嵌套控制流（if/else、循环、嵌套 switch）。
    /// 从 case_idx 开始，跟随控制流编译所有属于 case body 的 block。
    fn compile_switch_case(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        case_start: usize,
        exit_idx: usize,
        switch_break_depth: u32,
        extra_depth: u32,
        loops: &[LoopInfo],
    ) -> Result<()> {
        let initial_loop_depth = self.loop_stack.len();
        let mut idx = case_start;

        loop {
            if idx >= blocks.len() {
                break;
            }

            // 关闭已到达出口的循环
            while let Some(top) = self.loop_stack.last() {
                if idx >= top.exit_idx && self.loop_stack.len() > initial_loop_depth {
                    self.emit(WasmInstruction::End);
                    self.emit(WasmInstruction::End);
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            // 在循环头打开 block/loop
            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                self.loop_stack.push(loop_info.clone());
            }

            if self.compiled_blocks.contains(&idx) {
                break;
            }
            self.compiled_blocks.insert(idx);

            let block = &blocks[idx];

            // 编译指令
            for instruction in block.instructions() {
                self.compile_instruction(module, instruction)?;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    break;
                }
                Terminator::Unreachable => {
                    break;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if target_idx == exit_idx {
                        // switch break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(switch_break_depth));
                        break;
                    } else if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // loop continue（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx += 1; // 循环在 case body 内部，继续编译下一个 block（循环出口）
                        } else {
                            break; // 循环在 case body 外部（switch 在循环内），退出 case body 编译
                        }
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // loop break（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx = target_idx; // 循环在 case body 内部，继续到循环出口
                        } else {
                            break; // 循环在 case body 外部，退出 case body 编译
                        }
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    }
                }
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    let true_idx = true_block.0 as usize;
                    let false_idx = false_block.0 as usize;

                    // 循环头条件（while/for 模式）：
                    if self
                        .loop_stack
                        .last()
                        .map_or(false, |l| l.header_idx == idx)
                    {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::I32Eqz);
                        let break_depth = self.loop_break_depth(false_idx).unwrap_or(1);
                        let adj = if false_idx >= case_start {
                            break_depth
                        } else {
                            break_depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        let adj = if true_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = false_idx;
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                    if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                        self.emit(WasmInstruction::Nop);
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?;
                    }

                    self.emit(WasmInstruction::Else);
                    if false_is_merge {
                        self.emit_phi_moves(blocks, idx, false_idx);
                        self.emit(WasmInstruction::Nop);
                    } else {
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?;
                    }

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    // 继续到 merge block
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

                    if self.compiled_blocks.contains(&merge) {
                        let mut next = true_idx.max(false_idx) + 1;
                        while next < blocks.len() && self.compiled_blocks.contains(&next) {
                            next += 1;
                        }
                        idx = next;
                    } else {
                        idx = merge;
                    }
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block: nested_exit,
                } => {
                    // case body 内嵌套的 switch
                    let num_cases = cases.len();
                    let nested_default_idx = default_block.0 as usize;
                    let nested_exit_idx = nested_exit.0 as usize;
                    // 发射嵌套 switch 的 WASM blocks
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    for _ in 0..num_cases {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    // 发射比较链
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        let const_val = self.encode_constant(
                            &module.constants()[case.constant.0 as usize],
                            module,
                        )?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    self.emit(WasmInstruction::Br(num_cases as u32));

                    // 编译嵌套 case bodies
                    // 性能优化：直接迭代 cases，避免创建中间向量。
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::End);
                        let cidx = case.target.0 as usize;
                        self.compiled_blocks.insert(cidx);
                        let nested_break = (num_cases - i) as u32;
                        let nested_extra = extra_depth + (num_cases - i) as u32 + 1;
                        self.compile_switch_case(
                            module,
                            blocks,
                            cidx,
                            nested_exit_idx,
                            nested_break,
                            nested_extra,
                            loops,
                        )?;
                    }

                    // 编译嵌套 default body
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(nested_default_idx);
                    self.compile_switch_case(
                        module,
                        blocks,
                        nested_default_idx,
                        nested_exit_idx,
                        0,
                        extra_depth + 1,
                        loops,
                    )?;

                    // 关闭 nested exit block
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(nested_exit_idx);

                    idx = nested_exit_idx;
                }
                Terminator::Throw { value } => {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::Throw)
                        .copied()
                        .unwrap_or(3);
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Unreachable);
                    break;
                }
            }
        }

        // 关闭在 case body 内打开的循环
        while self.loop_stack.len() > initial_loop_depth {
            self.loop_stack.pop();
            self.emit(WasmInstruction::End);
            self.emit(WasmInstruction::End);
        }

        Ok(())
    }

    /// 编译分支体（if/else 的 true 或 false block）。
    /// 处理 Jump 到 merge block（no-op）、Return（发射）、循环 continue/break（发射 br）。
    fn compile_branch_body(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
    ) -> Result<()> {
        if idx >= blocks.len() {
            return Ok(());
        }
        let block = &blocks[idx];

        for instruction in block.instructions() {
            self.compile_instruction(module, instruction)?;
        }

        match block.terminator() {
            Terminator::Return { value } => {
                self.emit_return(value);
            }
            Terminator::Jump { target } => {
                let target_idx = target.0 as usize;
                if let Some(depth) = self.loop_continue_depth(target_idx) {
                    // back-edge：continue 循环
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.emit(WasmInstruction::Br(depth));
                } else if let Some(depth) = self.loop_break_depth(target_idx) {
                    // 跳到循环出口：break
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.emit(WasmInstruction::Br(depth));
                } else {
                    // 普通 merge 跳转
                    self.emit_phi_moves(blocks, idx, target_idx);
                }
            }
            Terminator::Unreachable => {}
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                // 分支体内嵌套的 if/else
                let true_idx = true_block.0 as usize;
                let false_idx = false_block.0 as usize;

                self.emit_to_bool_i32(condition.0);
                self.if_depth += 1;
                self.emit(WasmInstruction::If(BlockType::Empty));

                let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                if true_is_merge {
                    self.emit_phi_moves(blocks, idx, true_idx);
                    self.emit(WasmInstruction::Nop);
                } else {
                    self.compiled_blocks.insert(true_idx);
                    self.compile_branch_body(module, blocks, true_idx)?;
                }

                self.emit(WasmInstruction::Else);
                if false_is_merge {
                    self.emit_phi_moves(blocks, idx, false_idx);
                    self.emit(WasmInstruction::Nop);
                } else {
                    self.compiled_blocks.insert(false_idx);
                    self.compile_branch_body(module, blocks, false_idx)?;
                }

                self.emit(WasmInstruction::End);
                self.if_depth -= 1;
            }
            _ => {
                self.emit(WasmInstruction::Unreachable);
            }
        }

        Ok(())
    }

    /// Emit Phi moves: for each Phi instruction in the target block that references
    /// the current predecessor block, emit a move from the source value to the Phi local.
    fn emit_phi_moves(&mut self, blocks: &[BasicBlock], pred_idx: usize, target_idx: usize) {
        if target_idx >= blocks.len() {
            return;
        }
        let target_block = &blocks[target_idx];
        for instruction in target_block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                for source in sources {
                    if source.predecessor.0 as usize == pred_idx {
                        if let Some(&phi_local) = self.phi_locals.get(&dest.0) {
                            self.emit(WasmInstruction::LocalGet(self.local_idx(source.value.0)));
                            self.emit(WasmInstruction::LocalSet(phi_local));
                        }
                    }
                }
            }
        }
    }

    /// Check if `false_idx` is the natural merge block for a branch where
    /// true path is at `true_idx` and false path is at `false_idx`.
    fn is_merge_block(&self, blocks: &[BasicBlock], true_idx: usize, false_idx: usize) -> bool {
        // false_idx is the merge if and only if the true block jumps to false_idx.
        // This catches the if-without-else pattern: Branch → (true: Jump to merge, merge)
        if let Some(true_block) = blocks.get(true_idx) {
            match true_block.terminator() {
                Terminator::Jump { target } if target.0 as usize == false_idx => return true,
                _ => {}
            }
        }
        false
    }

    /// Find the merge block where true and false paths converge.
    fn find_merge(&self, blocks: &[BasicBlock], true_idx: usize, false_idx: usize) -> usize {
        // Check where the true block jumps to
        if let Some(true_block) = blocks.get(true_idx) {
            match true_block.terminator() {
                Terminator::Jump { target } => return target.0 as usize,
                _ => {}
            }
        }
        // Check where the false block jumps to
        if let Some(false_block) = blocks.get(false_idx) {
            match false_block.terminator() {
                Terminator::Jump { target } => return target.0 as usize,
                _ => {}
            }
        }
        // Default: the block after the false block
        false_idx + 1
    }

    fn emit_return(&mut self, value: &Option<ValueId>) {
        if let Some(v) = value {
            self.emit(WasmInstruction::LocalGet(self.local_idx(v.0)));
        } else if self.current_func_returns_value {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        self.emit(WasmInstruction::Return);
    }

    fn loop_continue_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.header_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + self.if_depth);
            }
        }
        None
    }

    fn loop_break_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.exit_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + 1 + self.if_depth);
            }
        }
        None
    }

    // ── Instruction compilation ─────────────────────────────────────────────

    fn compile_instruction(&mut self, module: &IrModule, instruction: &Instruction) -> Result<()> {
        match instruction {
            Instruction::Const { dest, constant } => {
                let constant = module
                    .constants()
                    .get(constant.0 as usize)
                    .with_context(|| format!("missing constant {constant}"))?;
                let encoded = self.encode_constant(constant, module)?;
                self.emit(WasmInstruction::I64Const(encoded));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::Binary { dest, op, lhs, rhs } => {
                match op {
                    // 加法：先尝试字符串连接，失败再做数值加法
                    BinaryOp::Add => {
                        // 调用 string_concat(lhs, rhs)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::Call(16)); // import 16: string_concat
                        // 存到 scratch
                        self.emit(WasmInstruction::LocalSet(self.string_concat_scratch_idx));
                        // 检查结果是否为 undefined（哨兵值：表示无字符串操作数）
                        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // 结果是 undefined → 走数值加法 (F64Add)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Add);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::Else);
                        // 结果是字符串 → 直接使用
                        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 其他算术运算（f64 操作）
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        match op {
                            BinaryOp::Sub => self.emit(WasmInstruction::F64Sub),
                            BinaryOp::Mul => self.emit(WasmInstruction::F64Mul),
                            BinaryOp::Div => self.emit(WasmInstruction::F64Div),
                            _ => unreachable!(),
                        }
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 位运算（i32 操作）
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                        // 左操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 执行位运算
                        match op {
                            BinaryOp::BitAnd => self.emit(WasmInstruction::I32And),
                            BinaryOp::BitOr => self.emit(WasmInstruction::I32Or),
                            BinaryOp::BitXor => self.emit(WasmInstruction::I32Xor),
                            _ => unreachable!(),
                        }
                        // 转换回 Number
                        self.emit(WasmInstruction::F64ConvertI32S);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 移位运算（需要掩码右操作数）
                    BinaryOp::Shl | BinaryOp::Shr | BinaryOp::UShr => {
                        // 左操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToInt32 并掩码 0x1F
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        self.emit(WasmInstruction::I32Const(0x1F));
                        self.emit(WasmInstruction::I32And);
                        // 执行移位
                        match op {
                            BinaryOp::Shl => self.emit(WasmInstruction::I32Shl),
                            BinaryOp::Shr => self.emit(WasmInstruction::I32ShrS),
                            BinaryOp::UShr => self.emit(WasmInstruction::I32ShrU),
                            _ => unreachable!(),
                        }
                        // 转换回 Number
                        if matches!(op, BinaryOp::UShr) {
                            self.emit(WasmInstruction::F64ConvertI32U);
                        } else {
                            self.emit(WasmInstruction::F64ConvertI32S);
                        }
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    BinaryOp::Mod | BinaryOp::Exp => {
                        bail!("Mod/Exp should be lowered to CallBuiltin, not Binary op");
                    }
                }
                Ok(())
            }
            Instruction::Unary { dest, op, value } => {
                match op {
                    UnaryOp::Not => {
                        self.emit_to_bool_i32(value.0);
                        self.emit(WasmInstruction::I32Const(1));
                        self.emit(WasmInstruction::I32Xor);
                        self.emit(WasmInstruction::I64ExtendI32U);
                        let box_base = value::BOX_BASE as i64;
                        let tag_bool = (value::TAG_BOOL << 32) as i64;
                        self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                        self.emit(WasmInstruction::I64Or);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Neg => {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Neg);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Pos => {
                        // +x 应执行 ToNumber(x):
                        //   f64 → 原值; null → 0; true → 1; false → 0;
                        //   undefined / string / object / 其他 → NaN
                        let val_local = self.local_idx(value.0);
                        let box_base = value::BOX_BASE as i64;

                        // 检查是否为 NaN-boxed 值
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(box_base));
                        self.emit(WasmInstruction::I64And);
                        self.emit(WasmInstruction::I64Const(box_base));
                        self.emit(WasmInstruction::I64Eq);

                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // boxed: 按 tag 分派
                        // 提取 tag
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I64Const(0xF));
                        self.emit(WasmInstruction::I64And);
                        // TAG_NULL?
                        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // null → +0
                        self.emit(WasmInstruction::I64Const(0)); // encode_f64(0.0)
                        self.emit(WasmInstruction::Else);
                        // 提取 tag
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I64Const(0xF));
                        self.emit(WasmInstruction::I64And);
                        // TAG_BOOL?
                        self.emit(WasmInstruction::I64Const(value::TAG_BOOL as i64));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // boolean: 检查 payload
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(1));
                        self.emit(WasmInstruction::I64And);
                        self.emit(WasmInstruction::I64Const(1));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // true → 1.0
                        self.emit(WasmInstruction::I64Const(1.0f64.to_bits() as i64));
                        self.emit(WasmInstruction::Else);
                        // false → 0.0
                        self.emit(WasmInstruction::I64Const(0));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::Else);
                        // 其他 boxed 类型 → NaN
                        self.emit(WasmInstruction::I64Const(value::BOX_BASE as i64));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::Else);
                        // not boxed → raw f64, 返回原值
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::End);

                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::BitNot => {
                        // ~x: ToInt32(x) XOR 0xFFFFFFFF
                        // 1. Load value and convert to i32 (ToInt32)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 2. XOR with -1 (all ones)
                        self.emit(WasmInstruction::I32Const(-1));
                        self.emit(WasmInstruction::I32Xor);
                        // 3. Convert back to Number (f64) and NaN-box
                        self.emit(WasmInstruction::F64ConvertI32S);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Void => {
                        let _ = value;
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::IsNullish => {
                        self.emit_is_nullish_i32(value.0);
                        self.emit(WasmInstruction::I64ExtendI32U);
                        let box_base = value::BOX_BASE as i64;
                        let tag_bool = (value::TAG_BOOL << 32) as i64;
                        self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                        self.emit(WasmInstruction::I64Or);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Delete => {
                        // delete 操作符在语义层被转换为 DeleteProp 或 Const(true)
                        // 这里不应该被到达
                        bail!(
                            "UnaryOp::Delete should not be reached - delete is handled by DeleteProp instruction"
                        );
                    }
                }
                Ok(())
            }
            Instruction::Compare { dest, op, lhs, rhs } => {
                self.compile_compare(*dest, *op, *lhs, *rhs)
            }
            Instruction::Phi { dest, .. } => {
                let phi_local = self
                    .phi_locals
                    .get(&dest.0)
                    .copied()
                    .with_context(|| format!("phi {dest} has no assigned WASM local"))?;
                self.emit(WasmInstruction::LocalGet(phi_local));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::CallBuiltin {
                dest,
                builtin,
                args,
            } => self.compile_builtin_call(*dest, builtin, args),
            Instruction::LoadVar { dest, name } => {
                let local_idx = self
                    .var_locals
                    .get(name)
                    .with_context(|| format!("variable `{name}` has no assigned WASM local"))?;
                self.emit(WasmInstruction::LocalGet(*local_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::StoreVar { name, value } => {
                let local_idx = *self
                    .var_locals
                    .get(name)
                    .with_context(|| format!("variable `{name}` has no assigned WASM local"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::LocalSet(local_idx));
                Ok(())
            }
            Instruction::Call {
                dest,
                callee,
                this_val,
                args,
            } => {
                // 使用影子栈传递参数
                // Type 12 签名: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64
                // callee 可能是 TAG_FUNCTION 或 TAG_CLOSURE，运行时解析

                // Step 1: 保存 shadow_sp 到 scratch local
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

                // Step 1b: 影子栈边界检查
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const((args.len() * 8) as i32));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
                self.emit(WasmInstruction::I32GtU);
                self.emit(WasmInstruction::If(BlockType::Empty));
                self.emit(WasmInstruction::Unreachable);
                self.emit(WasmInstruction::End);
                // Step 2: 将所有参数写入影子栈
                for arg in args.iter() {
                    self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                    self.emit(WasmInstruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                    self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                    self.emit(WasmInstruction::I32Const(8));
                    self.emit(WasmInstruction::I32Add);
                    self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                }

                // Step 3: 运行时解析 callee → (func_idx, env_obj)
                // 检查 callee tag == TAG_CLOSURE (0xA)
                // ((callee >> 32) & 0xF) == 0xA ?
                let call_func_idx_scratch = self.call_func_idx_scratch();
                let call_env_obj_scratch = self.call_env_obj_scratch();

                // 计算 tag
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(0xA)); // TAG_CLOSURE
                self.emit(WasmInstruction::I64Eq);
                // if closure
                self.emit(WasmInstruction::If(BlockType::Empty));
                // closure path: 调用 closure_get_func + closure_get_env
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::Call(self.closure_get_func_idx));
                self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::Call(self.closure_get_env_idx));
                self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
                self.emit(WasmInstruction::Else);
                // function path: func_idx = callee & 0xFFFFFFFF, env_obj = undefined
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
                self.emit(WasmInstruction::End);

                // Step 4: 推入 call_indirect 参数
                // 顺序: env_obj (i64), this_val (i64), args_base (i32), args_count (i32), func_idx (i32)
                self.emit(WasmInstruction::LocalGet(call_env_obj_scratch));
                self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(args.len() as i32));
                self.emit(WasmInstruction::LocalGet(call_func_idx_scratch));

                // Step 5: call_indirect type 12
                self.emit(WasmInstruction::CallIndirect {
                    type_index: 12,
                    table_index: 0,
                });

                // Step 6: 恢复 shadow_sp
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));

                // Step 7: 处理返回值
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Instruction::NewObject { dest, capacity } => {
                // Call $obj_new(capacity)
                self.emit(WasmInstruction::I32Const(*capacity as i32));
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
                // Result is i32 ptr — encode as object handle.
                // object_handle = BOX_BASE | (TAG_OBJECT << 32) | ptr
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_object = (value::TAG_OBJECT << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::GetProp { dest, object, key } => {
                // Pass full boxed i64 value — helper resolves tag internally.
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key: lower 32 bits (string pointer or name_id).
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::I32WrapI64);
                // Call $obj_get(boxed, name_id) -> i64
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::SetProp { object, key, value } => {
                // Pass full boxed i64 value — helper resolves tag internally.
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key.
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::I32WrapI64);
                // Value (i64 NaN-boxed).
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                // Call $obj_set(boxed, name_id, value)
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
                Ok(())
            }
            Instruction::DeleteProp { dest, object, key } => {
                // delete obj.prop -> bool (成功删除返回 true)
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key: lower 32 bits.
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::I32WrapI64);
                // Call $obj_delete(boxed, name_id) -> i64 (NaN-boxed bool)
                self.emit(WasmInstruction::Call(self.obj_delete_func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(())
            }
            Instruction::SetProto { object, value } => {
                // 验证 value 是有效的对象/函数引用后再设置 __proto__
                // 条件: is_boxed(value) AND (tag == OBJECT OR tag == FUNCTION)
                let val_local = self.local_idx(value.0);
                let obj_local = self.local_idx(object.0);
                let box_base = value::BOX_BASE as i64;

                // (1) is_boxed: (val & BOX_BASE) == BOX_BASE → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Eq);

                // (2) tag == OBJECT: ((val >> 32) & 0xF) == TAG_OBJECT → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_OBJECT as i64));
                self.emit(WasmInstruction::I64Eq);

                // (3) tag == FUNCTION: ((val >> 32) & 0xF) == TAG_FUNCTION → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);

                // (2) OR (3): tag_valid → i32
                self.emit(WasmInstruction::I32Or);
                // (1) AND tag_valid: combined → i32
                self.emit(WasmInstruction::I32And);

                // 条件分支：仅当 tag 有效时执行 __proto__ 存储
                // 需要通过 handle 表解析 obj 和 value 的真实 ptr
                self.emit(WasmInstruction::If(BlockType::Empty));
                // 解析 obj handle → real obj ptr
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::I32Const(4));
                self.emit(WasmInstruction::I32Mul);
                self.emit(WasmInstruction::GlobalGet(self.obj_table_global_idx));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // 直接存储 value 的 handle_idx（不需要解析为 ptr）
                // handle_idx = value 的低 32 位
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I32WrapI64);
                // 存储：obj[0] = value_handle_idx
                self.emit(WasmInstruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                self.emit(WasmInstruction::End);
                Ok(())
            }
        }
    }

    fn compile_compare(
        &mut self,
        dest: ValueId,
        op: CompareOp,
        lhs: ValueId,
        rhs: ValueId,
    ) -> Result<()> {
        // For Phase 3: implement strict equality and numeric comparisons.
        // All values are i64 NaN-boxed.
        //
        // For strict equality: check if both are f64, then compare as f64.
        // For numeric comparisons: reinterpret as f64 and compare.
        //
        // The result is a NaN-boxed bool (BOX_BASE | TAG_BOOL << 32 | 0 or 1).

        let box_base = value::BOX_BASE as i64;
        match op {
            CompareOp::StrictEq | CompareOp::StrictNotEq => {
                // StrictEq: 类型相同且值相同。
                // 对于两个 plain f64（非 NaN-boxed），使用 f64.eq：
                //   - 0 === -0 → true ✓
                //   - NaN === NaN → false ✓
                // 对于两个 NaN-boxed 值，使用 i64 eq 比较原始位：
                //   - null === null → true ✓
                //   - null === undefined → false（tag 不同）✓
                //   - bool/string/handle 同类型同值 → true ✓
                // 混合类型（一个 f64 一个 NaN-boxed）→ false ✓

                // 检查 lhs 是否为 plain f64：(lhs & BOX_BASE) != BOX_BASE
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Ne); // 1 if lhs is plain f64

                // 检查 rhs 是否为 plain f64
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Ne); // 1 if rhs is plain f64

                // both_f64 = lhs_is_f64 && rhs_is_f64
                self.emit(WasmInstruction::I32And);

                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                // 两者都是 plain f64：使用 f64.eq
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::F64Eq);
                self.emit(WasmInstruction::Else);
                // 至少一个是 NaN-boxed：使用 i64 位比较
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::End);

                if matches!(op, CompareOp::StrictNotEq) {
                    self.emit(WasmInstruction::I32Const(1));
                    self.emit(WasmInstruction::I32Xor);
                }

                // 将 i32 bool 转为 NaN-boxed bool
                self.emit(WasmInstruction::I64ExtendI32U);
                let tag_bool = (value::TAG_BOOL << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
            }
        }

        Ok(())
    }

    fn compile_builtin_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        match builtin {
            Builtin::ConsoleLog
            | Builtin::ConsoleError
            | Builtin::ConsoleWarn
            | Builtin::ConsoleInfo
            | Builtin::ConsoleDebug
            | Builtin::ConsoleTrace => {
                let first_arg = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least one argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(first_arg.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::Debugger => {
                // No-op in Phase 3
                Ok(())
            }
            Builtin::F64Mod | Builtin::F64Exp => {
                // f64_mod(a, b) / f64_pow(a, b) — call runtime host function
                let lhs = args.first().context("F64Mod/Exp expects 2 arguments")?;
                let rhs = args.get(1).context("F64Mod/Exp expects 2 arguments")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::SetTimeout | Builtin::SetInterval => {
                let callback = args
                    .first()
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                let delay = args
                    .get(1)
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(callback.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(delay.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::ClearTimeout | Builtin::ClearInterval => {
                let timer_id = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(timer_id.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::CreateClosure => {
                // args: [func_ref_val, env_obj_val]
                // func_ref_val 是 NaN-boxed 函数值 → 提取 table_idx (i32.wrap_i64)
                // env_obj_val 是 NaN-boxed 环境对象 (i64)
                // 调用 closure_create(table_idx, env_obj) → i64 (TAG_CLOSURE 编码)
                let func_ref_val = args
                    .get(0)
                    .with_context(|| "CreateClosure expects func_ref arg")?;
                let env_obj_val = args
                    .get(1)
                    .with_context(|| "CreateClosure expects env_obj arg")?;
                // 推入 func_idx (i32): 从 NaN-boxed 函数值提取
                self.emit(WasmInstruction::LocalGet(self.local_idx(func_ref_val.0)));
                self.emit(WasmInstruction::I32WrapI64);
                // 推入 env_obj (i64)
                self.emit(WasmInstruction::LocalGet(self.local_idx(env_obj_val.0)));
                // 调用 closure_create
                self.emit(WasmInstruction::Call(self.closure_create_func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::Fetch | Builtin::JsonStringify | Builtin::JsonParse => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::Throw => {
                if let Some(val) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(3);
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Unreachable);
                Ok(())
            }
            Builtin::IteratorFrom | Builtin::EnumeratorFrom => {
                let val = args
                    .first()
                    .context("IteratorFrom/EnumeratorFrom expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorNext | Builtin::EnumeratorNext => {
                let handle = args
                    .first()
                    .context("IteratorNext/EnumeratorNext expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorClose => {
                let handle = args.first().context("IteratorClose expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                Ok(())
            }
            Builtin::IteratorValue | Builtin::EnumeratorKey => {
                let handle = args
                    .first()
                    .context("IteratorValue/EnumeratorKey expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorDone | Builtin::EnumeratorDone => {
                let handle = args
                    .first()
                    .context("IteratorDone/EnumeratorDone expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::TypeOf => {
                // typeof(value) -> 返回类型名称字符串指针
                let val = args.first().context("TypeOf expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(13);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::In => {
                // prop in object -> bool
                let object = args.first().context("In expects 2 args (object, prop)")?;
                let prop = args.get(1).context("In expects 2 args (object, prop)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(prop.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(14);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::InstanceOf => {
                // value instanceof constructor -> bool
                let value = args
                    .first()
                    .context("InstanceOf expects 2 args (value, constructor)")?;
                let constructor = args
                    .get(1)
                    .context("InstanceOf expects 2 args (value, constructor)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(constructor.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(15);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AbstractEq => {
                // abstract_eq(a, b) -> bool
                let lhs = args.first().context("AbstractEq expects 2 args")?;
                let rhs = args.get(1).context("AbstractEq expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(19);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AbstractCompare => {
                // abstract_compare(a, b) -> bool (a < b)
                let lhs = args.first().context("AbstractCompare expects 2 args")?;
                let rhs = args.get(1).context("AbstractCompare expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(20);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::DefineProperty => {
                // define_property(obj: i64, key: i64, desc: i64) -> ()
                let obj_arg = args.first().context("DefineProperty expects 3 args")?;
                let key_arg = args.get(1).context("DefineProperty expects 3 args")?;
                let desc_arg = args.get(2).context("DefineProperty expects 3 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(desc_arg.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(17);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::GetOwnPropDesc => {
                // get_own_prop_desc(obj: i64, key: i64) -> i64
                let obj_arg = args.first().context("GetOwnPropDesc expects 2 args")?;
                let key_arg = args.get(1).context("GetOwnPropDesc expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(18);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
        }
    }

    // ── Constant encoding ────────────────────────────────────────────────────

    fn encode_constant(&mut self, constant: &Constant, _module: &IrModule) -> Result<i64> {
        match constant {
            Constant::Number(value) => Ok(value.to_bits() as i64),
            Constant::String(value) => {
                if let Some(&ptr) = self.string_ptr_cache.get(value) {
                    return Ok(value::encode_string_ptr(ptr));
                }
                let ptr = self.data_offset;
                let mut bytes = value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;
                self.string_ptr_cache.insert(value.clone(), ptr);

                Ok(value::encode_string_ptr(ptr))
            }
            Constant::Bool(b) => Ok(value::encode_bool(*b)),
            Constant::Null => Ok(value::encode_null()),
            Constant::Undefined => Ok(value::encode_undefined()),
            Constant::FunctionRef(function_id) => {
                // Table index = IR function index (functions are registered in order).
                // The element section maps table[i] → wasm_func_index.
                let table_idx = function_id.0;
                Ok(value::encode_function_idx(table_idx))
            }
        }
    }

    /// Emit WASM instructions that test whether a NaN-boxed i64 value is null or undefined.
    fn emit_is_nullish_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        let box_base = value::BOX_BASE as i64;

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::I32Or);

        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::End);
    }

    // ── Truthiness helpers ───────────────────────────────────────────────────

    /// Emit WASM instructions that convert a NaN-boxed i64 value to an i32 boolean
    /// (1 = truthy, 0 = falsy).
    ///
    /// This is the unified truthiness check for all control flow conditions.
    fn emit_to_bool_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        // Strategy:
        // 1. Check if it's undefined (TAG_UNDEFINED) → falsy
        // 2. Check if it's null (TAG_NULL) → falsy
        // 3. Check if it's bool (TAG_BOOL) → decode payload bit
        // 4. Check if it's f64 (no tag) → check 0.0 and NaN
        // 5. Otherwise (string, handle) → truthy
        //
        // Implementation using a series of nested if/else:

        let box_base = value::BOX_BASE as i64;

        // Check if the value is NaN-boxed (has BOX_BASE pattern)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);

        // If NaN-boxed, check the tag
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // NaN-boxed path: check tag
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);

        // Check TAG_UNDEFINED (0x2)
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // undefined is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_NULL (0x3)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // null is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_BOOL (0x4)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_BOOL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // Bool: extract payload bit (val & 1)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(1));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Else);
        // Check TAG_STRING (0x1): load first byte from memory to detect empty string
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_STRING as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // 运行时字符串句柄不对应线性内存指针；当前运行时只会产生非空字符串。
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::Else);
        // 编译期字符串：提取低 32 位作为内存指针，读取首字节
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // 如果首字节 == 0（nul-terminated 空串）则 falsy，否则 truthy
        self.emit(WasmInstruction::I32Eqz);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 空串 falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // 非空串 truthy
        self.emit(WasmInstruction::End); // end empty string check
        self.emit(WasmInstruction::End); // end runtime string check
        self.emit(WasmInstruction::Else);
        // Other NaN-boxed types (handle, etc.) → truthy
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::End); // end TAG_STRING check

        self.emit(WasmInstruction::End); // end TAG_BOOL check

        self.emit(WasmInstruction::End); // end TAG_NULL check

        self.emit(WasmInstruction::End); // end TAG_UNDEFINED check

        self.emit(WasmInstruction::Else);
        // Not NaN-boxed → it's a raw f64
        // Check for +0, -0, and NaN
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Const(0.0.into()));
        self.emit(WasmInstruction::F64Eq);
        // If equal to 0.0, it's falsy (+0 or -0)
        // Also need to check NaN (NaN != NaN, so NaN is falsy too)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 0 is falsy
        self.emit(WasmInstruction::Else);
        // Check for NaN: x != x
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Ne);
        // f64.ne returns 1 if NaN (since NaN != NaN)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // NaN is falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // non-zero number is truthy
        self.emit(WasmInstruction::End); // end NaN check
        self.emit(WasmInstruction::End); // end == 0 check

        self.emit(WasmInstruction::End); // end NaN-boxed check
    }

    // ── Local management ────────────────────────────────────────────────────

    fn required_local_count(&self, function: &IrFunction) -> u32 {
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        max_ssa
            .max(self.next_var_local)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1))
    }

    fn emit(&mut self, instruction: WasmInstruction<'_>) {
        self.current_func
            .as_mut()
            .expect("compiler function should be initialized before emission")
            .instruction(&instruction);
    }

    fn finish(mut self) -> Vec<u8> {
        // WASM section order: type, import, function, table, memory, global, export, element, code, data.
        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.functions);
        self.module.section(&self.table);
        self.module.section(&self.memory);
        self.module.section(&self.globals);
        self.module.section(&self.exports);
        self.module.section(&self.elements);
        self.module.section(&self.codes);

        if !self.string_data.is_empty() {
            self.module.section(&self.data);
        }

        self.module.finish()
    }
}

// ── Value ID collection ─────────────────────────────────────────────────

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
            _ => {}
        }
    }

    let mut loops: Vec<LoopInfo> = Vec::new();
    for (header_idx, _latches) in &back_edges {
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
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::compile;
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        compile(&program)
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
    fn dump_if_else_ir() -> Result<()> {
        let source = "if (true) { console.log(\"yes\"); } else { console.log(\"no\"); }";
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        Ok(())
    }
}
