//! wjsm CLI - AOT JavaScript/TypeScript to WebAssembly compiler
//!
//! Exit codes:
//! - 0: success
//! - 1: compile error (parse/lower/compile failure)
//! - 2: runtime error (WASM execution failure)
//! - 3: usage error (invalid arguments)

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Instant;
use wjsm_backend_wasm as backend_wasm;
use wjsm_ir::Program;
use wjsm_parser as parser;
use wjsm_runtime as runtime;
use wjsm_semantic as semantic;

// ============================================================================
// Exit Codes
// ============================================================================

const EXIT_SUCCESS: u8 = 0;
const EXIT_COMPILE_ERROR: u8 = 1;
const EXIT_RUNTIME_ERROR: u8 = 2;
const EXIT_USAGE_ERROR: u8 = 3;

// ============================================================================
// CLI Structure
// ============================================================================

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbose output (-v shows progress, -vv shows details)
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Show timing information for each pipeline stage
    #[arg(long, global = true)]
    time: bool,

    /// Show statistics after build (constants, functions, blocks, instructions, WASM size)
    #[arg(long, global = true)]
    stats: bool,

    /// Color output control (auto/always/never). Also respects NO_COLOR env var.
    #[arg(long, value_name = "WHEN", global = true)]
    color: Option<ColorChoice>,

    /// Target backend (wasm or jit)
    #[arg(long, default_value = "wasm", global = true)]
    target: Target,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ColorChoice {
    /// Auto-detect based on terminal
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Target {
    Wasm,
    Jit,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DumpFormat {
    Text,
    Dot,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Stage {
    /// Parse and print AST JSON
    Parse,
    /// Lower to IR and print
    Lower,
    /// Compile to WASM and write output
    Compile,
    /// Compile and execute (default for run)
    Execute,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a JS/TS file to WebAssembly
    Build {
        /// The input file to compile, or - for stdin
        input: String,

        /// The output .wasm file, or - for stdout
        #[arg(short, long, default_value = "out.wasm")]
        output: String,

        /// Stop at a specific pipeline stage
        #[arg(long, value_name = "STAGE")]
        stage: Option<Stage>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,
    },

    /// Run a JS/TS file directly
    Run {
        /// The input file to run, or - for stdin
        input: String,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

        /// Watch for changes and re-run
        #[arg(short, long)]
        watch: bool,
    },

    /// Parse and check a JS/TS file for errors (no output)
    Check {
        /// The input file to check, or - for stdin
        input: String,
    },

    /// Evaluate a JS expression and print the result
    Eval {
        /// The JS expression to evaluate
        code: String,
    },

    /// Dump IR for a JS/TS file
    DumpIr {
        /// The input file, or - for stdin
        input: String,

        /// Output format (text or dot for Graphviz)
        #[arg(long, default_value = "text")]
        format: DumpFormat,
    },

    /// Dump SWC AST as JSON for a JS/TS file
    DumpAst {
        /// The input file, or - for stdin
        input: String,
    },

    /// Dump WAT (WebAssembly Text) for a compiled JS/TS file
    DumpWat {
        /// The input file, or - for stdin
        input: String,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,
    },

    /// Format a JS/TS file using SWC codegen
    Fmt {
        /// The input file to format
        input: String,

        /// Write formatted output back to the file
        #[arg(short, long)]
        write: bool,
    },

    /// Validate a .wasm file
    Validate {
        /// The .wasm file to validate
        input: String,
    },

    /// Show size breakdown of WASM sections
    Size {
        /// The .wasm file to analyze
        input: String,
    },

    /// Disassemble a .wasm file with detailed output
    Disasm {
        /// The .wasm file to disassemble
        input: String,
    },

    /// Create a new wjsm project
    Init {
        /// The project directory to create
        path: String,
    },

    /// Show extended version information
    Version {
        /// Show extended version info
        #[arg(long)]
        extended: bool,
    },
}

// ============================================================================
// Pipeline Types
// ============================================================================

struct PipelineResult {
    #[allow(dead_code)]
    source: Option<String>,
    ast: Option<swc_core::ecma::ast::Module>,
    program: Option<Program>,
    wasm: Option<Vec<u8>>,
    timings: PipelineTimings,
}

struct PipelineTimings {
    parse_us: u64,
    lower_us: u64,
    compile_us: u64,
}

impl Default for PipelineTimings {
    fn default() -> Self {
        Self {
            parse_us: 0,
            lower_us: 0,
            compile_us: 0,
        }
    }
}

impl PipelineTimings {
    fn print(&self, verbose: u8) {
        if verbose >= 1 {
            eprintln!(
                "Timing: parse={}µs, lower={}µs, compile={}µs",
                self.parse_us, self.lower_us, self.compile_us
            );
        } else {
            eprintln!(
                "Timing: parse={}ms, lower={}ms, compile={}ms",
                self.parse_us / 1000,
                self.lower_us / 1000,
                self.compile_us / 1000
            );
        }
    }
}

// ============================================================================
// Entry Points
// ============================================================================

pub fn main_entry() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            e.print().ok();
            return ExitCode::from(EXIT_USAGE_ERROR);
        }
    };

    match execute(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {:#}", e);
            ExitCode::from(EXIT_COMPILE_ERROR)
        }
    }
}

