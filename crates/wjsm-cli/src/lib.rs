use anyhow::Result;
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use swc_core::ecma::ast as swc_ast;
use wjsm_backend_wasm as backend_wasm;
use wjsm_parser as parser;
use wjsm_runtime as runtime;
use wjsm_semantic as semantic;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Build a JS/TS file to WebAssembly")]
    Build {
        #[arg(help = "The input file to compile")]
        input: String,
        #[arg(
            short,
            long,
            default_value = "out.wasm",
            help = "The output .wasm file"
        )]
        output: String,
        #[arg(
            long,
            help = "The root directory for module resolution (for ES modules)"
        )]
        root: Option<String>,
    },
    #[command(about = "Run a JS/TS file directly")]
    Run {
        #[arg(help = "The input file to run, or - for stdin")]
        input: String,
        #[arg(
            long,
            help = "The root directory for module resolution (for ES modules)"
        )]
        root: Option<String>,
    },
    #[command(about = "Dump IR for a JS/TS file")]
    DumpIr {
        #[arg(help = "The input file")]
        input: String,
    },
}
pub fn main_entry() -> Result<()> {
    let cli = Cli::parse();
    execute(cli)
}

pub fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Build { input, output, root } => {
            let wasm_bytes = compile_from_file_input(&input, root.as_deref())?;
            fs::write(&output, wasm_bytes)?;
            println!("Successfully compiled {} to {}", input, output);
        }
        Commands::Run { input, root } => {
            let wasm_bytes = if input == "-" {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                compile_source(&source)?
            } else {
                compile_from_file_input(&input, root.as_deref())?
            };
            runtime::execute(&wasm_bytes)?;
        }
        Commands::DumpIr { input } => {
            let source = fs::read_to_string(&input)?;
            let module = parser::parse_module(&source)?;
            let program = semantic::lower_module(module)?;
            println!("{}", program.dump_text());
            return Ok(());
        }
    }

    Ok(())
}

fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = parser::parse_module(source)?;
    let program = semantic::lower_module(module)?;
    backend_wasm::compile(&program)
}

enum CompilePlan {
    Bundle { entry: String, root: PathBuf },
    SingleSource(String),
}

fn compile_from_file_input(input: &str, root: Option<&str>) -> Result<Vec<u8>> {
    let plan = build_compile_plan(Path::new(input), root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => wjsm_module::bundle(&entry, &root),
        CompilePlan::SingleSource(source) => compile_source(&source),
    }
}

fn build_compile_plan(input: &Path, root: Option<&str>) -> Result<CompilePlan> {
    if let Some(root_path) = root {
        return Ok(bundle_plan_from_root(input.to_path_buf(), PathBuf::from(root_path)));
    }

    let source = fs::read_to_string(input)?;
    if !contains_es_module_syntax(&source)? {
        return Ok(CompilePlan::SingleSource(source));
    }

    let canonical_input = input.canonicalize()?;
    let parent = canonical_input
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot infer module root from '{}'", input.display()))?;
    let file_name = canonical_input.file_name().ok_or_else(|| {
        anyhow::anyhow!("Cannot infer module entry file name from '{}'", input.display())
    })?;

    let entry = format!("./{}", file_name.to_string_lossy());
    Ok(CompilePlan::Bundle {
        entry,
        root: parent.to_path_buf(),
    })
}

fn bundle_plan_from_root(input: PathBuf, root: PathBuf) -> CompilePlan {
    let rel = input.strip_prefix(&root).unwrap_or(&input);
    let entry = format!("./{}", rel.to_string_lossy());
    CompilePlan::Bundle { entry, root }
}

fn contains_es_module_syntax(source: &str) -> Result<bool> {
    let module = parser::parse_module(source)?;
    Ok(module.body.iter().any(|item| {
        matches!(
            item,
            swc_ast::ModuleItem::ModuleDecl(
                swc_ast::ModuleDecl::Import(_)
                    | swc_ast::ModuleDecl::ExportDecl(_)
                    | swc_ast::ModuleDecl::ExportNamed(_)
                    | swc_ast::ModuleDecl::ExportDefaultDecl(_)
                    | swc_ast::ModuleDecl::ExportDefaultExpr(_)
                    | swc_ast::ModuleDecl::ExportAll(_)
            )
        )
    }))
}
