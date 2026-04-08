use anyhow::Result;
use swc_core::ecma::ast as swc_ast;
use wasm_encoder::{
    CodeSection, DataSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    ImportSection, Instruction, MemorySection, MemoryType, Module, TypeSection, ValType,
};
use wjsm_ir::{Program, value};

pub fn compile(program: &Program) -> Result<Vec<u8>> {
    let mut compiler = Compiler::new();
    compiler.compile_module(program.module())?;

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
    current_func: Function,
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
            current_func: Function::new([]),
            string_data: Vec::new(),
            data_offset: 0,
        }
    }

    fn compile_module(&mut self, ast: &swc_ast::Module) -> Result<()> {
        for item in &ast.body {
            if let swc_ast::ModuleItem::Stmt(stmt) = item {
                self.compile_stmt(stmt)?;
            }
        }

        self.current_func.instruction(&Instruction::End);
        self.codes.function(&self.current_func);

        if !self.string_data.is_empty() {
            self.data.active(
                0,
                &wasm_encoder::ConstExpr::i32_const(0),
                self.string_data.clone(),
            );
        }

        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &swc_ast::Stmt) -> Result<()> {
        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => {
                self.compile_expr(&expr_stmt.expr)?;
            }
            _ => anyhow::bail!("Unsupported statement type"),
        }
        Ok(())
    }

    fn compile_expr(&mut self, expr: &swc_ast::Expr) -> Result<()> {
        match expr {
            swc_ast::Expr::Call(call) => self.compile_call(call),
            swc_ast::Expr::Bin(bin) => self.compile_bin(bin),
            swc_ast::Expr::Lit(lit) => self.compile_lit(lit),
            _ => anyhow::bail!("Unsupported expression type"),
        }
    }

    fn compile_call(&mut self, call: &swc_ast::CallExpr) -> Result<()> {
        if let swc_ast::Callee::Expr(callee_expr) = &call.callee {
            if let swc_ast::Expr::Member(member) = &**callee_expr {
                if let (swc_ast::Expr::Ident(obj), swc_ast::MemberProp::Ident(prop)) =
                    (&*member.obj, &member.prop)
                {
                    if obj.sym == "console" && prop.sym == "log" {
                        if call.args.is_empty() {
                            anyhow::bail!("console.log requires at least 1 argument");
                        }
                        self.compile_expr(&call.args[0].expr)?;
                        self.current_func.instruction(&Instruction::Call(0));
                        return Ok(());
                    }
                }
            }
        }
        anyhow::bail!("Unsupported call expression")
    }

    fn compile_bin(&mut self, bin: &swc_ast::BinExpr) -> Result<()> {
        self.compile_expr(&bin.left)?;
        self.current_func
            .instruction(&Instruction::F64ReinterpretI64);

        self.compile_expr(&bin.right)?;
        self.current_func
            .instruction(&Instruction::F64ReinterpretI64);

        match bin.op {
            swc_ast::BinaryOp::Add => {
                self.current_func.instruction(&Instruction::F64Add);
            }
            swc_ast::BinaryOp::Sub => {
                self.current_func.instruction(&Instruction::F64Sub);
            }
            swc_ast::BinaryOp::Mul => {
                self.current_func.instruction(&Instruction::F64Mul);
            }
            swc_ast::BinaryOp::Div => {
                self.current_func.instruction(&Instruction::F64Div);
            }
            _ => anyhow::bail!("Unsupported binary operation"),
        }
        self.current_func
            .instruction(&Instruction::I64ReinterpretF64);
        Ok(())
    }

    fn compile_lit(&mut self, lit: &swc_ast::Lit) -> Result<()> {
        match lit {
            swc_ast::Lit::Num(num) => {
                let bits = num.value.to_bits() as i64;
                self.current_func.instruction(&Instruction::I64Const(bits));
                Ok(())
            }
            swc_ast::Lit::Str(s) => {
                let ptr = self.data_offset;
                let mut bytes = s.value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;

                let encoded = value::encode_string_ptr(ptr);
                self.current_func
                    .instruction(&Instruction::I64Const(encoded));
                Ok(())
            }
            _ => anyhow::bail!("Unsupported literal type"),
        }
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
