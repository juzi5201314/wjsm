//! wjsm CLI - AOT JavaScript/TypeScript to WebAssembly compiler
//!
//! Exit codes:
//! - 0: success
//! - 1: compile error (parse/lower/compile failure)
//! - 2: runtime error (WASM execution failure)
//! - 3: usage error (invalid arguments)

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use std::ffi::{OsStr, OsString};
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
mod cli_config;
mod cli_install;
mod cli_lint;
mod cli_scripts;
mod ir_output;

mod runtime_loader;
use cli_args::*;
use cli_config::parse_cli;
use cli_lint::lint_module;
use ir_output::{print_ir, print_ir_dot, print_ir_func, print_stats};

// ============================================================================
// Exit Codes
// ============================================================================

const EXIT_SUCCESS: u8 = 0;
const EXIT_COMPILE_ERROR: u8 = 1;
const EXIT_RUNTIME_ERROR: u8 = 2;
const EXIT_USAGE_ERROR: u8 = 3;

fn module_resolution_options(cli: &Cli) -> wjsm_module::ResolutionOptions {
    wjsm_module::ResolutionOptions::default()
        .with_browser(cli.browser)
        .with_conditions(cli.condition.iter().cloned())
}

// ============================================================================
// Runtime bridge (sync CLI -> async Store)
// ============================================================================

fn runtime_options_for_file(
    cli: &Cli,
    input: &Path,
    root: Option<&Path>,
    script_args: &[OsString],
) -> Result<runtime::RuntimeOptions> {
    let script = if path_is_stdin(input) {
        "[stdin]".to_string()
    } else {
        input
            .canonicalize()
            .with_context(|| {
                format!(
                    "failed to canonicalize '{}' for process.argv",
                    input.display()
                )
            })?
            .to_string_lossy()
            .into_owned()
    };
    let env = runtime_env_snapshot();
    let sandbox = fs_sandbox_for_file(input, root, &env);
    let module_loader = runtime_module_loader_for_file(
        input,
        root,
        &sandbox,
        module_resolution_options(cli),
        cli.wants_debug_codegen(),
    )?;
    let mut options = runtime_options_with_script(cli, script, script_args, env, sandbox)?;
    options.module_loader = module_loader;
    Ok(options)
}

fn runtime_options_for_inline(
    cli: &Cli,
    mode_tag: &str,
    script_args: &[OsString],
) -> Result<runtime::RuntimeOptions> {
    let env = runtime_env_snapshot();
    let sandbox = fs_sandbox_for_inline(&env);
    runtime_options_with_script(cli, mode_tag.to_string(), script_args, env, sandbox)
}

fn runtime_options_with_script(
    cli: &Cli,
    script: String,
    script_args: &[OsString],
    env: Vec<(String, String)>,
    sandbox: FsSandbox,
) -> Result<runtime::RuntimeOptions> {
    let mut argv = Vec::with_capacity(script_args.len() + 2);
    argv.push(wjsm_argv0());
    argv.push(script);
    argv.extend(script_args.iter().map(|arg| os_string_lossy(arg)));
    runtime_options_with_argv(cli, argv, env, sandbox)
}

fn runtime_options_with_argv(
    cli: &Cli,
    argv: Vec<String>,
    env: Vec<(String, String)>,
    sandbox: FsSandbox,
) -> Result<runtime::RuntimeOptions> {
    let gc_algorithm = match cli.gc.as_deref() {
        Some(raw) if !raw.is_empty() => raw.parse().map_err(anyhow::Error::msg)?,
        _ => runtime::gc_algorithm_from_env(&env).map_err(anyhow::Error::msg)?,
    };
    let inspect = cli.inspect_config().map_err(anyhow::Error::msg)?;
    let shadow_stack_max = cli
        .shadow_stack_max
        .or_else(|| {
            env.iter()
                .find(|(k, _)| k == "WJSM_SHADOW_STACK_MAX")
                .and_then(|(_, v)| parse_shadow_stack_max_env(v))
        })
        .unwrap_or(wjsm_ir::SHADOW_STACK_DEFAULT_MAX_SIZE as usize);
    Ok(runtime::RuntimeOptions {
        max_heap_size: cli.max_heap_size,
        shadow_stack_max,
        wasmtime_memory_reservation: cli.wasmtime_memory_reservation.map(|value| value as u64),
        gc_algorithm,
        argv,
        cwd: runtime_cwd_string(),
        env,
        pid: std::process::id(),
        ppid: 0,
        platform: node_platform(),
        arch: node_arch(),
        fs_read_roots: sandbox.read_roots,
        fs_write_roots: sandbox.write_roots,
        fs_allow_write_anywhere: sandbox.allow_write_anywhere,
        inspect,
        ..runtime::RuntimeOptions::default()
    })
}

fn parse_shadow_stack_max_env(raw: &str) -> Option<usize> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // 复用 CLI 同款后缀解析：纯数字 / K/M/G。
    crate::cli_args::parse_heap_size(s).ok()
}

fn runtime_module_loader_for_file(
    input: &Path,
    root: Option<&Path>,
    sandbox: &FsSandbox,
    resolution_options: wjsm_module::ResolutionOptions,
    debug_codegen: bool,
) -> Result<Option<std::sync::Arc<dyn runtime::RuntimeModuleLoader>>> {
    if path_is_stdin(input) {
        return Ok(None);
    }
    let root = runtime_loader_root(input, root)?;
    Ok(Some(std::sync::Arc::new(
        runtime_loader::CliRuntimeModuleLoader::with_debug(
            root,
            sandbox.read_roots.clone(),
            resolution_options,
            debug_codegen,
        ),
    )))
}

fn runtime_loader_root(input: &Path, root: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = root {
        return root.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize runtime loader root '{}'",
                root.display()
            )
        });
    }
    let input = input.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize '{}' for runtime module loader",
            input.display()
        )
    })?;
    input.parent().map(Path::to_path_buf).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot infer runtime module root from '{}'",
            input.display()
        )
    })
}

fn wjsm_argv0() -> String {
    std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wjsm".to_string())
}

struct FsSandbox {
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    allow_write_anywhere: bool,
}

fn fs_sandbox_for_file(input: &Path, root: Option<&Path>, env: &[(String, String)]) -> FsSandbox {
    let mut read_roots = default_project_read_roots(env);
    if let Some(root_path) = root {
        push_canonical_root(&mut read_roots, root_path);
    } else if !path_is_stdin(input)
        && let Some(parent) = input
            .canonicalize()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        push_unique_root(&mut read_roots, parent);
    }
    fs_sandbox_from_read_roots(read_roots, env)
}

fn fs_sandbox_for_inline(env: &[(String, String)]) -> FsSandbox {
    fs_sandbox_from_read_roots(default_project_read_roots(env), env)
}