pub fn execute(cli: Cli) -> Result<ExitCode> {
    // Handle color configuration
    setup_colors(cli.color);

    match cli.command {
        Commands::Build {
            ref input,
            ref output,
            stage,
            ref root,
        } => cmd_build(&cli, input, output, stage, root.as_deref()),

        Commands::Run {
            ref input,
            ref root,
            watch,
        } => {
            if watch {
                cmd_run_watch(&cli, input, root.as_deref())
            } else {
                cmd_run(&cli, input, root.as_deref())
            }
        }

        Commands::Check { ref input } => cmd_check(&cli, input),

        Commands::Eval { ref code } => cmd_eval(&cli, code),

        Commands::DumpIr {
            ref input,
            format,
        } => cmd_dump_ir(&cli, input, format),

        Commands::DumpAst { ref input } => cmd_dump_ast(&cli, input),

        Commands::DumpWat {
            ref input,
            ref root,
        } => cmd_dump_wat(&cli, input, root.as_deref()),

        Commands::Fmt {
            ref input,
            write,
        } => cmd_fmt(input, write),

        Commands::Validate { ref input } => cmd_validate(input),

        Commands::Size { ref input } => cmd_size(input),

        Commands::Disasm { ref input } => cmd_disasm(input),

        Commands::Init { ref path } => cmd_init(path),
        Commands::Version { extended } => cmd_version(extended),
    }
}

// ============================================================================
// Color Setup
// ============================================================================

fn setup_colors(choice: Option<ColorChoice>) {
    let use_colors = match choice {
        Some(ColorChoice::Always) => true,
        Some(ColorChoice::Never) => false,
        Some(ColorChoice::Auto) | None => {
            // Check NO_COLOR env var
            if std::env::var("NO_COLOR").is_ok() {
                false
            } else {
                // Auto-detect based on stdout
                io::stdout().is_terminal()
            }
        }
    };

    colored::control::set_override(use_colors);
}

// ============================================================================
// Command Implementations
// ============================================================================

