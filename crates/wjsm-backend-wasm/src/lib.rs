use anyhow::{Context, Result};
use std::collections::HashMap;
use wasm_encoder::{
    CodeSection, DataSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    ImportSection, Instruction as WasmInstruction, MemorySection, MemoryType, Module, TypeSection,
    ValType,
};
use wjsm_ir::{
    BasicBlock, BinaryOp, Builtin, Constant, Function as IrFunction, Instruction,
    Module as IrModule, Program, value,
};

pub fn compile(program: &Program) -> Result<Vec<u8>> {
    let mut compiler = Compiler::new();
    compiler.compile_module(program)?;

    Ok(compiler.finish())
}

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
}

impl Compiler {
    fn new() -> Self {
        let mut types = TypeSection::new();
        types.ty().function(vec![ValType::I64], vec![]);
        types.ty().function(vec![], vec![]);

        let mut imports = ImportSection::new();
        imports.import("env", "console_log", EntityType::Function(0));

        let mut functions = FunctionSection::new();
        functions.function(1);

        let mut exports = ExportSection::new();
        exports.export("main", ExportKind::Func, 1);
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
        // First pass: assign WASM local indices to all variable names.
        self.assign_var_locals(function);

        let local_count = self.required_local_count(function);
        let locals = if local_count == 0 {
            Vec::new()
        } else {
            vec![(local_count, ValType::I64)]
        };
        self.current_func = Some(Function::new(locals));

        for block in function.blocks() {
            self.compile_block(module, block)?;
        }

        self.emit(WasmInstruction::End);
        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        Ok(())
    }

    fn assign_var_locals(&mut self, function: &IrFunction) {
        self.var_locals.clear();
        // Compute max SSA temporary index.
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .flat_map(collect_instruction_value_ids)
            .max()
            .map_or(0, |max| max + 1);

        // Assign variable names to local indices starting after SSA temporaries.
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

    fn compile_block(&mut self, module: &IrModule, block: &BasicBlock) -> Result<()> {
        for instruction in block.instructions() {
            self.compile_instruction(module, instruction)?;
        }

        match block.terminator() {
            wjsm_ir::Terminator::Return { .. } => Ok(()),
        }
    }

    fn compile_instruction(&mut self, module: &IrModule, instruction: &Instruction) -> Result<()> {
        match instruction {
            Instruction::Const { dest, constant } => {
                let constant = module
                    .constants()
                    .get(constant.0 as usize)
                    .with_context(|| format!("missing constant {constant}"))?;
                let encoded = self.encode_constant(constant);
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
                }

                self.emit(WasmInstruction::I64ReinterpretF64);
                self.emit(WasmInstruction::LocalSet(dest.0));
                Ok(())
            }
            Instruction::CallBuiltin { builtin, args, .. } => match builtin {
                Builtin::ConsoleLog => {
                    let first_arg = args
                        .first()
                        .context("console.log lowering expects at least one argument")?;
                    self.emit(WasmInstruction::LocalGet(first_arg.0));
                    self.emit(WasmInstruction::Call(0));
                    Ok(())
                }
            },
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

    fn encode_constant(&mut self, constant: &Constant) -> i64 {
        match constant {
            Constant::Number(value) => value.to_bits() as i64,
            Constant::String(value) => {
                let ptr = self.data_offset;
                let mut bytes = value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;

                value::encode_string_ptr(ptr)
            }
            Constant::Undefined => value::encode_undefined(),
        }
    }

    fn required_local_count(&self, function: &IrFunction) -> u32 {
        // 计算当前函数所需的 WASM local 总数。
        //
        // WASM local 索引空间由两部分共享：
        // - SSA 临时变量（指令的 dest/lhs/rhs/value 等）
        // - var 变量（LoadVar/StoreVar 中的 name 对应的 local）
        //
        // 取所有索引的最大值 + 1 即为所需 local 数量。
        // 注意：assign_var_locals 执行后 next_var_local 等于最后一个 var local 的索引 + 1，
        // 但这里仍从所有指令和 var_locals 中取 max 以确保安全。
        function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .flat_map(collect_instruction_value_ids)
            .chain(self.var_locals.values().copied())
            .max()
            .map_or(0, |max| max + 1)
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

/// 遍历指令收集所有引用的 ValueId（dest、lhs、rhs、value、args 等），
/// 用于计算 SSA 临时变量索引范围。
fn collect_instruction_value_ids(instruction: &Instruction) -> Vec<u32> {
    match instruction {
        Instruction::Const { dest, .. } => vec![dest.0],
        Instruction::Binary { dest, lhs, rhs, .. } => vec![dest.0, lhs.0, rhs.0],
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
        // Just verify it compiles without error — the runtime test validates output.
        assert!(!wasm_bytes.is_empty());
        Ok(())
    }
}