fn fs_sandbox_for_in_process(
    input: &Path,
    env: &[(String, String)],
    cwd_override: Option<&Path>,
) -> FsSandbox {
    let mut read_roots = Vec::new();
    if let Some(cwd) = cwd_override {
        push_canonical_root(&mut read_roots, cwd);
    } else if let Ok(cwd) = std::env::current_dir() {
        push_canonical_root(&mut read_roots, &cwd);
    }
    if let Some(parent) = input
        .canonicalize()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        push_unique_root(&mut read_roots, parent);
    }
    push_env_read_roots(&mut read_roots, env);
    fs_sandbox_from_read_roots(read_roots, env)
}

fn default_project_read_roots(env: &[(String, String)]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        push_canonical_root(&mut roots, &cwd);
    }
    push_env_read_roots(&mut roots, env);
    roots
}

fn fs_sandbox_from_read_roots(mut read_roots: Vec<PathBuf>, env: &[(String, String)]) -> FsSandbox {
    push_unique_root(&mut read_roots, std::env::temp_dir());
    let write_roots = read_roots.clone();
    FsSandbox {
        read_roots,
        write_roots,
        allow_write_anywhere: env
            .iter()
            .any(|(key, value)| key == "WJSM_FS_ALLOW_WRITE" && value == "1"),
    }
}

fn push_env_read_roots(roots: &mut Vec<PathBuf>, env: &[(String, String)]) {
    for raw in env
        .iter()
        .filter_map(|(key, value)| (key == "WJSM_FS_ALLOW_READ").then_some(value))
    {
        for path in std::env::split_paths(raw) {
            push_canonical_root(roots, &path);
        }
    }
}

fn push_canonical_root(roots: &mut Vec<PathBuf>, path: &Path) {
    if let Ok(canonical) = path.canonicalize() {
        push_unique_root(roots, canonical);
    }
}

fn push_unique_root(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.iter().any(|existing| existing == &path) {
        roots.push(path);
    }
}
fn os_string_lossy(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

fn runtime_cwd_string() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.to_string_lossy().into_owned())
}

fn runtime_env_snapshot() -> Vec<(String, String)> {
    std::env::vars_os()
        .map(|(key, value)| (os_string_lossy(&key), os_string_lossy(&value)))
        .collect()
}

fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
}

fn block_on_wasm_execute(wasm: &[u8], options: runtime::RuntimeOptions) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create Tokio runtime for WASM execution")?
        .block_on(runtime::execute_with_options(wasm, options))
}

