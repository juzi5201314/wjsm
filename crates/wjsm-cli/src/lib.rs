//! wjsm CLI - AOT JavaScript/TypeScript to WebAssembly compiler
//!
//! Exit codes:
//! - 0: success
//! - 1: compile error (parse/lower/compile failure)
//! - 2: runtime error (WASM execution failure)
//! - 3: usage error (invalid arguments)

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use wjsm_backend_wasm as backend_wasm;
use wjsm_ir::Program;
use wjsm_parser as parser;
use wjsm_runtime as runtime;
use wjsm_semantic as semantic;

mod cli_args;
mod ir_output;

use cli_args::*;
use ir_output::{print_ir, print_ir_dot, print_stats};

// ============================================================================
// Exit Codes
// ============================================================================

const EXIT_SUCCESS: u8 = 0;
const EXIT_COMPILE_ERROR: u8 = 1;
const EXIT_RUNTIME_ERROR: u8 = 2;
const EXIT_USAGE_ERROR: u8 = 3;

// ============================================================================
// Runtime bridge (sync CLI -> async Store)
// ============================================================================

fn block_on_wasm_execute(wasm: &[u8]) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create Tokio runtime for WASM execution")?
        .block_on(runtime::execute(wasm))
}

// ============================================================================
// Pipeline Types
// ============================================================================

pub(crate) struct PipelineResult {
    pub(crate) ast: Option<swc_core::ecma::ast::Module>,
    pub(crate) program: Option<Program>,
    pub(crate) wasm: Option<Vec<u8>>,
    pub(crate) timings: PipelineTimings,
}

#[derive(Default)]
struct PipelineTimings {
    parse_us: u64,
    lower_us: u64,
    compile_us: u64,
    execute_us: u64,
}

impl PipelineTimings {
    fn print(&self, verbose: u8) {
        let exec = if self.execute_us > 0 {
            if verbose >= 1 {
                format!(", execute={}µs", self.execute_us)
            } else {
                format!(", execute={}ms", self.execute_us / 1000)
            }
        } else {
            String::new()
        };
        if verbose >= 1 {
            eprintln!(
                "Timing: parse={}µs, lower={}µs, compile={}µs{}",
                self.parse_us, self.lower_us, self.compile_us, exec
            );
        } else {
            eprintln!(
                "Timing: parse={}ms, lower={}ms, compile={}ms{}",
                self.parse_us / 1000,
                self.lower_us / 1000,
                self.compile_us / 1000,
                exec
            );
        }
    }
}

// ============================================================================
// Entry Points
// ============================================================================

pub fn main_entry() -> ExitCode {
    // Install embedded startup snapshot + support cwasm at CLI startup.
    // Both are produced at `cargo build` time via wjsm-runtime-snapshot/
    // wjsm-runtime-support build.rs and `include_bytes!`'d into the binary.
    if let Some(bytes) = wjsm_runtime_snapshot::EMBEDDED_STARTUP_SNAPSHOT {
        wjsm_runtime::install_embedded_startup_snapshot(bytes);
    }
    if let Some(bytes) = wjsm_runtime_support::EMBEDDED_SUPPORT_CWASM {
        wjsm_runtime::install_embedded_support_cwasm(bytes);
    }

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            e.print().ok();
            let code = match e.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    EXIT_SUCCESS
                }
                _ => EXIT_USAGE_ERROR,
            };
            return ExitCode::from(code);
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
            script,
            ref eval,
        } => {
            if let Some(code) = eval {
                cmd_run_eval(&cli, code, script)
            } else if let Some(input) = input {
                if watch {
                    cmd_run_watch(&cli, input, root.as_deref(), script)
                } else {
                    cmd_run(&cli, input, root.as_deref(), script)
                }
            } else {
                bail!("Either an input file or -e <code> is required");
            }
        }

        Commands::Check { ref input, ref root } => cmd_check(&cli, input, root.as_deref()),

        Commands::Eval { ref code } => cmd_eval(&cli, code),

        Commands::DumpIr {
            ref input,
            format,
            ref root,
        } => cmd_dump_ir(&cli, input, format, root.as_deref()),

        Commands::DumpAst { ref input, ref root } => cmd_dump_ast(&cli, input, root.as_deref()),

        Commands::DumpWat {
            ref input,
            ref root,
        } => cmd_dump_wat(&cli, input, root.as_deref()),

        Commands::Fmt { ref input, write } => cmd_fmt(input, write),

        Commands::Validate { ref input } => cmd_validate(input),

        Commands::Size { ref input } => cmd_size(input),

        Commands::Disasm { ref input } => cmd_disasm(input),

        Commands::Init { ref path, force } => cmd_init(path, force),
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
        Some(ColorChoice::Auto) | None => resolve_auto_colors(),
    };

    colored::control::set_override(use_colors);
}

