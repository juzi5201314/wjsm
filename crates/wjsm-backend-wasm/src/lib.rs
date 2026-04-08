use anyhow::{Context, Result};
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
        }
    }

    fn required_local_count(&self, function: &IrFunction) -> u32 {
        function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .flat_map(|instruction| match instruction {
                Instruction::Const { dest, .. } => [Some(dest.0), None, None],
                Instruction::Binary { dest, lhs, rhs, .. } => {
                    [Some(dest.0), Some(lhs.0), Some(rhs.0)]
                }
                Instruction::CallBuiltin { dest, args, .. } => {
                    let max_arg = args.iter().map(|value| value.0).max();
                    [dest.map(|value| value.0), max_arg, None]
                }
            })
            .flatten()
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

#[cfg(test)]
mod tests {
    use super::compile;
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module);
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
}