fn process_exit_code_from_error(error: &anyhow::Error) -> Option<ExitCode> {
    runtime::process_exit_code(error).map(|code| ExitCode::from(code as u8))
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

fn install_embedded_runtime_artifacts() {
    // 安装构建期嵌入的 startup snapshot 与 support cwasm；CLI 与 in-process fixture runner
    // 必须共用同一 runtime artifact 边界，否则相同 WASM 在测试入口会走不同 support 路径。
    if let Some(bytes) = wjsm_runtime_snapshot::EMBEDDED_STARTUP_SNAPSHOT {
        wjsm_runtime::install_embedded_startup_snapshot(bytes);
    }
    if let Some(bytes) = wjsm_runtime_support::embedded_support_cwasm(
        wjsm_runtime_support::SupportGcFlavor::MarkSweep,
    ) {
        wjsm_runtime::install_embedded_support_cwasm(bytes);
    }
}

pub fn main_entry() -> ExitCode {
    install_embedded_runtime_artifacts();

    let cli = match parse_cli(std::env::args_os()) {
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
    setup_colors(cli.color, cli.no_color);

    match cli.command {
        Commands::Build {
            ref input,
            ref eval,
            ref output,
            stage,
            ref root,
            script,
        } => cmd_build(&cli, input, eval, output, stage, root.as_deref(), script),

        Commands::Run {
            ref input,
            ref root,
            watch,
            script,
            ref eval,
            ref args,
        } => {
            if let Some(code) = eval {
                cmd_run_eval(&cli, code, script, "[run-eval]", args)
            } else if let Some(input) = input {
                let script_name = input.to_string_lossy();
                if !path_is_stdin(input)
                    && !input.exists()
                    && cli_scripts::package_script_exists(root.as_deref(), &script_name)?
                {
                    if watch {
                        bail!("watch mode is not supported for package scripts");
                    }
                    cli_scripts::run_package_script(root.as_deref(), &script_name, args)?;
                    Ok(ExitCode::from(EXIT_SUCCESS))
                } else if watch {
                    cmd_run_watch(&cli, input, root.as_deref(), script, args)
                } else {
                    cmd_run(&cli, input, root.as_deref(), script, args)
                }
            } else {
                bail!("Either an input file or -e <code> is required");
            }
        }

        Commands::Test {
            ref input,
            ref eval,
            ref root,
            script,
        } => cmd_test(&cli, input, eval, root.as_deref(), script),

        Commands::Check {
            ref input,
            ref eval,
            ref root,
            script,
        } => cmd_check(&cli, input, eval, root.as_deref(), script),

        Commands::Lint {
            ref input,
            ref eval,
            ref root,
            script,
        } => cmd_lint(&cli, input, eval, root.as_deref(), script),

        Commands::Eval { ref code } => cmd_eval(&cli, code),

        Commands::Repl { ref eval, script } => cmd_repl(&cli, eval.as_deref(), script),

        Commands::DumpIr {
            ref input,
            ref eval,
            format,
            ref root,
            script,
            ref func,
        } => cmd_dump_ir(
            &cli,
            input,
            eval,
            format,
            root.as_deref(),
            script,
            func.as_deref(),
        ),

        Commands::DumpAst {
            ref input,
            ref eval,
            ref root,
            script,
        } => cmd_dump_ast(&cli, input, eval, root.as_deref(), script),

        Commands::DumpWat {
            ref input,
            ref eval,
            ref root,
            script,
            ref func,
            skeleton,
        } => cmd_dump_wat(
            &cli,
            input,
            eval,
            root.as_deref(),
            script,
            func.as_deref(),
            skeleton,
        ),

        Commands::Fmt { ref input, write } => cmd_fmt(input, write),

        Commands::Validate { ref input } => cmd_validate(input),

        Commands::Size { ref input } => cmd_size(input),

        Commands::Disasm {
            ref input,
            ref func,
            skeleton,
        } => cmd_disasm(input, func.as_deref(), skeleton),

        Commands::Cache { ref command } => cmd_cache(command),

        Commands::Install { ref packages } => {
            cli_install::install_packages(packages)?;
            Ok(ExitCode::from(EXIT_SUCCESS))
        }

        Commands::Completions { shell } => cmd_completions(shell),

        Commands::Init { ref path, force } => cmd_init(path, force),
        Commands::Version { extended } => cmd_version(extended),
    }
}

// ============================================================================
// Color Setup
// ============================================================================

fn setup_colors(choice: Option<ColorChoice>, no_color: bool) {
    let use_colors = if no_color {
        false
    } else {
        match choice {
            Some(ColorChoice::Always) => true,
            Some(ColorChoice::Never) => false,
            Some(ColorChoice::Auto) | None => resolve_auto_colors(),
        }
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
    input: &Option<PathBuf>,
    eval: &Option<String>,
    output: &Path,
    stage: Option<Stage>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    let stage = stage.unwrap_or(Stage::Compile);

    if matches!(stage, Stage::Parse | Stage::Lower) && output != Path::new("out.wasm") {
        bail!(
            "`-o` / `--output` cannot be used with `--stage parse` or `--stage lower` (output goes to stdout)"
        );
    }

    match stage {
        Stage::Parse | Stage::Lower => {
            let result = match resolve_input(input, eval)? {
                InputSource::Inline(code) => run_pipeline(
                    &code,
                    None,
                    stage,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?,
                InputSource::File(path) => {
                    if path_is_stdin(&path) {
                        let (source, filename) = read_input_for_parse(&path)?;
                        run_pipeline(
                            &source,
                            filename.as_deref(),
                            stage,
                            cli.effective_verbose(),
                            cli.time,
                            cli.target,
                            script,
                            cli.should_verify_ir(),
                            cli.wants_debug_codegen(),
                        )?
                    } else {
                        run_file_input_pipeline(&path, root, stage, cli, script)?
                    }
                }
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
            if path_is_stdout(output) && io::stdout().is_terminal() {
                bail!(
                    "refusing to write binary WASM to a terminal; redirect stdout to a file or use `-o <path>`"
                );
            }

            if !cli.quiet
                && !path_is_stdout(output)
                && output == Path::new("out.wasm")
                && output.exists()
            {
                eprintln!(
                    "warning: '{}' already exists and will be overwritten (use `-o` to choose another path)",
                    output.display()
                );
            }

            let wasm = match resolve_input(input, eval)? {
                InputSource::Inline(code) => {
                    compile_source(
                        &code,
                        None,
                        cli.target,
                        script,
                        cli.should_verify_ir(),
                        cli.wants_debug_codegen(),
                    )?
                }
                InputSource::File(path) => {
                    if path_is_stdin(&path) {
                        let (source, _) = read_input_for_parse(&path)?;
                        compile_source(
                            &source,
                            None,
                            cli.target,
                            script,
                            cli.should_verify_ir(),
                            cli.wants_debug_codegen(),
                        )?
                    } else {
                        compile_from_file_input(
                            &path,
                            root,
                            cli.target,
                            script,
                            cli.should_verify_ir(),
                            cli.wants_debug_codegen(),
                            &module_resolution_options(cli),
                        )?
                    }
                }
            };

            if path_is_stdout(output) {
                io::stdout().write_all(&wasm)?;
            } else {
                fs::write(output, &wasm)?;
                if cli.verbose_enabled(1) {
                    eprintln!("Wrote {} bytes to {}", wasm.len(), output.display());
                }
            }

            if cli.stats {
                eprintln!("Output: {} bytes", wasm.len());
            }
        }
        Stage::Execute => {
            if path_is_stdout(output) && io::stdout().is_terminal() {
                bail!(
                    "refusing to write binary WASM to a terminal; redirect stdout to a file or use `-o <path>`"
                );
            }

            let (result, options) = match resolve_input(input, eval)? {
                InputSource::Inline(code) => (
                    compile_source_to_pipeline_result(
                        &code,
                        None,
                        cli.target,
                        script,
                        cli.verbose_enabled(1),
                        cli.should_verify_ir(),
                        cli.wants_debug_codegen(),
                    )?,
                    runtime_options_for_inline(cli, "[run-eval]", &[])?,
                ),
                InputSource::File(path) => {
                    if path_is_stdin(&path) {
                        let (source, _) = read_input_for_parse(&path)?;
                        (
                            compile_source_to_pipeline_result(
                                &source,
                                None,
                                cli.target,
                                script,
                                cli.verbose_enabled(1),
                                cli.should_verify_ir(),
                                cli.wants_debug_codegen(),
                            )?,
                            runtime_options_for_file(cli, &path, root, &[])?,
                        )
                    } else {
                        (
                            compile_file_input_to_pipeline_result(
                                &path,
                                root,
                                cli.target,
                                script,
                                cli.verbose_enabled(1),
                                cli.should_verify_ir(),
                                cli.wants_debug_codegen(),
                                &module_resolution_options(cli),
                            )?,
                            runtime_options_for_file(cli, &path, root, &[])?,
                        )
                    }
                }
            };

            let wasm = result
                .wasm
                .as_ref()
                .context("compile stage produced no WASM")?;

            if path_is_stdout(output) {
                io::stdout().write_all(wasm)?;
            } else {
                fs::write(output, wasm)?;
                if cli.verbose_enabled(1) {
                    eprintln!("Wrote {} bytes to {}", wasm.len(), output.display());
                }
            }

            return run_compile_then_execute(cli, result, options);
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_run(
    cli: &Cli,
    input: &Path,
    root: Option<&Path>,
    script: bool,
    script_args: &[OsString],
) -> Result<ExitCode> {
    let verbose_compile = cli.verbose_enabled(1);
    let result = if path_is_stdin(input) {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        compile_source_to_pipeline_result(
            &source,
            None,
            cli.target,
            script,
            verbose_compile,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?
    } else {
        compile_file_input_to_pipeline_result(
            input,
            root,
            cli.target,
            script,
            verbose_compile,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
            &module_resolution_options(cli),
        )?
    };
    let options = runtime_options_for_file(cli, input, root, script_args)?;

    run_compile_then_execute(cli, result, options)
}

fn cmd_run_eval(
    cli: &Cli,
    code: &str,
    script: bool,
    mode_tag: &str,
    script_args: &[OsString],
) -> Result<ExitCode> {
    let result = compile_source_to_pipeline_result(
        code,
        None,
        cli.target,
        script,
        cli.verbose_enabled(1),
        cli.should_verify_ir(),
        cli.wants_debug_codegen(),
    )?;
    let options = runtime_options_for_inline(cli, mode_tag, script_args)?;
    run_compile_then_execute(cli, result, options)
}

fn cmd_test(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    if let Some(code) = eval {
        return cmd_run_eval(cli, code, script, "[run-eval]", &[]);
    }

    if input.is_none() && cli_scripts::package_script_exists(root, "test")? {
        cli_scripts::run_package_script(root, "test", &[])?;
        return Ok(ExitCode::from(EXIT_SUCCESS));
    }

    let input = input.as_deref().unwrap_or_else(|| Path::new("."));
    let files = if input.is_dir() {
        discover_test_files(input)?
    } else {
        vec![input.to_path_buf()]
    };

    if files.is_empty() {
        bail!("no JS/TS test files found under '{}'", input.display());
    }

    let mut failed = 0usize;
    for file in &files {
        if cli.verbose_enabled(1) {
            eprintln!("test {}", file.display());
        }
        match cmd_run(cli, file, root, script, &[]) {
            Ok(code) if code == ExitCode::from(EXIT_SUCCESS) => {
                if cli.verbose_enabled(1) {
                    eprintln!("ok {}", file.display());
                }
            }
            Ok(code) => {
                failed += 1;
                eprintln!("FAILED {} (exit {:?})", file.display(), code);
            }
            Err(error) => {
                failed += 1;
                eprintln!("FAILED {}: {:#}", file.display(), error);
            }
        }
    }

    if !cli.quiet {
        let passed = files.len() - failed;
        eprintln!("test result: {passed} passed; {failed} failed");
    }

    if failed == 0 {
        Ok(ExitCode::from(EXIT_SUCCESS))
    } else {
        Ok(ExitCode::from(EXIT_COMPILE_ERROR))
    }
}

fn discover_test_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() && is_test_file(entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn is_test_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".test.js")
        || name.ends_with(".test.ts")
        || name.ends_with("_test.js")
        || name.ends_with("_test.ts")
}

fn cmd_lint(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    let result = match resolve_input(input, eval)? {
        InputSource::Inline(code) => run_pipeline(
            &code,
            None,
            Stage::Parse,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?,
        InputSource::File(path) => {
            if path_is_stdin(&path) {
                let (source, filename) = read_input_for_parse(&path)?;
                run_pipeline(
                    &source,
                    filename.as_deref(),
                    Stage::Parse,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?
            } else {
                run_file_input_pipeline(&path, root, Stage::Parse, cli, script)?
            }
        }
    };

    let diagnostics = result.ast.as_ref().map(lint_module).unwrap_or_default();
    if diagnostics.is_empty() {
        if cli.verbose_enabled(1) {
            eprintln!("✓ No lint warnings found");
        }
        return Ok(ExitCode::from(EXIT_SUCCESS));
    }

    for diagnostic in &diagnostics {
        eprintln!("warning[{}]: {}", diagnostic.code, diagnostic.message);
    }
    Ok(ExitCode::from(EXIT_COMPILE_ERROR))
}

fn cmd_repl(cli: &Cli, eval: Option<&str>, script: bool) -> Result<ExitCode> {
    if let Some(code) = eval {
        return if script {
            cmd_run_eval(cli, code, true, "[repl]", &[])
        } else {
            cmd_eval(cli, code)
        };
    }

    let mut line = String::new();
    loop {
        if io::stdin().is_terminal() {
            print!("wjsm> ");
            io::stdout().flush()?;
        }
        line.clear();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let source = line.trim();
        if source.is_empty() {
            continue;
        }
        if matches!(source, ".exit" | ".quit") {
            break;
        }
        let result = if script {
            cmd_run_eval(cli, source, true, "[repl]", &[])
        } else {
            cmd_eval(cli, source)
        };
        if let Err(error) = result {
            eprintln!("Error: {error:#}");
        }
    }
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_run_watch(
    cli: &Cli,
    input: &Path,
    root: Option<&Path>,
    script: bool,
    script_args: &[OsString],
) -> Result<ExitCode> {
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

    if !input.exists() {
        bail!("Input file '{}' does not exist", input.display());
    }

    let watch_target = root.unwrap_or(input);
    let watch_mode = if root.is_some() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    eprintln!("Watching {} for changes...", watch_target.display());
    let mut last_exit = match cmd_run(cli, input, root, script, script_args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Initial run failed: {:#}", e);
            ExitCode::from(EXIT_COMPILE_ERROR)
        }
    };

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

    watcher.watch(watch_target, watch_mode)?;

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
                    last_exit = match cmd_run(cli, input, root, script, script_args) {
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

fn cmd_check(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    let result = match resolve_input(input, eval)? {
        InputSource::Inline(code) => run_pipeline(
            &code,
            None,
            Stage::Lower,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?,
        InputSource::File(path) => {
            if path_is_stdin(&path) {
                let (source, filename) = read_input_for_parse(&path)?;
                run_pipeline(
                    &source,
                    filename.as_deref(),
                    Stage::Lower,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?
            } else {
                run_file_input_pipeline(&path, root, Stage::Lower, cli, script)?
            }
        }
    };

    if cli.verbose_enabled(1) {
        eprintln!("✓ No errors found");
    }

    if cli.stats {
        print_stats(&result);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_eval(cli: &Cli, code: &str) -> Result<ExitCode> {
    let wrapped = format!("console.log(({code}))");
    cmd_run_eval(cli, &wrapped, false, "[eval]", &[])
}

fn cmd_dump_ir(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    format: DumpFormat,
    root: Option<&Path>,
    script: bool,
    func: Option<&str>,
) -> Result<ExitCode> {
    if func.is_some() && format == DumpFormat::Dot {
        bail!("--func cannot be used with --format dot");
    }

    let result = match resolve_input(input, eval)? {
        InputSource::Inline(code) => run_pipeline(
            &code,
            None,
            Stage::Lower,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?,
        InputSource::File(path) => {
            if path_is_stdin(&path) {
                let (source, filename) = read_input_for_parse(&path)?;
                run_pipeline(
                    &source,
                    filename.as_deref(),
                    Stage::Lower,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?
            } else {
                run_file_input_pipeline(&path, root, Stage::Lower, cli, script)?
            }
        }
    };

    if let Some(program) = &result.program {
        if let Some(name) = func {
            print_ir_func(program, name)?;
        } else {
            match format {
                DumpFormat::Text => print_ir(program),
                DumpFormat::Dot => print_ir_dot(program),
            }
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_ast(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    let result = match resolve_input(input, eval)? {
        InputSource::Inline(code) => run_pipeline(
            &code,
            None,
            Stage::Parse,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?,
        InputSource::File(path) => {
            if path_is_stdin(&path) {
                let (source, filename) = read_input_for_parse(&path)?;
                run_pipeline(
                    &source,
                    filename.as_deref(),
                    Stage::Parse,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?
            } else {
                run_file_input_pipeline(&path, root, Stage::Parse, cli, script)?
            }
        }
    };

    if let Some(ast) = &result.ast {
        let json = serde_json::to_string_pretty(ast)?;
        println!("{}", json);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

/// 用 wasmprinter Config 输出 WAT 字符串。`name_unnamed(true)` 始终启用，
/// 使合成函数获得 `$fN` 名称；`skeleton` 为 true 时省略指令体。
fn print_wat_to_string(wasm: &[u8], skeleton: bool) -> Result<String> {
    use wasmprinter::{Config, PrintFmtWrite};
    let mut cfg = Config::new();
    cfg.name_unnamed(true);
    if skeleton {
        cfg.print_skeleton(true);
    }
    let mut dst = String::new();
    cfg.print(wasm, &mut PrintFmtWrite(&mut dst))?;
    Ok(dst)
}

/// 从完整 WAT 文本中提取单个函数定义块（按 `$name` 匹配）。
/// 跟踪括号深度：从 `(func $name` 行开始，深度归零时结束。
fn filter_wat_func(wat: &str, name: &str) -> Result<String> {
    let target = format!("${name}");
    let lines: Vec<&str> = wat.lines().collect();
    let mut available: Vec<String> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let rest = match trimmed.strip_prefix("(func ") {
            Some(r) => r,
            None => continue,
        };
        let token_end = rest
            .find(|c: char| c.is_whitespace() || c == ')')
            .unwrap_or(rest.len());
        let token = &rest[..token_end];
        if !token.starts_with('$') {
            continue;
        }
        available.push(token.to_string());
        if token != target {
            continue;
        }
        // 找到目标函数，按括号深度截取完整块
        let mut result = String::new();
        let mut depth: i32 = 0;
        for &l in &lines[i..] {
            for ch in l.chars() {
                match ch {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            result.push_str(l);
            result.push('\n');
            if depth == 0 {
                return Ok(result);
            }
        }
    }

    bail!(
        "function '{name}' not found in WAT; available: {}",
        available.join(", ")
    );
}

fn cmd_dump_wat(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
    func: Option<&str>,
    skeleton: bool,
) -> Result<ExitCode> {
    if func.is_some() && skeleton {
        bail!("--skeleton and --func are mutually exclusive");
    }

    let result = match resolve_input(input, eval)? {
        InputSource::Inline(code) => run_pipeline(
            &code,
            None,
            Stage::Compile,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        )?,
        InputSource::File(path) => {
            if path_is_stdin(&path) {
                let mut source = String::new();
                io::stdin().read_to_string(&mut source)?;
                run_pipeline(
                    &source,
                    None,
                    Stage::Compile,
                    cli.effective_verbose(),
                    cli.time,
                    cli.target,
                    script,
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                )?
            } else {
                let pipeline = compile_file_input_to_pipeline_result(
                    &path,
                    root,
                    cli.target,
                    script,
                    cli.verbose_enabled(1),
                    cli.should_verify_ir(),
                    cli.wants_debug_codegen(),
                    &module_resolution_options(cli),
                )?;
                if cli.time {
                    pipeline.timings.print(cli.effective_verbose());
                }
                pipeline
            }
        }
    };

    if cli.stats {
        print_stats(&result);
    }

    if let Some(wasm) = &result.wasm {
        let wat = print_wat_to_string(wasm, skeleton)?;
        if let Some(name) = func {
            println!("{}", filter_wat_func(&wat, name)?);
        } else {
            println!("{}", wat);
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_fmt(input: &Path, write: bool) -> Result<ExitCode> {
    let source = read_source_file(input)?;

    let module = parser::parse_module_with_path(&source, input)?;
    let formatted = emit_js(&module)?;

    if write {
        fs::write(input, &formatted)?;
        eprintln!("Formatted {}", input.display());
    } else {
        println!("{}", formatted);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_validate(input: &Path) -> Result<ExitCode> {
    let bytes = fs::read(input)?;

    match wasmparser::validate(&bytes) {
        Ok(_) => {
            println!("✓ {} is valid WASM", input.display());
            Ok(ExitCode::from(EXIT_SUCCESS))
        }
        Err(e) => {
            println!("✗ {} is NOT valid WASM", input.display());
            eprintln!("Validation error: {}", e);
            Ok(ExitCode::from(EXIT_COMPILE_ERROR))
        }
    }
}

fn cmd_size(input: &Path) -> Result<ExitCode> {
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

    println!("WASM Size Breakdown for {}", input.display());
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

fn cmd_disasm(input: &Path, func: Option<&str>, skeleton: bool) -> Result<ExitCode> {
    if func.is_some() && skeleton {
        bail!("--skeleton and --func are mutually exclusive");
    }

    let bytes = fs::read(input)?;
    let wat = print_wat_to_string(&bytes, skeleton)?;

    if let Some(name) = func {
        println!("{}", filter_wat_func(&wat, name)?);
    } else {
        println!("{}", wat);
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_cache(command: &CacheCommand) -> Result<ExitCode> {
    match command {
        CacheCommand::Stats => {
            let stats = runtime::module_cache_stats()?;
            match stats.path {
                Some(path) => {
                    println!("Cache directory: {}", path.display());
                    println!("Entries: {}", stats.entries);
                    println!("Bytes: {}", stats.bytes);
                }
                None => {
                    println!("Cache disabled");
                    println!("Entries: 0");
                    println!("Bytes: 0");
                }
            }
        }
        CacheCommand::Clear => {
            let removed = runtime::clear_module_cache()?;
            println!("Cleared {removed} cache entries");
        }
    }
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_completions(shell: clap_complete::Shell) -> Result<ExitCode> {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, bin_name, &mut io::stdout());
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_init(path: &Path, force: bool) -> Result<ExitCode> {
    let dir = path;
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
    fs::write(&package_path, serde_json::to_string_pretty(&package_json)?)?;

    println!("Created project at {}", path.display());
    println!();
    println!("To run:");
    println!("  cd {}", path.display());
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

        println!("  Target: wasm");
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
    verify_ir: bool,
    debug_codegen: bool,
) -> Result<Program> {
    let display_name = filename.map(str::to_string).unwrap_or_else(|| {
        if script {
            "input.js".into()
        } else {
            "input.ts".into()
        }
    });
    // debug_codegen 时在语句入口发射 DebugCheck，供 inspector 单步/断点映射。
    let program = semantic::lower_module_with_debug_source(
        module,
        script,
        Some(std::sync::Arc::from(source)),
        display_name,
        debug_codegen,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    verify_ir_for_pipeline(&program, verify_ir)?;
    Ok(program)
}

fn compile_program_to_wasm(
    program: &Program,
    target: Target,
    debug_codegen: bool,
) -> Result<Vec<u8>> {
    match target {
        Target::Wasm => backend_wasm::compile_with_options(
            program,
            backend_wasm::CompileOptions {
                debug: debug_codegen,
            },
        ),
        Target::Jit => bail!("JIT backend is not implemented yet"),
    }
}

fn verify_ir_for_pipeline(program: &Program, verify_ir: bool) -> Result<()> {
    if verify_ir {
        program.verify().context("IR verification failed")?;
    }
    Ok(())
}

fn run_pipeline(
    source: &str,
    filename: Option<&str>,
    stop_at: Stage,
    verbose: u8,
    time: bool,
    target: Target,
    script: bool,
    verify_ir: bool,
    debug_codegen: bool,
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
    if verbose >= 2 {
        eprintln!("Parsed module items: {}", module.body.len());
    }
    result.ast = Some(module);

    if matches!(stop_at, Stage::Parse) {
        return Ok(result);
    }

    // Lower
    if verbose >= 1 {
        eprintln!("Lowering to IR...");
    }
    let start = Instant::now();
    let program = lower_parsed_module(
        source,
        filename,
        result.ast.take().unwrap(),
        script,
        verify_ir,
        debug_codegen,
    )?;
    result.timings.lower_us = start.elapsed().as_micros() as u64;
    if verbose >= 2 {
        eprintln!(
            "Lowered IR: {} constants, {} functions",
            program.constants().len(),
            program.functions().len()
        );
    }
    result.program = Some(program);

    if matches!(stop_at, Stage::Lower) {
        return Ok(result);
    }

    // Compile
    if verbose >= 1 {
        eprintln!("Compiling to WASM...");
    }
    let start = Instant::now();
    let wasm = compile_program_to_wasm(result.program.as_ref().unwrap(), target, debug_codegen)?;
    result.timings.compile_us = start.elapsed().as_micros() as u64;
    if verbose >= 2 {
        eprintln!("Compiled WASM bytes: {}", wasm.len());
    }
    result.wasm = Some(wasm);

    if time {
        result.timings.print(verbose);
    }

    Ok(result)
}

/// 文件输入走 compile plan（含 `--root` bundling），在指定 stage 停止。
fn run_file_input_pipeline(
    input: &Path,
    root: Option<&Path>,
    stop_at: Stage,
    cli: &Cli,
    script: bool,
) -> Result<PipelineResult> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            if cli.verbose_enabled(1) {
                eprintln!("Bundling modules...");
            }
            let start = Instant::now();
            let mut result = PipelineResult {
                ast: None,
                program: None,
                wasm: None,
                timings: PipelineTimings::default(),
            };
            let resolution_options = module_resolution_options(cli);
            match stop_at {
                Stage::Parse => {
                    let ast = wjsm_module::parse_entry_ast_with_options(
                        &entry,
                        &root,
                        resolution_options.clone(),
                    )?;
                    result.timings.parse_us = start.elapsed().as_micros() as u64;
                    result.ast = Some(ast);
                }
                Stage::Lower => {
                    let program = wjsm_module::lower_bundle_with_options(
                        &entry,
                        &root,
                        resolution_options.clone(),
                    )?;
                    verify_ir_for_pipeline(&program, cli.should_verify_ir())?;
                    result.timings.lower_us = start.elapsed().as_micros() as u64;
                    result.program = Some(program);
                }
                Stage::Compile | Stage::Execute => {
                    let wasm = compile_bundle(
                        &entry,
                        &root,
                        cli.target,
                        cli.should_verify_ir(),
                        cli.wants_debug_codegen(),
                        &resolution_options,
                    )?;
                    result.timings.compile_us = start.elapsed().as_micros() as u64;
                    result.wasm = Some(wasm);
                }
            }
            if cli.time {
                result.timings.print(cli.effective_verbose());
            }
            Ok(result)
        }
        CompilePlan::SingleSource { source, filename } => run_pipeline(
            &source,
            Some(filename.as_str()),
            stop_at,
            cli.effective_verbose(),
            cli.time,
            cli.target,
            script,
            cli.should_verify_ir(),
            cli.wants_debug_codegen(),
        ),
    }
}

// ============================================================================
// Input/Output Helpers

fn path_is_stdio(path: &Path, marker: &str) -> bool {
    path.as_os_str() == OsStr::new(marker)
}

fn path_is_stdin(path: &Path) -> bool {
    path_is_stdio(path, "-")
}

fn path_is_stdout(path: &Path) -> bool {
    path_is_stdio(path, "-")
}

/// 将路径转为字符串作为 SWC 诊断文件名；文件系统操作必须继续使用 `Path`。
fn path_to_diagnostic_filename(path: &Path) -> String {
    path.display().to_string()
}

/// 读取源码文件：按字节读取再 UTF-8 解码，避免对路径本身使用 lossy 转换
fn read_source_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("Failed to read '{}'", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ============================================================================

fn read_input(input: &Path) -> Result<String> {
    if path_is_stdin(input) {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("Failed to read from stdin")?;
        Ok(source)
    } else {
        read_source_file(input)
    }
}

/// 读取源码，并在输入为文件路径时返回用于诊断的路径字符串。
fn read_input_for_parse(input: &Path) -> Result<(String, Option<String>)> {
    let source = read_input(input)?;
    let filename = if path_is_stdin(input) {
        None
    } else {
        Some(path_to_diagnostic_filename(input))
    };
    Ok((source, filename))
}

/// CLI 输入来源：内联代码或文件路径。
enum InputSource {
    Inline(String),
    File(PathBuf),
}

/// 统一解析 `-e <code>` 与位置参数 `<file>`：`-e` 优先，二者皆无则报错。
fn resolve_input(input: &Option<PathBuf>, eval: &Option<String>) -> Result<InputSource> {
    match (eval, input) {
        (Some(code), _) => Ok(InputSource::Inline(code.clone())),
        (None, Some(path)) => Ok(InputSource::File(path.clone())),
        (None, None) => bail!("Either an input file or -e <code> is required"),
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
    Bundle { entry: PathBuf, root: PathBuf },
    SingleSource { source: String, filename: String },
}

fn build_compile_plan(input: &Path, root: Option<&Path>) -> Result<CompilePlan> {
    if let Some(root_path) = root {
        return bundle_plan_from_root(input, root_path);
    }

    let source = read_source_file(input)?;
    let module = parser::parse_module_with_path(&source, input)?;
    let is_esm = wjsm_module::is_es_module(&module);
    let is_cjs = wjsm_module::is_commonjs_module(&module);

    if !is_esm && !is_cjs {
        return Ok(CompilePlan::SingleSource {
            source,
            filename: path_to_diagnostic_filename(input),
        });
    }

    let canonical_input = input.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize input file after reading '{}'; file may have been moved or deleted",
            input.display()
        )
    })?;
    let parent = canonical_input
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot infer module root from '{}'", input.display()))?;
    let file_name = canonical_input.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot infer module entry file name from '{}'",
            input.display()
        )
    })?;

    Ok(CompilePlan::Bundle {
        entry: PathBuf::from(file_name),
        root: parent.to_path_buf(),
    })
}

fn bundle_plan_from_root(input: &Path, root: &Path) -> Result<CompilePlan> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize root path '{}'", root.display()))?;
    let canonical_input = input.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize input file '{}' under root '{}'",
            input.display(),
            root.display()
        )
    })?;
    canonical_input.strip_prefix(&canonical_root).map_err(|_| {
        anyhow::anyhow!(
            "input file '{}' is not under root '{}'",
            input.display(),
            root.display()
        )
    })?;
    Ok(CompilePlan::Bundle {
        entry: canonical_input,
        root: canonical_root,
    })
}

fn run_compile_then_execute(
    cli: &Cli,
    mut result: PipelineResult,
    options: runtime::RuntimeOptions,
) -> Result<ExitCode> {
    let wasm = result
        .wasm
        .as_ref()
        .context("compile stage produced no WASM")?;

    if cli.stats {
        print_stats(&result);
    }

    let start = Instant::now();
    let exec_result = block_on_wasm_execute(wasm, options);
    result.timings.execute_us = start.elapsed().as_micros() as u64;

    if cli.time {
        result.timings.print(cli.effective_verbose());
    }

    if let Err(e) = exec_result {
        if let Some(code) = process_exit_code_from_error(&e) {
            return Ok(code);
        }
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
    verify_ir: bool,
    debug_codegen: bool,
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
    let program = lower_parsed_module(
        source,
        filename,
        result.ast.take().unwrap(),
        script,
        verify_ir,
        debug_codegen,
    )?;
    result.timings.lower_us = start.elapsed().as_micros() as u64;
    result.program = Some(program);

    if verbose {
        eprintln!("Compiling to WASM...");
    }
    let start = Instant::now();
    let wasm = compile_program_to_wasm(result.program.as_ref().unwrap(), target, debug_codegen)?;
    result.timings.compile_us = start.elapsed().as_micros() as u64;
    result.wasm = Some(wasm);

    Ok(result)
}

fn compile_file_input_to_pipeline_result(
    input: &Path,
    root: Option<&Path>,
    target: Target,
    script: bool,
    verbose: bool,
    verify_ir: bool,
    debug_codegen: bool,
    resolution_options: &wjsm_module::ResolutionOptions,
) -> Result<PipelineResult> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            if verbose {
                eprintln!("Bundling modules...");
            }
            let start = Instant::now();
            let wasm = compile_bundle(
                &entry,
                &root,
                target,
                verify_ir,
                debug_codegen,
                resolution_options,
            )?;
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
        CompilePlan::SingleSource { source, filename } => compile_source_to_pipeline_result(
            &source,
            Some(filename.as_str()),
            target,
            script,
            verbose,
            verify_ir,
            debug_codegen,
        ),
    }
}

fn compile_from_file_input(
    input: &Path,
    root: Option<&Path>,
    target: Target,
    script: bool,
    verify_ir: bool,
    debug_codegen: bool,
    resolution_options: &wjsm_module::ResolutionOptions,
) -> Result<Vec<u8>> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => compile_bundle(
            &entry,
            &root,
            target,
            verify_ir,
            debug_codegen,
            resolution_options,
        ),
        CompilePlan::SingleSource { source, filename } => compile_source(
            &source,
            Some(filename.as_str()),
            target,
            script,
            verify_ir,
            debug_codegen,
        ),
    }
}

fn compile_source(
    source: &str,
    filename: Option<&str>,
    target: Target,
    script: bool,
    verify_ir: bool,
    debug_codegen: bool,
) -> Result<Vec<u8>> {
    let module = if script {
        parser::parse_script_as_module(source)?
    } else if let Some(filename) = filename {
        parser::parse_module_with_filename(source, filename)?
    } else {
        parser::parse_module(source)?
    };
    let program = lower_parsed_module(source, filename, module, script, verify_ir, debug_codegen)?;
    compile_program_to_wasm(&program, target, debug_codegen)
}

fn compile_bundle(
    entry: &Path,
    root: &Path,
    target: Target,
    verify_ir: bool,
    debug_codegen: bool,
    resolution_options: &wjsm_module::ResolutionOptions,
) -> Result<Vec<u8>> {
    match target {
        Target::Wasm => {
            let program = wjsm_module::lower_bundle_with_debug(
                entry,
                root,
                resolution_options.clone(),
                debug_codegen,
            )
            .with_context(|| {
                format!(
                    "bundle entry {} from root {}",
                    entry.display(),
                    root.display()
                )
            })?;
            if verify_ir {
                verify_ir_for_pipeline(&program, true)?;
            }
            compile_program_to_wasm(&program, target, debug_codegen)
        }
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
    run_file_in_process_with_options(input, &[], &[], None)
}

pub fn run_file_in_process_with_options(
    input: &Path,
    script_args: &[&str],
    env_overrides: &[(&str, &str)],
    cwd_override: Option<&Path>,
) -> (i32, Vec<u8>, Vec<u8>) {
    install_embedded_runtime_artifacts();

    let wasm = match compile_file_input_to_pipeline_result(
        input,
        None,
        Target::Wasm,
        false,
        false,
        false,
        false,
        &wjsm_module::ResolutionOptions::default(),
    )
    .and_then(|result| result.wasm.context("compile stage produced no WASM"))
    {
        Ok(wasm) => wasm,
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

    let options =
        match runtime_options_for_in_process(input, script_args, env_overrides, cwd_override) {
            Ok(options) => options,
            Err(e) => {
                return (
                    EXIT_RUNTIME_ERROR as i32,
                    Vec::new(),
                    format!("Runtime error: {e:#}\n").into_bytes(),
                );
            }
        };
    let mut stdout: Vec<u8> = Vec::new();
    match rt.block_on(runtime::execute_with_writer_with_options(
        &wasm,
        &mut stdout,
        options,
    )) {
        Ok((_, diagnostics)) => (EXIT_SUCCESS as i32, stdout, diagnostics),
        Err(e) => {
            if let Some(code) = runtime::process_exit_code(&e) {
                let diagnostics = runtime::process_exit_diagnostics(&e)
                    .map(|bytes| bytes.to_vec())
                    .unwrap_or_default();
                return (code, stdout, diagnostics);
            }
            (
                EXIT_RUNTIME_ERROR as i32,
                stdout,
                format!("Runtime error: {e:#}\n").into_bytes(),
            )
        }
    }
}

fn runtime_options_for_in_process(
    input: &Path,
    script_args: &[&str],
    env_overrides: &[(&str, &str)],
    cwd_override: Option<&Path>,
) -> Result<runtime::RuntimeOptions> {
    let script = input
        .canonicalize()
        .unwrap_or_else(|_| input.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut argv = Vec::with_capacity(script_args.len() + 2);
    argv.push(wjsm_argv0());
    argv.push(script);
    argv.extend(script_args.iter().map(|arg| (*arg).to_string()));

    let mut env = runtime_env_snapshot();
    for (key, value) in env_overrides {
        env.retain(|(existing, _)| existing != key);
        env.push(((*key).to_string(), (*value).to_string()));
    }

    let gc_algorithm = runtime::gc_algorithm_from_env(&env).map_err(anyhow::Error::msg)?;
    let sandbox = fs_sandbox_for_in_process(input, &env, cwd_override);
    let module_loader = runtime_module_loader_for_file(
        input,
        None,
        &sandbox,
        wjsm_module::ResolutionOptions::default(),
        false,
    )?;

    Ok(runtime::RuntimeOptions {
        max_heap_size: None,
        shadow_stack_max: wjsm_ir::SHADOW_STACK_DEFAULT_MAX_SIZE as usize,
        gc_algorithm,
        argv,
        cwd: cwd_override
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .or_else(runtime_cwd_string),
        env,
        pid: std::process::id(),
        ppid: 0,
        platform: node_platform(),
        arch: node_arch(),
        fs_read_roots: sandbox.read_roots,
        fs_write_roots: sandbox.write_roots,
        fs_allow_write_anywhere: sandbox.allow_write_anywhere,
        module_loader,
        ..runtime::RuntimeOptions::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::ops::Deref;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_PROJECT: AtomicUsize = AtomicUsize::new(0);

    struct TestProject {
        path: PathBuf,
    }

    impl TestProject {
        fn new(case: &str) -> Self {
            let id = NEXT_TEST_PROJECT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wjsm_cli_browser_conditions_{case}_{}_{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("test project should be created");
            Self { path }
        }
    }

    impl Deref for TestProject {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.path
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, content).expect("fixture file should be writable");
    }

    fn parse_cli_for_test(args: &[&str]) -> Cli {
        parse_cli(args).expect("CLI args should parse")
    }

    #[test]
    fn cli_inspect_flag_defaults_and_parses_address() {
        // `--inspect` 无值：default_missing_value → 127.0.0.1:9229
        let bare = parse_cli_for_test(&["wjsm", "--inspect", "eval", "1"]);
        let cfg = bare.inspect_config().expect("inspect parse");
        assert_eq!(
            cfg,
            Some(runtime::InspectConfig {
                host: "127.0.0.1".into(),
                port: 9229,
                break_on_start: false,
            })
        );
        assert!(bare.wants_debug_codegen());

        // 必须用 `=` 传自定义地址，避免吞子命令
        let custom = parse_cli_for_test(&["wjsm", "--inspect=0.0.0.0:0", "eval", "1"]);
        let cfg = custom.inspect_config().expect("inspect parse");
        assert_eq!(
            cfg,
            Some(runtime::InspectConfig {
                host: "0.0.0.0".into(),
                port: 0,
                break_on_start: false,
            })
        );

        let port_only = parse_cli_for_test(&["wjsm", "--inspect=9230", "eval", "1"]);
        let cfg = port_only.inspect_config().expect("inspect parse");
        assert_eq!(
            cfg,
            Some(runtime::InspectConfig {
                host: "127.0.0.1".into(),
                port: 9230,
                break_on_start: false,
            })
        );
    }

    #[test]
    fn cli_inspect_brk_implies_break_on_start() {
        let brk = parse_cli_for_test(&["wjsm", "--inspect-brk", "eval", "1"]);
        let cfg = brk.inspect_config().expect("inspect-brk parse");
        assert_eq!(
            cfg,
            Some(runtime::InspectConfig {
                host: "127.0.0.1".into(),
                port: 9229,
                break_on_start: true,
            })
        );
        assert!(brk.wants_debug_codegen());

        // inspect-brk 优先于 inspect 的地址，并强制 break_on_start
        let both = parse_cli_for_test(&[
            "wjsm",
            "--inspect=127.0.0.1:1111",
            "--inspect-brk=127.0.0.1:2222",
            "eval",
            "1",
        ]);
        let cfg = both.inspect_config().expect("both flags");
        assert_eq!(
            cfg,
            Some(runtime::InspectConfig {
                host: "127.0.0.1".into(),
                port: 2222,
                break_on_start: true,
            })
        );
    }

    #[test]
    fn cli_browser_flag_enables_browser_condition() {
        let root = TestProject::new("browser_flag");
        write_file(&root, "package.json", r#"{"type":"module"}"#);
        write_file(
            &root,
            "main.js",
            "import { value } from 'pkg';\nconsole.log(value);\n",
        );
        write_file(
            &root,
            "node_modules/pkg/package.json",
            r#"{"type":"module","main":"node.js","browser":"browser.js"}"#,
        );
        write_file(
            &root,
            "node_modules/pkg/node.js",
            "export const other = 1;\n",
        );
        write_file(
            &root,
            "node_modules/pkg/browser.js",
            "export const value = 1;\n",
        );

        let default_cli = parse_cli_for_test(&[
            "wjsm",
            "check",
            "--root",
            root.to_str().expect("root should be UTF-8"),
            root.join("main.js")
                .to_str()
                .expect("input should be UTF-8"),
        ]);
        let default_error = execute(default_cli).expect_err("browser should be opt-in");
        let default_message = format!("{default_error:#}");
        assert!(
            default_message.contains("Missing export 'value'"),
            "{default_message}"
        );

        let browser_cli = parse_cli_for_test(&[
            "wjsm",
            "--browser",
            "check",
            "--root",
            root.to_str().expect("root should be UTF-8"),
            root.join("main.js")
                .to_str()
                .expect("input should be UTF-8"),
        ]);

        assert_eq!(
            execute(browser_cli).expect("browser flag should enable browser entry"),
            ExitCode::from(EXIT_SUCCESS)
        );
    }

    #[test]
    fn cli_condition_adds_custom_condition() {
        let root = TestProject::new("custom_condition");
        write_file(&root, "package.json", r#"{"type":"module"}"#);
        write_file(
            &root,
            "main.js",
            "import { value } from 'pkg';\nconsole.log(value);\n",
        );
        write_file(
            &root,
            "node_modules/pkg/package.json",
            r#"{"type":"module","exports":{".":{"custom":"./custom.js","default":"./default.js"}}}"#,
        );
        write_file(
            &root,
            "node_modules/pkg/custom.js",
            "export const value = 1;\n",
        );
        write_file(
            &root,
            "node_modules/pkg/default.js",
            "export const other = 1;\n",
        );

        let default_cli = parse_cli_for_test(&[
            "wjsm",
            "check",
            "--root",
            root.to_str().expect("root should be UTF-8"),
            root.join("main.js")
                .to_str()
                .expect("input should be UTF-8"),
        ]);
        let default_error = execute(default_cli).expect_err("custom condition should be opt-in");
        let default_message = format!("{default_error:#}");
        assert!(
            default_message.contains("Missing export 'value'"),
            "{default_message}"
        );

        let custom_cli = parse_cli_for_test(&[
            "wjsm",
            "--condition",
            "custom",
            "check",
            "--root",
            root.to_str().expect("root should be UTF-8"),
            root.join("main.js")
                .to_str()
                .expect("input should be UTF-8"),
        ]);

        assert_eq!(
            execute(custom_cli).expect("custom condition should select custom export"),
            ExitCode::from(EXIT_SUCCESS)
        );
    }
}
