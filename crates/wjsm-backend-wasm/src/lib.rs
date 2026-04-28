use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, DataSection, EntityType, ExportKind, ExportSection, Function,
    FunctionSection, ImportSection, Instruction as WasmInstruction, MemorySection, MemoryType,
    Module, TypeSection, ValType,
};
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, Function as IrFunction,
    Instruction, Module as IrModule, Program, Terminator, UnaryOp, ValueId, value,
};

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
    /// WASM function index counter for imports.
    _next_import_func: u32,
    /// Map builtin → WASM function index.
    builtin_func_indices: HashMap<Builtin, u32>,
    /// 活跃循环栈，用于跟踪嵌套循环的 WASM 标签深度。
    loop_stack: Vec<LoopInfo>,
    /// if/else 嵌套深度，用于计算 br 指令的标签深度偏移。
    if_depth: u32,
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
        // Type 3: (i64) -> (i64)  — iterator_from, enumerator_from, iterator_value, enumerator_key, iterator_done, enumerator_done, iterator_next, enumerator_next
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
        // Type 4: () -> (i64)  — (unused placeholder)
        types.ty().function(vec![], vec![ValType::I64]);
        // Type 5: (i64) -> ()  — throw, iterator_close, iterator_next, enumerator_next
        // (same as type 0 but we keep it separate for clarity)
        // Actually type 0 is already (i64) -> ()
        // Type 5: (i64, i64) -> () — begin_try, etc.
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![]);

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

        let mut builtin_func_indices = HashMap::new();
        builtin_func_indices.insert(Builtin::ConsoleLog, 0);
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

        let mut functions = FunctionSection::new();
        functions.function(1);

        let mut exports = ExportSection::new();
        exports.export("main", ExportKind::Func, 13); // main is func index 13 (13 imports + 1)
        exports.export("memory", ExportKind::Memory, 0);

        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 1,
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
            current_func: None,
            string_data: Vec::new(),
            data_offset: 0,
            var_locals: HashMap::new(),
            next_var_local: 0,
            phi_locals: HashMap::new(),
            compiled_blocks: std::collections::HashSet::new(),
            loop_stack: Vec::new(),
            if_depth: 0,
            _next_import_func: 13, // 13 imports (0-12)
            builtin_func_indices,
        }
    }

    fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        let main = module
            .functions()
            .iter()
            .find(|function| function.name() == "main")
            .context("backend-wasm expects lowered `main` function")?;

        self.compile_function(module, main)?;

        if !self.string_data.is_empty() {
            self.data.active(
                0,
                &wasm_encoder::ConstExpr::i32_const(0),
                self.string_data.clone(),
            );
        }

        Ok(())
    }

    fn compile_function(&mut self, module: &IrModule, function: &IrFunction) -> Result<()> {
        // Pass 1: assign WASM local indices to all variable names.
        self.assign_var_locals(function);

        // Pass 2: lower Phi to dedicated locals after variable locals to avoid index overlap.
        self.lower_phi_to_locals(function);

        let local_count = self.required_local_count(function);
        let locals = if local_count == 0 {
            Vec::new()
        } else {
            vec![(local_count, ValType::I64)]
        };
        self.current_func = Some(Function::new(locals));

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.compile_region_tree(module, function, &region_tree)?;

        self.emit(WasmInstruction::End);
        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        Ok(())
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
            .flat_map(collect_instruction_value_ids)
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
                    let num_cases = cases.len();
                    let default_idx = default_block.0 as usize;
                    let exit_idx = exit_block.0 as usize;
                    self.compiled_blocks.insert(idx);

                    // 收集所有 case block index
                    let case_indices: Vec<usize> =
                        cases.iter().map(|c| c.target.0 as usize).collect();

                    // 发射 switch exit block（最外层）
                    self.emit(WasmInstruction::Block(BlockType::Empty));

                    // 发射 default block
                    self.emit(WasmInstruction::Block(BlockType::Empty));

                    // 发射 case blocks（反序嵌套，case_0 最内层）
                    for _ in 0..num_cases {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    // 发射比较链
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::LocalGet(value.0));
                        let const_val =
                            self.encode_constant(&module.constants()[case.constant.0 as usize])?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    // br 到 default
                    self.emit(WasmInstruction::Br(num_cases as u32));

                    // 编译每个 case body
                    for i in 0..num_cases {
                        self.emit(WasmInstruction::End); // 关闭 case block
                        let case_idx = case_indices[i];
                        self.compiled_blocks.insert(case_idx);
                        let switch_break_depth = (num_cases - i) as u32;
                        let extra_depth = switch_break_depth + 1;
                        self.compile_switch_case(
                            module,
                            blocks,
                            case_idx,
                            exit_idx,
                            switch_break_depth,
                            extra_depth,
                        )?;
                    }

                    // 编译 default body
                    self.emit(WasmInstruction::End); // 关闭 default block
                    self.compiled_blocks.insert(default_idx);
                    self.compile_switch_case(module, blocks, default_idx, exit_idx, 0, 1)?;

                    // 关闭 exit block
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(exit_idx);

                    idx = exit_idx;
                }
                Terminator::Throw { value } => {
                    // 调用 runtime throw host function，然后 trap
                    self.emit(WasmInstruction::LocalGet(value.0));
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

    /// 编译 switch case body。处理 break (br 到 switch exit)、fall-through（no-op）、
    /// 循环 break/continue（调整 depth）以及 Return/Throw。
    fn compile_switch_case(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        case_idx: usize,
        exit_idx: usize,
        switch_break_depth: u32,
        extra_depth: u32,
    ) -> Result<()> {
        if case_idx >= blocks.len() {
            return Ok(());
        }
        let block = &blocks[case_idx];

        for instruction in block.instructions() {
            self.compile_instruction(module, instruction)?;
        }

        match block.terminator() {
            Terminator::Return { value } => {
                self.emit_return(value);
            }
            Terminator::Jump { target } => {
                let target_idx = target.0 as usize;
                if target_idx == exit_idx {
                    // switch break
                    self.emit_phi_moves(blocks, case_idx, target_idx);
                    self.emit(WasmInstruction::Br(switch_break_depth));
                } else if let Some(depth) = self.loop_continue_depth(target_idx) {
                    self.emit_phi_moves(blocks, case_idx, target_idx);
                    self.emit(WasmInstruction::Br(depth + extra_depth));
                } else if let Some(depth) = self.loop_break_depth(target_idx) {
                    self.emit_phi_moves(blocks, case_idx, target_idx);
                    self.emit(WasmInstruction::Br(depth + extra_depth));
                } else {
                    // fall-through 到下一个 case 或其他前向跳转
                    self.emit_phi_moves(blocks, case_idx, target_idx);
                }
            }
            Terminator::Throw { value } => {
                self.emit(WasmInstruction::LocalGet(value.0));
                let func_idx = self
                    .builtin_func_indices
                    .get(&Builtin::Throw)
                    .copied()
                    .unwrap_or(3);
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Unreachable);
            }
            Terminator::Unreachable => {
                // 死代码
            }
            _ => {
                // 其他 terminator（Branch, Switch）—— 递归编译
                self.emit(WasmInstruction::Unreachable);
            }
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
                } else {
                    self.compiled_blocks.insert(true_idx);
                    self.compile_branch_body(module, blocks, true_idx)?;
                }

                self.emit(WasmInstruction::Else);
                if false_is_merge {
                    self.emit_phi_moves(blocks, idx, false_idx);
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
                            self.emit(WasmInstruction::LocalGet(source.value.0));
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
            self.emit(WasmInstruction::LocalGet(v.0));
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
                let encoded = self.encode_constant(constant)?;
                self.emit(WasmInstruction::I64Const(encoded));
                self.emit(WasmInstruction::LocalSet(dest.0));
                Ok(())
            }
            Instruction::Binary { dest, op, lhs, rhs } => {
                self.emit(WasmInstruction::LocalGet(lhs.0));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::LocalGet(rhs.0));
                self.emit(WasmInstruction::F64ReinterpretI64);

                match op {
                    BinaryOp::Add => self.emit(WasmInstruction::F64Add),
                    BinaryOp::Sub => self.emit(WasmInstruction::F64Sub),
                    BinaryOp::Mul => self.emit(WasmInstruction::F64Mul),
                    BinaryOp::Div => self.emit(WasmInstruction::F64Div),
                    BinaryOp::Mod | BinaryOp::Exp => {
                        bail!("Mod/Exp should be lowered to CallBuiltin, not Binary op");
                    }
                }

                self.emit(WasmInstruction::I64ReinterpretF64);
                self.emit(WasmInstruction::LocalSet(dest.0));
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
                        self.emit(WasmInstruction::LocalSet(dest.0));
                    }
                    UnaryOp::Neg => {
                        self.emit(WasmInstruction::LocalGet(value.0));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Neg);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(dest.0));
                    }
                    UnaryOp::Pos => {
                        self.emit(WasmInstruction::LocalGet(value.0));
                        self.emit(WasmInstruction::LocalSet(dest.0));
                    }
                    UnaryOp::BitNot => {
                        bail!("bitwise NOT not yet supported");
                    }
                    UnaryOp::Void => {
                        let _ = value;
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                        self.emit(WasmInstruction::LocalSet(dest.0));
                    }
                    UnaryOp::IsNullish => {
                        self.emit_is_nullish_i32(value.0);
                        self.emit(WasmInstruction::I64ExtendI32U);
                        let box_base = value::BOX_BASE as i64;
                        let tag_bool = (value::TAG_BOOL << 32) as i64;
                        self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                        self.emit(WasmInstruction::I64Or);
                        self.emit(WasmInstruction::LocalSet(dest.0));
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
                self.emit(WasmInstruction::LocalSet(dest.0));
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
                self.emit(WasmInstruction::LocalSet(dest.0));
                Ok(())
            }
            Instruction::StoreVar { name, value } => {
                let local_idx = *self
                    .var_locals
                    .get(name)
                    .with_context(|| format!("variable `{name}` has no assigned WASM local"))?;
                self.emit(WasmInstruction::LocalGet(value.0));
                self.emit(WasmInstruction::LocalSet(local_idx));
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

        match op {
            CompareOp::StrictEq | CompareOp::StrictNotEq => {
                // StrictEq: values must be same type AND same value.
                // For Phase 3, if both are f64 (no tag), compare f64 bits.
                // If both have same tag, compare payload.
                // Otherwise, false.
                //
                // Simplification for Phase 3: just compare raw i64 bits.
                // This works correctly for f64 values and identical NaN-boxed values.
                self.emit(WasmInstruction::LocalGet(lhs.0));
                self.emit(WasmInstruction::LocalGet(rhs.0));
                self.emit(WasmInstruction::I64Eq);

                if matches!(op, CompareOp::StrictNotEq) {
                    self.emit(WasmInstruction::I32Const(1));
                    self.emit(WasmInstruction::I32Xor);
                }

                // Convert i32 boolean result to NaN-boxed bool
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_bool = (value::TAG_BOOL << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(dest.0));
            }
            CompareOp::Eq | CompareOp::NotEq => {
                // Loose equality: null == undefined, same type compare.
                // For Phase 3: approximate with strict equality for most cases.
                // null == undefined is handled specially.
                self.emit(WasmInstruction::LocalGet(lhs.0));
                self.emit(WasmInstruction::LocalGet(rhs.0));
                self.emit(WasmInstruction::I64Eq);

                // Also check null == undefined
                // If lhs is null and rhs is undefined (or vice versa), they're equal
                // For simplicity in Phase 3, just use i64 equality (works for same-encoded values)
                // TODO: implement proper loose equality

                if matches!(op, CompareOp::NotEq) {
                    self.emit(WasmInstruction::I32Const(1));
                    self.emit(WasmInstruction::I32Xor);
                }

                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_bool = (value::TAG_BOOL << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(dest.0));
            }
            CompareOp::Lt | CompareOp::LtEq | CompareOp::Gt | CompareOp::GtEq => {
                // Numeric comparison: reinterpret as f64 and compare
                self.emit(WasmInstruction::LocalGet(lhs.0));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::LocalGet(rhs.0));
                self.emit(WasmInstruction::F64ReinterpretI64);

                match op {
                    CompareOp::Lt => self.emit(WasmInstruction::F64Lt),
                    CompareOp::LtEq => self.emit(WasmInstruction::F64Le),
                    CompareOp::Gt => self.emit(WasmInstruction::F64Gt),
                    CompareOp::GtEq => self.emit(WasmInstruction::F64Ge),
                    _ => unreachable!(),
                }

                // Convert i32 boolean to NaN-boxed bool
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_bool = (value::TAG_BOOL << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(dest.0));
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
            Builtin::ConsoleLog => {
                let first_arg = args
                    .first()
                    .context("console.log expects at least one argument")?;
                self.emit(WasmInstruction::LocalGet(first_arg.0));
                self.emit(WasmInstruction::Call(0));
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
                self.emit(WasmInstruction::LocalGet(lhs.0));
                self.emit(WasmInstruction::LocalGet(rhs.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(d.0));
                }
                Ok(())
            }
            Builtin::Throw => {
                if let Some(val) = args.first() {
                    self.emit(WasmInstruction::LocalGet(val.0));
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
                self.emit(WasmInstruction::LocalGet(val.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(d.0));
                }
                Ok(())
            }
            Builtin::IteratorNext | Builtin::EnumeratorNext => {
                let handle = args
                    .first()
                    .context("IteratorNext/EnumeratorNext expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(handle.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(d.0));
                }
                Ok(())
            }
            Builtin::IteratorClose => {
                let handle = args.first().context("IteratorClose expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(handle.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                Ok(())
            }
            Builtin::IteratorValue | Builtin::EnumeratorKey => {
                let handle = args
                    .first()
                    .context("IteratorValue/EnumeratorKey expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(handle.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(d.0));
                }
                Ok(())
            }
            Builtin::IteratorDone | Builtin::EnumeratorDone => {
                let handle = args
                    .first()
                    .context("IteratorDone/EnumeratorDone expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(handle.0));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(d.0));
                }
                Ok(())
            }
            Builtin::BeginTry | Builtin::EndTry | Builtin::BeginFinally | Builtin::EndFinally => {
                // Phase 6
                bail!("builtin {builtin} not yet implemented");
            }
        }
    }

    // ── Constant encoding ────────────────────────────────────────────────────

    fn encode_constant(&mut self, constant: &Constant) -> Result<i64> {
        match constant {
            Constant::Number(value) => Ok(value.to_bits() as i64),
            Constant::String(value) => {
                let ptr = self.data_offset;
                let mut bytes = value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;

                Ok(value::encode_string_ptr(ptr))
            }
            Constant::Bool(b) => Ok(value::encode_bool(*b)),
            Constant::Null => Ok(value::encode_null()),
            Constant::Undefined => Ok(value::encode_undefined()),
        }
    }

    /// Emit WASM instructions that test whether a NaN-boxed i64 value is null or undefined.
    fn emit_is_nullish_i32(&mut self, val_local: u32) {
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
        self.emit(WasmInstruction::I64Const(0x7));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x7));
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
    fn emit_to_bool_i32(&mut self, val_local: u32) {
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
        self.emit(WasmInstruction::I64Const(0x7));
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
        self.emit(WasmInstruction::I64Const(0x7));
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
        self.emit(WasmInstruction::I64Const(0x7));
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
        // Other NaN-boxed types (string, handle, etc.) → truthy
        self.emit(WasmInstruction::I32Const(1));
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
            .flat_map(collect_instruction_value_ids)
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
        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.functions);
        self.module.section(&self.memory);
        self.module.section(&self.exports);
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

fn collect_instruction_value_ids(instruction: &Instruction) -> Vec<u32> {
    match instruction {
        Instruction::Const { dest, .. } => vec![dest.0],
        Instruction::Binary { dest, lhs, rhs, .. } => vec![dest.0, lhs.0, rhs.0],
        Instruction::Unary { dest, value, .. } => vec![dest.0, value.0],
        Instruction::Compare { dest, lhs, rhs, .. } => vec![dest.0, lhs.0, rhs.0],
        Instruction::Phi { dest, sources } => {
            let mut ids: Vec<u32> = sources.iter().map(|s| s.value.0).collect();
            ids.push(dest.0);
            ids
        }
        Instruction::CallBuiltin { dest, args, .. } => {
            let mut ids: Vec<u32> = args.iter().map(|v| v.0).collect();
            if let Some(d) = dest {
                ids.push(d.0);
            }
            ids
        }
        Instruction::LoadVar { dest, .. } => vec![dest.0],
        Instruction::StoreVar { value, .. } => vec![value.0],
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
        eprintln!("IR:\n{}", program.dump_text());
        Ok(())
    }
}