fn cmd_build(
    cli: &Cli,
    input: &str,
    output: &str,
    stage: Option<Stage>,
    root: Option<&str>,
) -> Result<ExitCode> {
    let stage = stage.unwrap_or(Stage::Compile);

    match stage {
        Stage::Parse | Stage::Lower => {
            let source = read_input(input)?;
            let result = run_pipeline(&source, stage, cli.verbose, cli.time, cli.target)?;

            if matches!(stage, Stage::Parse) {
                if let Some(ast) = &result.ast {
                    let json = serde_json::to_string_pretty(ast)?;
                    println!("{}", json);
                }
            } else {
                if let Some(program) = &result.program {
                    print_ir(program);
                }
            }
        }
        Stage::Compile | Stage::Execute => {
            let wasm = if input == "-" {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                compile_source(&source, cli.target)?
            } else {
                compile_from_file_input(input, root, cli.target)?
            };

            if output == "-" {
                io::stdout().write_all(&wasm)?;
            } else {
                fs::write(output, &wasm)?;
                if cli.verbose >= 1 {
                    eprintln!("Wrote {} bytes to {}", wasm.len(), output);
                }
            }

            if cli.stats {
                eprintln!("Output: {} bytes", wasm.len());
            }
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_run(cli: &Cli, input: &str, root: Option<&str>) -> Result<ExitCode> {
    let wasm = if input == "-" {
        // stdin can't be a module
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        compile_source(&source, cli.target)?
    } else {
        compile_from_file_input(input, root, cli.target)?
    };

    if let Err(e) = runtime::execute(&wasm) {
        eprintln!("Runtime error: {:#}", e);
        return Ok(ExitCode::from(EXIT_RUNTIME_ERROR));
    }

    if cli.stats {
        // TODO: print stats for module compilation
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_run_watch(cli: &Cli, input: &str, root: Option<&str>) -> Result<ExitCode> {
    use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let input_path = PathBuf::from(input);
    if !input_path.exists() {
        bail!("Input file '{}' does not exist", input);
    }

    // Determine watch target: root directory if provided, otherwise just the input file
    let watch_target = root.map(PathBuf::from).unwrap_or_else(|| input_path.clone());
    let watch_mode = if root.is_some() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };

    // Initial run
    eprintln!("Watching {} for changes...", watch_target.display());
    let mut last_exit = match cmd_run(cli, input, root) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Initial run failed: {:#}", e);
            ExitCode::from(EXIT_COMPILE_ERROR)
        }
    };

    // Set up watcher
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_)) {
                    let _ = tx.send(event);
                }
            }
        },
        Config::default(),
    )?;

    watcher.watch(&watch_target, watch_mode)?;

    // Wait for changes
    while rx.recv().is_ok() {
        eprintln!("\n--- File changed, re-running ---");
        last_exit = match cmd_run(cli, input, root) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("Error: {:#}", e);
                ExitCode::from(EXIT_COMPILE_ERROR)
            }
        };
    }

    Ok(last_exit)
}

