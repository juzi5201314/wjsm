use anyhow::Result;
use clap::{Parser, Subcommand};
use std::fs;

pub use wjsm_backend_jit as backend_jit;
pub use wjsm_backend_wasm as backend_wasm;
pub use wjsm_ir as ir;
pub use wjsm_parser as parser;
pub use wjsm_runtime as runtime;
pub use wjsm_semantic as semantic;

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
    },
    #[command(about = "Run a JS/TS file directly")]
    Run {
        #[arg(help = "The input file to run")]
        input: String,
    },
}

pub fn main_entry() -> Result<()> {
    let cli = Cli::parse();
    execute(cli)
}

pub fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Build { input, output } => {
            let source = fs::read_to_string(&input)?;
            let wasm_bytes = compile_source(&source)?;
            fs::write(&output, wasm_bytes)?;
            println!("Successfully compiled {} to {}", input, output);
        }
        Commands::Run { input } => {
            let source = fs::read_to_string(&input)?;
            let wasm_bytes = compile_source(&source)?;
            runtime::execute(&wasm_bytes)?;
        }
    }

    Ok(())
}

fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = parser::parse_module(source)?;
    let program = semantic::lower_module(module);
    backend_wasm::compile(&program)
}
