use anyhow::Result;
use wjsm_ir::Program;

mod compiler;
pub mod host_abi;

pub use host_abi::builtin_arity;

use compiler::{CompileMode, Compiler};

pub fn compile(program: &Program) -> Result<Vec<u8>> {
    debug_assert_eq!(
        host_abi::HOST_IMPORT_NAMES.len(),
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
