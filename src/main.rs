mod compiler;
mod runtime;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::fs;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a JS/TS file to WebAssembly
    Build {
        /// The input file to compile
        input: String,
        /// The output .wasm file (default: out.wasm)
        #[arg(short, long, default_value = "out.wasm")]
        output: String,
    },
    /// Run a JS/TS file directly
    Run {
        /// The input file to run
        input: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Build { input, output } => {
            let source = fs::read_to_string(input)?;
            let wasm_bytes = compiler::compile(&source)?;
            fs::write(output, wasm_bytes)?;
            println!("Successfully compiled {} to {}", input, output);
        }
        Commands::Run { input } => {
            let source = fs::read_to_string(input)?;
            let wasm_bytes = compiler::compile(&source)?;
            runtime::execute(&wasm_bytes)?;
        }
    }

    Ok(())
}