fn cmd_check(cli: &Cli, input: &str) -> Result<ExitCode> {
    let source = read_input(input)?;

    let result = run_pipeline(&source, Stage::Lower, cli.verbose, cli.time, cli.target)?;

    if cli.verbose >= 1 {
        eprintln!("✓ No errors found");
    }

    if cli.stats {
        print_stats(&result);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_eval(cli: &Cli, code: &str) -> Result<ExitCode> {
    // Wrap the expression in a module that logs it
    let source = format!("console.log({});", code);

    let result = run_pipeline(&source, Stage::Execute, cli.verbose, cli.time, cli.target)?;

    if let Some(wasm) = &result.wasm {
        if let Err(e) = runtime::execute(wasm) {
            eprintln!("Runtime error: {:#}", e);
            return Ok(ExitCode::from(EXIT_RUNTIME_ERROR));
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_ir(cli: &Cli, input: &str, format: DumpFormat) -> Result<ExitCode> {
    let source = read_input(input)?;

    let result = run_pipeline(&source, Stage::Lower, cli.verbose, cli.time, cli.target)?;

    if let Some(program) = &result.program {
        match format {
            DumpFormat::Text => print_ir(program),
            DumpFormat::Dot => print_ir_dot(program),
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_ast(cli: &Cli, input: &str) -> Result<ExitCode> {
    let source = read_input(input)?;

    let result = run_pipeline(&source, Stage::Parse, cli.verbose, cli.time, cli.target)?;

    if let Some(ast) = &result.ast {
        let json = serde_json::to_string_pretty(ast)?;
        println!("{}", json);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_wat(cli: &Cli, input: &str, _root: Option<&str>) -> Result<ExitCode> {
    let source = read_input(input)?;

    let result = run_pipeline(&source, Stage::Compile, cli.verbose, cli.time, cli.target)?;

    if let Some(wasm) = &result.wasm {
        let wat = wasmprinter::print_bytes(wasm)?;
        println!("{}", wat);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_fmt(input: &str, write: bool) -> Result<ExitCode> {
    let source = fs::read_to_string(input)?;

    // Parse
    let module = parser::parse_module(&source)?;

    // Use SWC codegen to emit formatted code
    let formatted = emit_js(&module)?;

    if write {
        fs::write(input, &formatted)?;
        eprintln!("Formatted {}", input);
    } else {
        println!("{}", formatted);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_validate(input: &str) -> Result<ExitCode> {
    let bytes = fs::read(input)?;

    match wasmparser::validate(&bytes) {
        Ok(_) => {
            println!("✓ {} is valid WASM", input);
            Ok(ExitCode::from(EXIT_SUCCESS))
        }
        Err(e) => {
            println!("✗ {} is NOT valid WASM", input);
            eprintln!("Validation error: {}", e);
            Ok(ExitCode::from(EXIT_COMPILE_ERROR))
        }
    }
}

fn cmd_size(input: &str) -> Result<ExitCode> {
    let bytes = fs::read(input)?;

    let mut sizes: Vec<(&str, usize)> = Vec::new();
    let mut code_size: usize = 0;

    // Parse WASM sections
    let parser = wasmparser::Parser::new(0);

    for payload in parser.parse_all(&bytes) {
        if let Ok(payload) = payload {
            use wasmparser::Payload::*;
            let (name, size) = match payload {
                TypeSection(s) => ("Type", s.range().len()),
                ImportSection(s) => ("Import", s.range().len()),
                FunctionSection(s) => ("Function", s.range().len()),
                TableSection(s) => ("Table", s.range().len()),
                MemorySection(s) => ("Memory", s.range().len()),
                GlobalSection(s) => ("Global", s.range().len()),
                ExportSection(s) => ("Export", s.range().len()),
                StartSection { range, .. } => ("Start", range.len()),
                ElementSection(s) => ("Element", s.range().len()),
                CodeSectionEntry(s) => {
                    code_size += s.range().len();
                    continue;
                }
                DataSection(s) => ("Data", s.range().len()),
                CustomSection(s) => ("Custom", s.range().len()),
                _ => continue,
            };
            sizes.push((name, size));
        }
    }
    if code_size > 0 {
        sizes.push(("Code", code_size));
    }

    println!("WASM Size Breakdown for {}", input);
    println!("{}", "─".repeat(50));
    println!("{:<15} {:>10} {:>10}", "Section", "Bytes", "% Total");
    println!("{}", "─".repeat(50));

    let total: usize = sizes.iter().map(|(_, s)| s).sum();
    for (name, size) in &sizes {
        let pct = (*size as f64 / total as f64) * 100.0;
        println!("{:<15} {:>10} {:>9.1}%", name, size, pct);
    }

    println!("{}", "─".repeat(50));
    println!("{:<15} {:>10}", "Total", total);
    println!("{:<15} {:>10}", "File Size", bytes.len());

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_disasm(input: &str) -> Result<ExitCode> {
    let bytes = fs::read(input)?;

    // Use wasmprinter for detailed disassembly
    let disasm = wasmprinter::print_bytes(&bytes)?;
    println!("{}", disasm);

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_init(path: &str) -> Result<ExitCode> {
    let dir = PathBuf::from(path);
    let name = dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid path"))?
        .to_string_lossy();

    // Create directory
    fs::create_dir_all(&dir)?;

    // Create main.js
    let main_js = format!(
        r#"// {} - wjsm project
console.log("Hello from {}!");
"#,
        name, name
    );
    fs::write(dir.join("main.js"), main_js)?;

    // Create package.json (optional, for IDE support)
    let package_json = serde_json::json!({
        "name": name,
        "version": "0.1.0",
        "type": "module",
    });
    fs::write(
        dir.join("package.json"),
        serde_json::to_string_pretty(&package_json)?,
    )?;

    println!("Created project at {}", path);
    println!();
    println!("To run:");
    println!("  cd {}", path);
    println!("  wjsm run main.js");

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_version(verbose: bool) -> Result<ExitCode> {
    println!("wjsm {}", env!("CARGO_PKG_VERSION"));

    if verbose {
        println!("  Edition: 2024");

        // Try to get git hash
        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
        {
            if output.status.success() {
                let hash = String::from_utf8_lossy(&output.stdout);
                println!("  Git: {}", hash.trim());
            }
        }

        // Features (derived from Cargo.toml dependencies)
        println!("  Features: serde, wasmprinter, wasmparser, notify, regex");
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

// ============================================================================
// Pipeline Implementation
// ============================================================================

fn run_pipeline(
    source: &str,
    stop_at: Stage,
    verbose: u8,
    time: bool,
    target: Target,
) -> Result<PipelineResult> {
    let mut result = PipelineResult {
        source: Some(source.to_string()),
        ast: None,
        program: None,
        wasm: None,
        timings: PipelineTimings::default(),
    };

    // Parse
    if verbose >= 1 {
        eprintln!("Parsing...");
    }
    let start = Instant::now();
    let module = parser::parse_module(source)?;
    result.timings.parse_us = start.elapsed().as_micros() as u64;
    result.ast = Some(module);

    if matches!(stop_at, Stage::Parse) {
        return Ok(result);
    }

    // Lower
    if verbose >= 1 {
        eprintln!("Lowering to IR...");
    }
    let start = Instant::now();
    let program = semantic::lower_module(result.ast.take().unwrap())?;
    result.timings.lower_us = start.elapsed().as_micros() as u64;
    result.program = Some(program);

    if matches!(stop_at, Stage::Lower) {
        return Ok(result);
    }

    // Compile
    if verbose >= 1 {
        eprintln!("Compiling to WASM...");
    }
    let start = Instant::now();
    let wasm = match target {
        Target::Wasm => backend_wasm::compile(result.program.as_ref().unwrap())?,
        Target::Jit => {
            bail!("JIT backend is not implemented yet");
        }
    };
    result.timings.compile_us = start.elapsed().as_micros() as u64;
    result.wasm = Some(wasm);

    if time {
        result.timings.print(verbose);
    }

    Ok(result)
}

// ============================================================================
// Input/Output Helpers
// ============================================================================

fn read_input(input: &str) -> Result<String> {
    if input == "-" {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("Failed to read from stdin")?;
        Ok(source)
    } else {
        fs::read_to_string(input).with_context(|| format!("Failed to read '{}'", input))
    }
}

// ============================================================================
// IR Output
// ============================================================================

fn print_ir(program: &Program) {
    let text = program.dump_text();

    // Check if colors are enabled
    if colored::control::SHOULD_COLORIZE.should_colorize() {
        for line in text.lines() {
            let colored_line = colorize_ir_line(line);
            println!("{}", colored_line);
        }
    } else {
        println!("{}", text);
    }
}

fn colorize_ir_line(line: &str) -> String {
    static VALUE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static SCOPE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static CONST_RE: OnceLock<regex::Regex> = OnceLock::new();

    // Keywords in blue
    let keywords = [
        "module", "fn", "entry=", "bb", "return", "call", "const", "jump", "branch",
    ];

    let mut result = line.to_string();

    // Color keywords
    for kw in &keywords {
        result = result.replace(kw, &kw.blue().to_string());
    }

    // Color types (like "number", "string") in green
    result = result.replace("number(", &"number(".green().to_string());
    result = result.replace("string(", &"string(".green().to_string());

    // Color values (like %0, $0.x) in cyan
    if result.contains('%') {
        let re = VALUE_RE.get_or_init(|| regex::Regex::new(r"%\d+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                caps[0].cyan().to_string()
            })
            .to_string();
    }

    // Color scope-qualified names like $0.x in cyan
    if result.contains('$') {
        let re = SCOPE_RE.get_or_init(|| regex::Regex::new(r"\$\d+\.\w+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                caps[0].cyan().to_string()
            })
            .to_string();
    }

    // Color constants like c0, c1 in yellow
    if result.contains(" c") || result.starts_with('c') {
        let re = CONST_RE.get_or_init(|| regex::Regex::new(r"\bc\d+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                caps[0].yellow().to_string()
            })
            .to_string();
    }

    result
}

fn print_ir_dot(program: &Program) {
    println!("digraph IR {{");
    println!("  rankdir=TB;");
    println!("  node [shape=box];");
    println!();

    // For each function
    for func in program.functions() {
        println!("  subgraph cluster_{} {{", func.name());
        println!("    label=\"{}\";", func.name());
        println!("    style=rounded;");
        println!();

        // Create nodes for each basic block using actual block IDs
        for bb in func.blocks() {
            let bb_id = bb.id();
            let label = format!(
                "{}\\l{}",
                bb_id,
                bb.instructions()
                    .iter()
                    .map(|inst| format!("  {}", inst))
                    .collect::<Vec<_>>()
                    .join("\\l")
            );
            println!("    bb{} [label=\"{}\"];", bb_id.0, label);
        }

        // Create edges for control flow using actual block IDs
        for bb in func.blocks() {
            let bb_id = bb.id();
            use wjsm_ir::Terminator;
            match bb.terminator() {
                Terminator::Return { .. } => {
                    // No outgoing edges
                }
                Terminator::Jump { target } => {
                    println!("    bb{} -> bb{};", bb_id.0, target.0);
                }
                Terminator::Branch {
                    condition: _,
                    true_block,
                    false_block,
                } => {
                    println!("    bb{} -> bb{} [label=\"true\"];", bb_id.0, true_block.0);
                    println!("    bb{} -> bb{} [label=\"false\"];", bb_id.0, false_block.0);
                }
                Terminator::Switch {
                    value: _,
                    cases,
                    default_block,
                    exit_block,
                } => {
                    for case in cases {
                        println!("    bb{} -> bb{};", bb_id.0, case.target.0);
                    }
                    println!("    bb{} -> bb{} [label=\"default\"];", bb_id.0, default_block.0);
                    println!("    bb{} -> bb{} [label=\"exit\"];", bb_id.0, exit_block.0);
                }
                Terminator::Throw { .. } => {
                    // No outgoing edges
                }
                Terminator::Unreachable => {
                    // No outgoing edges
                }
            }
        }

        println!("  }}");
    }

    println!("}}");
}

fn print_stats(result: &PipelineResult) {
    eprintln!();
    eprintln!("=== Statistics ===");

    if let Some(program) = &result.program {
        let mut total_blocks = 0;
        let mut total_instructions = 0;

        for func in program.functions() {
            total_blocks += func.blocks().len();
            for bb in func.blocks() {
                total_instructions += bb.instructions().len();
            }
        }

        eprintln!("  Constants: {}", program.constants().len());
        eprintln!("  Functions: {}", program.functions().len());
        eprintln!("  Basic Blocks: {}", total_blocks);
        eprintln!("  Instructions: {}", total_instructions);
    }

    if let Some(wasm) = &result.wasm {
        eprintln!("  WASM Size: {} bytes", wasm.len());
    }
}

// ============================================================================
// SWC Codegen (for fmt command)
// ============================================================================

fn emit_js(module: &swc_core::ecma::ast::Module) -> Result<String> {
    use swc_core::common::SourceMap;
    use swc_core::common::sync::Lrc;
    use swc_core::ecma::codegen::{Config, Emitter, text_writer::JsWriter};

    let cm: Lrc<SourceMap> = Default::default();

    let mut buf = Vec::new();
    {
        let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
        let mut emitter = Emitter {
            cfg: Config::default(),
            cm,
            comments: None,
            wr: writer,
        };
        emitter.emit_module(module)?;
    }

    Ok(String::from_utf8(buf)?)
}

// ============================================================================
// Compile Plan (for module support)
// ============================================================================

enum CompilePlan {
    Bundle { entry: String, root: PathBuf },
    SingleSource(String),
}

fn build_compile_plan(input: &Path, root: Option<&str>) -> Result<CompilePlan> {
    if let Some(root_path) = root {
        return bundle_plan_from_root(input.to_path_buf(), PathBuf::from(root_path));
    }

    let source = fs::read_to_string(input)?;
    let module = parser::parse_module(&source)?;
    let is_esm = wjsm_module::is_es_module(&module);
    let is_cjs = wjsm_module::is_commonjs_module(&module);

    if !is_esm && !is_cjs {
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

fn bundle_plan_from_root(input: PathBuf, root: PathBuf) -> Result<CompilePlan> {
    let canonical_root = std::fs::canonicalize(&root)
        .map_err(|e| anyhow::anyhow!("cannot canonicalize root path {:?}: {}", root, e))?;
    let canonical_input = std::fs::canonicalize(&input)
        .map_err(|e| anyhow::anyhow!("cannot canonicalize input path {:?}: {}", input, e))?;
    let rel = canonical_input.strip_prefix(&canonical_root).map_err(|_| {
        anyhow::anyhow!("input file {:?} is not under root {:?}", input, root)
    })?;
    let entry = format!("./{}", rel.to_string_lossy());
    Ok(CompilePlan::Bundle {
        entry,
        root: canonical_root,
    })
}

fn compile_from_file_input(input: &str, root: Option<&str>, target: Target) -> Result<Vec<u8>> {
    let plan = build_compile_plan(Path::new(input), root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            let wasm = wjsm_module::bundle(&entry, &root)?;
            match target {
                Target::Wasm => Ok(wasm),
                Target::Jit => bail!("JIT backend is not implemented yet"),
            }
        }
        CompilePlan::SingleSource(source) => compile_source(&source, target),
    }
}

fn compile_source(source: &str, target: Target) -> Result<Vec<u8>> {
    let module = parser::parse_module(source)?;
    let program = semantic::lower_module(module)?;
    match target {
        Target::Wasm => backend_wasm::compile(&program),
        Target::Jit => bail!("JIT backend is not implemented yet"),
    }
}