/// 自动颜色：尊重 NO_COLOR / CLICOLOR_FORCE，并检测 stdout、stderr 是否为 TTY。
fn resolve_auto_colors() -> bool {
    if let Ok(v) = std::env::var("CLICOLOR_FORCE") {
        if !v.is_empty() && v != "0" {
            return true;
        }
    }
    if let Ok(v) = std::env::var("NO_COLOR") {
        if !v.is_empty() {
            return false;
        }
    }
    io::stdout().is_terminal() || io::stderr().is_terminal()
}

fn cmd_build(
    cli: &Cli,
    input: &str,
    output: &str,
    stage: Option<Stage>,
    root: Option<&str>,
) -> Result<ExitCode> {
    let stage = stage.unwrap_or(Stage::Compile);

    if matches!(stage, Stage::Parse | Stage::Lower) && output != "out.wasm" {
        bail!("`-o` / `--output` cannot be used with `--stage parse` or `--stage lower` (output goes to stdout)");
    }

    match stage {
        Stage::Parse | Stage::Lower => {
            let result = if input == "-" {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                run_pipeline(
                    &source,
                    None,
                    stage,
                    cli.verbose,
                    cli.time,
                    cli.target,
                    false,
                )?
            } else {
                run_file_input_pipeline(input, root, stage, cli)?
            };

            if matches!(stage, Stage::Parse) {
                if let Some(ast) = &result.ast {
                    let json = serde_json::to_string_pretty(ast)?;
                    println!("{}", json);
                }
            } else if let Some(program) = &result.program {
                print_ir(program);
            }
        }
        Stage::Compile => {
            if output == "-" && io::stdout().is_terminal() {
                bail!("refusing to write binary WASM to a terminal; redirect stdout to a file or use `-o <path>`");
            }

            if output != "-" && output == "out.wasm" && Path::new(output).exists() {
                eprintln!(
                    "warning: '{}' already exists and will be overwritten (use `-o` to choose another path)",
                    output
                );
            }

            let wasm = if input == "-" {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                compile_source(&source, None, cli.target, false)?
            } else {
                compile_from_file_input(Path::new(input), root, cli.target, false)?
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
        Stage::Execute => {
            if output == "-" && io::stdout().is_terminal() {
                bail!("refusing to write binary WASM to a terminal; redirect stdout to a file or use `-o <path>`");
            }

            let result = if input == "-" {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                compile_source_to_pipeline_result(&source, None, cli.target, false, cli.verbose >= 1)?
            } else {
                compile_file_input_to_pipeline_result(
                    Path::new(input),
                    root,
                    cli.target,
                    false,
                    cli.verbose >= 1,
                )?
            };

            let wasm = result
                .wasm
                .as_ref()
                .context("compile stage produced no WASM")?;

            if output == "-" {
                io::stdout().write_all(wasm)?;
            } else {
                fs::write(output, wasm)?;
                if cli.verbose >= 1 {
                    eprintln!("Wrote {} bytes to {}", wasm.len(), output);
                }
            }

            return run_compile_then_execute(cli, result);
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_run(cli: &Cli, input: &str, root: Option<&str>, script: bool) -> Result<ExitCode> {
    let verbose_compile = cli.verbose >= 1;
    let result = if input == "-" {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        compile_source_to_pipeline_result(&source, None, cli.target, script, verbose_compile)?
    } else {
        compile_file_input_to_pipeline_result(Path::new(input), root, cli.target, script, verbose_compile)?
    };

    run_compile_then_execute(cli, result)
}

fn cmd_run_eval(cli: &Cli, code: &str, script: bool) -> Result<ExitCode> {
    let result = compile_source_to_pipeline_result(code, None, cli.target, script, cli.verbose >= 1)?;
    run_compile_then_execute(cli, result)
}

fn cmd_run_watch(cli: &Cli, input: &str, root: Option<&str>, script: bool) -> Result<ExitCode> {
    use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::{Duration, Instant};

    const WATCH_DEBOUNCE: Duration = Duration::from_millis(200);

    fn watch_event_triggers_rebuild(kind: &EventKind) -> bool {
        match kind {
            EventKind::Modify(_) => true,
            EventKind::Create(_) => true,
            EventKind::Remove(_) => true,
            EventKind::Any => true,
            EventKind::Access(_) => false,
            EventKind::Other => true,
        }
    }

    let input_path = PathBuf::from(input);
    if !input_path.exists() {
        bail!("Input file '{}' does not exist", input);
    }

    let watch_target = root
        .map(PathBuf::from)
        .unwrap_or_else(|| input_path.clone());
    let watch_mode = if root.is_some() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    eprintln!("Watching {} for changes...", watch_target.display());
    let mut last_exit = match cmd_run(cli, input, root, script) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Initial run failed: {:#}", e);
            ExitCode::from(EXIT_COMPILE_ERROR)
        }
    };

    // Set up watcher
    let (tx, rx) = channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res
                && watch_event_triggers_rebuild(&event.kind)
            {
                let _ = tx.send(());
            }
        },
        Config::default(),
    )?;

    watcher.watch(&watch_target, watch_mode)?;

    let mut pending_rebuild = false;
    let mut debounce_deadline: Option<Instant> = None;

    loop {
        let wait_for = debounce_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(86400));

        match rx.recv_timeout(wait_for) {
            Ok(()) => {
                pending_rebuild = true;
                debounce_deadline = Some(Instant::now() + WATCH_DEBOUNCE);
            }
            Err(RecvTimeoutError::Timeout) => {
                if pending_rebuild {
                    eprintln!("\n--- File changed, re-running ---");
                    last_exit = match cmd_run(cli, input, root, script) {
                        Ok(code) => code,
                        Err(e) => {
                            eprintln!("Error: {:#}", e);
                            ExitCode::from(EXIT_COMPILE_ERROR)
                        }
                    };
                    pending_rebuild = false;
                    debounce_deadline = None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(last_exit)
}

fn cmd_check(cli: &Cli, input: &str, root: Option<&str>) -> Result<ExitCode> {
    let result = if input == "-" {
        let (source, filename) = read_input_for_parse(input)?;
        run_pipeline(
            &source,
            filename.as_deref(),
            Stage::Lower,
            cli.verbose,
            cli.time,
            cli.target,
            false,
        )?
    } else {
        run_file_input_pipeline(input, root, Stage::Lower, cli)?
    };

    if cli.verbose >= 1 {
        eprintln!("✓ No errors found");
    }

    if cli.stats {
        print_stats(&result);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_eval(cli: &Cli, code: &str) -> Result<ExitCode> {
    let wrapped = format!("console.log(({code}))");
    cmd_run_eval(cli, &wrapped, false)
}

fn cmd_dump_ir(
    cli: &Cli,
    input: &str,
    format: DumpFormat,
    root: Option<&str>,
) -> Result<ExitCode> {
    let result = if input == "-" {
        let (source, filename) = read_input_for_parse(input)?;
        run_pipeline(
            &source,
            filename.as_deref(),
            Stage::Lower,
            cli.verbose,
            cli.time,
            cli.target,
            false,
        )?
    } else {
        run_file_input_pipeline(input, root, Stage::Lower, cli)?
    };

    if let Some(program) = &result.program {
        match format {
            DumpFormat::Text => print_ir(program),
            DumpFormat::Dot => print_ir_dot(program),
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_ast(cli: &Cli, input: &str, root: Option<&str>) -> Result<ExitCode> {
    let result = if input == "-" {
        let (source, filename) = read_input_for_parse(input)?;
        run_pipeline(
            &source,
            filename.as_deref(),
            Stage::Parse,
            cli.verbose,
            cli.time,
            cli.target,
            false,
        )?
    } else {
        run_file_input_pipeline(input, root, Stage::Parse, cli)?
    };

    if let Some(ast) = &result.ast {
        let json = serde_json::to_string_pretty(ast)?;
        println!("{}", json);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_wat(cli: &Cli, input: &str, root: Option<&str>) -> Result<ExitCode> {
    let result = if input == "-" {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        run_pipeline(
            &source,
            None,
            Stage::Compile,
            cli.verbose,
            cli.time,
            cli.target,
            false,
        )?
    } else {
        let pipeline = compile_file_input_to_pipeline_result(
            Path::new(input),
            root,
            cli.target,
            false,
            cli.verbose >= 1,
        )?;
        if cli.time {
            pipeline.timings.print(cli.verbose);
        }
        pipeline
    };

    if cli.stats {
        print_stats(&result);
    }

    if let Some(wasm) = &result.wasm {
        let wat = wasmprinter::print_bytes(wasm)?;
        println!("{}", wat);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_fmt(input: &str, write: bool) -> Result<ExitCode> {
    let source = fs::read_to_string(input)?;

    let module = parser::parse_module_with_filename(&source, input)?;
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
        let payload = payload?;
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
    if code_size > 0 {
        sizes.push(("Code", code_size));
    }

    println!("WASM Size Breakdown for {}", input);
    println!("{}", "─".repeat(50));
    println!("{:<15} {:>10} {:>10}", "Section", "Bytes", "% Total");
    println!("{}", "─".repeat(50));

    let total: usize = sizes.iter().map(|(_, s)| s).sum();
    for (name, size) in &sizes {
        let pct = if total == 0 {
            0.0
        } else {
            (*size as f64 / total as f64) * 100.0
        };
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

fn cmd_init(path: &str, force: bool) -> Result<ExitCode> {
    let dir = PathBuf::from(path);
    let name = dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid path"))?
        .to_string_lossy();

    fs::create_dir_all(&dir)?;

    let main_path = dir.join("main.js");
    let package_path = dir.join("package.json");

    for file_path in [&main_path, &package_path] {
        if file_path.exists() && !force {
            bail!(
                "'{}' already exists. Use --force to overwrite.",
                file_path.display()
            );
        }
    }

    let main_js = format!(
        r#"// {} - wjsm project
console.log("Hello from {}!");
"#,
        name, name
    );
    fs::write(&main_path, main_js)?;

    let package_json = serde_json::json!({
        "name": name,
        "version": "0.1.0",
        "type": "module",
    });
    fs::write(
        &package_path,
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
            && output.status.success()
        {
            let hash = String::from_utf8_lossy(&output.stdout);
            println!("  Git: {}", hash.trim());
        }

        // Features (derived from Cargo.toml dependencies)
        println!("  Features: serde, wasmprinter, wasmparser, notify, regex");
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

// ============================================================================
// Pipeline Implementation
// ============================================================================

fn lower_parsed_module(
    source: &str,
    filename: Option<&str>,
    module: swc_core::ecma::ast::Module,
    script: bool,
) -> Result<Program> {
    let display_name = filename
        .map(str::to_string)
        .unwrap_or_else(|| if script { "input.js".into() } else { "input.ts".into() });
    semantic::lower_module_with_source(
        module,
        script,
        Some(std::sync::Arc::from(source)),
        display_name,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn run_pipeline(
    source: &str,
    filename: Option<&str>,
    stop_at: Stage,
    verbose: u8,
    time: bool,
    target: Target,
    script: bool,
) -> Result<PipelineResult> {
    let mut result = PipelineResult {
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
    let module = if script {
        parser::parse_script_as_module(source)?
    } else if let Some(filename) = filename {
        parser::parse_module_with_filename(source, filename)?
    } else {
        parser::parse_module(source)?
    };
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
    let program = lower_parsed_module(source, filename, result.ast.take().unwrap(), script)?;
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

/// 文件输入走 compile plan（含 `--root` bundling），在指定 stage 停止。
fn run_file_input_pipeline(
    input: &str,
    root: Option<&str>,
    stop_at: Stage,
    cli: &Cli,
) -> Result<PipelineResult> {
    let plan = build_compile_plan(Path::new(input), root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            if cli.verbose >= 1 {
                eprintln!("Bundling modules...");
            }
            let start = Instant::now();
            let mut result = PipelineResult {
                ast: None,
                program: None,
                wasm: None,
                timings: PipelineTimings::default(),
            };
            match stop_at {
                Stage::Parse => {
                    let ast = wjsm_module::parse_entry_ast(&entry, &root)?;
                    result.timings.parse_us = start.elapsed().as_micros() as u64;
                    result.ast = Some(ast);
                }
                Stage::Lower => {
                    let program = wjsm_module::lower_bundle(&entry, &root)?;
                    result.timings.lower_us = start.elapsed().as_micros() as u64;
                    result.program = Some(program);
                }
                Stage::Compile | Stage::Execute => {
                    let wasm = wjsm_module::bundle(&entry, &root)?;
                    result.timings.compile_us = start.elapsed().as_micros() as u64;
                    result.wasm = Some(wasm);
                }
            }
            if cli.time {
                result.timings.print(cli.verbose);
            }
            Ok(result)
        }
        CompilePlan::SingleSource { source, filename } => run_pipeline(
            &source,
            Some(filename.as_str()),
            stop_at,
            cli.verbose,
            cli.time,
            cli.target,
            false,
        ),
    }
}

// ============================================================================
// Input/Output Helpers

/// 将路径转为字符串（非 UTF-8 字节用 U+FFFD 替换，与 `to_string_lossy` 一致，但保留 `Path` 入参避免 CLI 层过早丢失路径类型）
fn path_to_lossy_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 读取源码文件：按字节读取再 UTF-8 解码，避免对路径本身使用 lossy 转换
fn read_source_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("Failed to read '{}'", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ============================================================================

fn read_input(input: &str) -> Result<String> {
    if input == "-" {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("Failed to read from stdin")?;
        Ok(source)
    } else {
        read_source_file(Path::new(input))
    }
}

/// 读取源码，并在输入为文件路径时返回用于选择解析语法的路径字符串。
fn read_input_for_parse(input: &str) -> Result<(String, Option<String>)> {
    let source = read_input(input)?;
    let filename = if input == "-" {
        None
    } else {
        Some(input.to_string())
    };
    Ok((source, filename))
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
    SingleSource { source: String, filename: String },
}

fn build_compile_plan(input: &Path, root: Option<&str>) -> Result<CompilePlan> {
    if let Some(root_path) = root {
        return bundle_plan_from_root(input.to_path_buf(), PathBuf::from(root_path));
    }

    let source = read_source_file(input)?;
    let filename = path_to_lossy_string(input);
    let module = parser::parse_module_with_filename(&source, &filename)?;
    let is_esm = wjsm_module::is_es_module(&module);
    let is_cjs = wjsm_module::is_commonjs_module(&module);

    if !is_esm && !is_cjs {
        return Ok(CompilePlan::SingleSource {
            source,
            filename: path_to_lossy_string(input),
        });
    }

    let canonical_input = input.canonicalize()?;
    let parent = canonical_input
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot infer module root from '{}'", input.display()))?;
    let file_name = canonical_input.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot infer module entry file name from '{}'",
            input.display()
        )
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
    let rel = canonical_input
        .strip_prefix(&canonical_root)
        .map_err(|_| anyhow::anyhow!("input file {:?} is not under root {:?}", input, root))?;
    let entry = format!("./{}", rel.to_string_lossy());
    Ok(CompilePlan::Bundle {
        entry,
        root: canonical_root,
    })
}

fn run_compile_then_execute(cli: &Cli, mut result: PipelineResult) -> Result<ExitCode> {
    let wasm = result
        .wasm
        .as_ref()
        .context("compile stage produced no WASM")?;

    if cli.stats {
        print_stats(&result);
    }

    let start = Instant::now();
    let exec_result = block_on_wasm_execute(wasm);
    result.timings.execute_us = start.elapsed().as_micros() as u64;

    if cli.time {
        result.timings.print(cli.verbose);
    }

    if let Err(e) = exec_result {
        eprintln!("Runtime error: {:#}", e);
        return Ok(ExitCode::from(EXIT_RUNTIME_ERROR));
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn compile_source_to_pipeline_result(
    source: &str,
    filename: Option<&str>,
    target: Target,
    script: bool,
    verbose: bool,
) -> Result<PipelineResult> {
    let mut result = PipelineResult {
        ast: None,
        program: None,
        wasm: None,
        timings: PipelineTimings::default(),
    };

    if verbose {
        eprintln!("Parsing...");
    }
    let start = Instant::now();
    let module = if script {
        parser::parse_script_as_module(source)?
    } else if let Some(filename) = filename {
        parser::parse_module_with_filename(source, filename)?
    } else {
        parser::parse_module(source)?
    };
    result.timings.parse_us = start.elapsed().as_micros() as u64;
    result.ast = Some(module);

    if verbose {
        eprintln!("Lowering to IR...");
    }
    let start = Instant::now();
    let program = lower_parsed_module(source, filename, result.ast.take().unwrap(), script)?;
    result.timings.lower_us = start.elapsed().as_micros() as u64;
    result.program = Some(program);

    if verbose {
        eprintln!("Compiling to WASM...");
    }
    let start = Instant::now();
    let wasm = match target {
        Target::Wasm => backend_wasm::compile(result.program.as_ref().unwrap())?,
        Target::Jit => bail!("JIT backend is not implemented yet"),
    };
    result.timings.compile_us = start.elapsed().as_micros() as u64;
    result.wasm = Some(wasm);

    Ok(result)
}
fn compile_file_input_to_pipeline_result(
    input: &Path,
    root: Option<&str>,
    target: Target,
    script: bool,
    verbose: bool,
) -> Result<PipelineResult> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            if verbose {
                eprintln!("Bundling modules...");
            }
            let start = Instant::now();
            let wasm = wjsm_module::bundle(&entry, &root)?;
            let mut result = PipelineResult {
                ast: None,
                program: None,
                wasm: None,
                timings: PipelineTimings::default(),
            };
            result.timings.compile_us = start.elapsed().as_micros() as u64;
            match target {
                Target::Wasm => result.wasm = Some(wasm),
                Target::Jit => bail!("JIT backend is not implemented yet"),
            }
            Ok(result)
        }
        CompilePlan::SingleSource { source, filename } => {
            compile_source_to_pipeline_result(&source, Some(filename.as_str()), target, script, verbose)
        }
    }
}
fn compile_from_file_input(
    input: &Path,
    root: Option<&str>,
    target: Target,
    script: bool,
) -> Result<Vec<u8>> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            let wasm = wjsm_module::bundle(&entry, &root)?;
            match target {
                Target::Wasm => Ok(wasm),
                Target::Jit => bail!("JIT backend is not implemented yet"),
            }
        }
        CompilePlan::SingleSource { source, filename } => {
            compile_source(&source, Some(filename.as_str()), target, script)
        }
    }
}

fn compile_source(
    source: &str,
    filename: Option<&str>,
    target: Target,
    script: bool,
) -> Result<Vec<u8>> {
    let module = if script {
        parser::parse_script_as_module(source)?
    } else if let Some(filename) = filename {
        parser::parse_module_with_filename(source, filename)?
    } else {
        parser::parse_module(source)?
    };
    let program = lower_parsed_module(source, filename, module, script)?;
    match target {
        Target::Wasm => backend_wasm::compile(&program),
        Target::Jit => bail!("JIT backend is not implemented yet"),
    }
}

/// In-process 复现 `wjsm run <file>` 的可观测行为（stdout / stderr / exit_code），
/// 供 E2E fixture 测试在测试进程内直接调用，免去每个 fixture spawn 一个 wjsm 子进程
/// （省一层进程 + 510MB ELF 加载）。
///
/// 退出码 / stderr 契约必须与 `main_entry` + `cmd_run` 逐字一致：
/// - 编译错（parse/lower/bundle/compile）→ 退出码 1，stderr = `Error: {e:#}\n`
/// - 运行时错（WASM 执行失败）→ 退出码 2，stderr = `Runtime error: {e:#}\n`
/// - 成功 → 退出码 0，stdout = 程序输出，stderr 空
///
/// 偏离 CLI 的唯一点：stdout 写入返回的 buffer 而非真实 fd（测试需捕获）。
/// 与 CLI 默认对齐：target=Wasm、script=false、root 由文件路径推断。
pub fn run_file_in_process(input: &Path) -> (i32, Vec<u8>, Vec<u8>) {
    // 与 main_entry 一致：安装 embedded snapshot + support cwasm（OnceLock，幂等）。
    if let Some(bytes) = wjsm_runtime_snapshot::EMBEDDED_STARTUP_SNAPSHOT {
        wjsm_runtime::install_embedded_startup_snapshot(bytes);
    }
    if let Some(bytes) = wjsm_runtime_support::EMBEDDED_SUPPORT_CWASM {
        wjsm_runtime::install_embedded_support_cwasm(bytes);
    }

    let wasm = match compile_from_file_input(input, None, Target::Wasm, false) {
        Ok(w) => w,
        Err(e) => {
            return (
                EXIT_COMPILE_ERROR as i32,
                Vec::new(),
                format!("Error: {e:#}\n").into_bytes(),
            );
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            return (
                EXIT_COMPILE_ERROR as i32,
                Vec::new(),
                format!("Error: {e:#}\n").into_bytes(),
            );
        }
    };

    let mut stdout: Vec<u8> = Vec::new();
    match rt.block_on(runtime::execute_with_writer(&wasm, &mut stdout)) {
        Ok(_) => (EXIT_SUCCESS as i32, stdout, Vec::new()),
        Err(e) => (
            EXIT_RUNTIME_ERROR as i32,
            stdout,
            format!("Runtime error: {e:#}\n").into_bytes(),
        ),
    }
}
