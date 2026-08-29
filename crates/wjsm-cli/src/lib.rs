//! wjsm CLI：把 JS/TS 编成当前宿主的 generic native 并执行
//!
//! 静态模块在第一次执行前编译；`eval` 与 overlay 仍可能在运行时编译。
//!
//! Exit codes:
//! - 0: success
//! - 1: compile error (parse/lower/compile failure)
//! - 2: runtime error (native execution failure)
//! - 3: usage error (invalid arguments)
use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;
use wjsm_artifact_format::{
    ArtifactBuildInput, ArtifactLimits, BuildOptions, ManifestModule, ModuleKind, ModuleManifest,
    PortableArtifact,
};
use wjsm_ir::Program;
use wjsm_parser as parser;
use wjsm_semantic as semantic;

mod cli_args;
mod cli_cache;
mod cli_config;
mod cli_install;
mod cli_lint;
mod cli_scripts;
mod ir_output;
mod native_exec;
mod repl;

use cli_args::*;
use cli_config::parse_cli;
use cli_lint::lint_module;
use ir_output::{print_ir, print_ir_dot, print_ir_func, print_stats};

include!(concat!(env!("OUT_DIR"), "/cli_pipeline_hash.rs"));

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
// Pipeline Types
// ============================================================================
pub(crate) struct PipelineResult {
    pub(crate) ast: Option<swc_core::ecma::ast::Module>,
    pub(crate) program: Option<Program>,
    pub(crate) artifact: Option<PortableArtifact>,
    pub(crate) module_root: PathBuf,
    pub(crate) timings: PipelineTimings,
}
fn source_module_root(filename: Option<&str>) -> PathBuf {
    let candidate = filename
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|path| !path.as_os_str().is_empty());
    candidate
        .and_then(|path| path.canonicalize().ok())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Default)]
struct PipelineTimings {
    parse_us: u64,
    lower_us: u64,
    compile_us: u64,
    execute_us: u64,
}

/// `build` 子命令的输出与输入选项。
struct BuildArgs<'a> {
    input: &'a Option<PathBuf>,
    eval: &'a Option<String>,
    output: &'a Path,
    format: BuildFormat,
    stage: Option<Stage>,
    root: Option<&'a Path>,
    script: bool,
    include: &'a [PathBuf],
}

/// 源码身份：路径、逻辑 URL 与模块根。
struct SourceIdentity<'a> {
    source: &'a str,
    filename: Option<&'a str>,
    logical_url: &'a str,
    module_root: PathBuf,
}

impl<'a> SourceIdentity<'a> {
    fn from_source(source: &'a str, filename: Option<&'a str>) -> Self {
        Self {
            source,
            filename,
            logical_url: filename.unwrap_or("input.js"),
            module_root: source_module_root(filename),
        }
    }
}

/// 各 pipeline 阶段共享的编译开关。
#[derive(Clone, Copy, Default)]
struct PipelineFlags {
    script: bool,
    verify_ir: bool,
    debug_codegen: bool,
}

impl PipelineFlags {
    fn from_cli(cli: &Cli, script: bool) -> Self {
        Self {
            script,
            verify_ir: cli.should_verify_ir(),
            debug_codegen: cli.wants_debug_codegen(),
        }
    }
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
    cli.inspect_config().map_err(anyhow::Error::msg)?;
    // Handle color configuration
    setup_colors(cli.color, cli.no_color);

    match cli.command {
        Commands::Build {
            ref input,
            ref eval,
            ref output,
            format,
            stage,
            ref root,
            script,
            ref include,
        } => cmd_build(
            &cli,
            BuildArgs {
                input,
                eval,
                output,
                format,
                stage,
                root: root.as_deref(),
                script,
                include,
            },
        ),

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

        Commands::Validate { ref input } => cmd_validate(&cli, input),

        Commands::DumpClif {
            ref input,
            ref eval,
            ref root,
            script,
        } => cmd_dump_clif(&cli, input, eval, root.as_deref(), script),

        Commands::Disasm { ref input } => cmd_disasm(input),
        Commands::Size { ref input } => cmd_size(input),
        Commands::Cache { ref dir, command } => {
            cli_cache::run(command, dir.as_deref(), cli.quiet)?;
            Ok(ExitCode::from(EXIT_SUCCESS))
        }

        Commands::Fmt { ref input, write } => cmd_fmt(input, write),
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
    if let Ok(v) = std::env::var("CLICOLOR_FORCE")
        && !v.is_empty()
        && v != "0"
    {
        return true;
    }
    if let Ok(v) = std::env::var("NO_COLOR")
        && !v.is_empty()
    {
        return false;
    }
    io::stdout().is_terminal() || io::stderr().is_terminal()
}

fn cmd_build(cli: &Cli, args: BuildArgs<'_>) -> Result<ExitCode> {
    let BuildArgs {
        input,
        eval,
        output,
        format,
        stage,
        root,
        script,
        include,
    } = args;
    let flags = PipelineFlags::from_cli(cli, script);
    let stage = stage.unwrap_or(Stage::Compile);
    if format != BuildFormat::NativeExecutable && !include.is_empty() {
        bail!("`--include` is only valid with `--format native-executable`");
    }
    if format == BuildFormat::NativeExecutable {
        if !matches!(stage, Stage::Compile) {
            bail!("`--format native-executable` only supports `--stage compile`");
        }
        if path_is_stdout(output) {
            bail!("refusing to write a native executable to stdout; use `-o <path>`");
        }
        wjsm_exec_format::locate_exec_stub().context("failed to locate wjsm-exec stub")?;
        let (artifact_bytes, files) =
            compile_native_executable_artifact(cli, input, eval, root, script, include)?;
        native_exec::write_native_executable(&artifact_bytes, files, output)?;
        if cli.verbose_enabled(1) {
            eprintln!("Wrote native executable to {}", output.display());
        }
        return Ok(ExitCode::from(EXIT_SUCCESS));
    }

    if matches!(stage, Stage::Parse | Stage::Lower) && output != Path::new("out.wjsm") {
        bail!(
            "`-o` / `--output` cannot be used with `--stage parse` or `--stage lower` (output goes to stdout)"
        );
    }

    match stage {
        Stage::Parse | Stage::Lower => {
            let result = match resolve_input(input, eval)? {
                InputSource::Inline(code) => {
                    run_pipeline(&code, None, stage, cli.effective_verbose(), cli.time, flags)?
                }
                InputSource::File(path) => {
                    if path_is_stdin(&path) {
                        let (source, filename) = read_input_for_parse(&path)?;
                        run_pipeline(
                            &source,
                            filename.as_deref(),
                            stage,
                            cli.effective_verbose(),
                            cli.time,
                            flags,
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
                    "refusing to write binary artifact to a terminal; redirect stdout to a file or use `-o <path>`"
                );
            }

            if !cli.quiet
                && !path_is_stdout(output)
                && output == Path::new("out.wjsm")
                && output.exists()
            {
                eprintln!(
                    "warning: '{}' already exists and will be overwritten (use `-o` to choose another path)",
                    output.display()
                );
            }

            let artifact_bytes = compile_cli_artifact(cli, input, eval, root, script)?;

            if path_is_stdout(output) {
                io::stdout().write_all(&artifact_bytes)?;
            } else {
                fs::write(output, &artifact_bytes)?;
                if cli.verbose_enabled(1) {
                    eprintln!(
                        "Wrote {} bytes to {}",
                        artifact_bytes.len(),
                        output.display()
                    );
                }
            }

            if cli.stats {
                eprintln!("Output: {} bytes", artifact_bytes.len());
            }
        }
        Stage::Execute => {
            if path_is_stdout(output) && io::stdout().is_terminal() {
                bail!(
                    "refusing to write binary artifact to a terminal; redirect stdout to a file or use `-o <path>`"
                );
            }
            let result = match resolve_input(input, eval)? {
                InputSource::Inline(code) => {
                    compile_source_to_pipeline_result(&code, None, flags, cli.verbose_enabled(1))?
                }
                InputSource::File(path) => {
                    if path_is_stdin(&path) {
                        let (source, _) = read_input_for_parse(&path)?;
                        compile_source_to_pipeline_result(
                            &source,
                            None,
                            flags,
                            cli.verbose_enabled(1),
                        )?
                    } else {
                        compile_file_input_to_pipeline_result(
                            &path,
                            root,
                            None,
                            flags,
                            cli.verbose_enabled(1),
                            &module_resolution_options(cli),
                        )?
                    }
                }
            };
            let output_bytes = result
                .artifact
                .as_ref()
                .context("compile stage produced no portable artifact")?
                .bytes()
                .to_vec();
            if path_is_stdout(output) {
                io::stdout().write_all(&output_bytes)?;
            } else {
                fs::write(output, &output_bytes)?;
                if cli.verbose_enabled(1) {
                    eprintln!("Wrote {} bytes to {}", output_bytes.len(), output.display());
                }
            }
            return run_compile_then_execute(cli, result);
        }
    }

    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn compile_cli_artifact(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<Vec<u8>> {
    compile_cli_artifact_with_root(cli, input, eval, root, script).map(|(bytes, _)| bytes)
}

fn compile_cli_artifact_with_root(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<(Vec<u8>, PathBuf)> {
    match resolve_input(input, eval)? {
        InputSource::Inline(code) => {
            let module_root = resolved_module_root(source_module_root(None));
            let bytes = compile_source(&code, None, PipelineFlags::from_cli(cli, script))?;
            Ok((bytes, module_root))
        }
        InputSource::File(path) => {
            if path.extension().and_then(|extension| extension.to_str()) == Some("wjsm") {
                let bytes = fs::read(&path).with_context(|| {
                    format!("failed to read portable artifact '{}'", path.display())
                })?;
                let module_root = resolved_module_root(
                    root.map(Path::to_path_buf)
                        .or_else(|| path.parent().map(Path::to_path_buf))
                        .unwrap_or_else(|| PathBuf::from(".")),
                );
                Ok((bytes, module_root))
            } else if path_is_stdin(&path) {
                let (source, _) = read_input_for_parse(&path)?;
                let module_root = resolved_module_root(source_module_root(None));
                let bytes = compile_source(&source, None, PipelineFlags::from_cli(cli, script))?;
                Ok((bytes, module_root))
            } else {
                compile_from_file_input(
                    &path,
                    root,
                    PipelineFlags::from_cli(cli, script),
                    &module_resolution_options(cli),
                )
            }
        }
    }
}

fn resolved_module_root(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

type NativeExecutableOutput = (Vec<u8>, BTreeMap<String, Vec<u8>>);

fn compile_native_executable_artifact(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
    include: &[PathBuf],
) -> Result<NativeExecutableOutput> {
    let options = module_resolution_options(cli);
    let verify_ir = cli.should_verify_ir();
    let debug_codegen = cli.wants_debug_codegen();
    match resolve_input(input, eval)? {
        InputSource::Inline(code) => {
            let bytes = compile_source_with_identity(
                SourceIdentity {
                    source: &code,
                    filename: Some("/wjsm-exec/eval.js"),
                    logical_url: "eval.js",
                    module_root: PathBuf::from(wjsm_module::SNAPSHOT_VIRTUAL_ROOT),
                },
                PipelineFlags {
                    script,
                    verify_ir,
                    debug_codegen,
                },
            )?;
            let files = snapshot_inline_source(
                "eval.js",
                code.as_bytes(),
                include_root(root, None)?,
                include,
            )?;
            Ok((bytes, files))
        }
        InputSource::File(path) => {
            if path.extension().and_then(|extension| extension.to_str()) == Some("wjsm") {
                let bytes = fs::read(&path).with_context(|| {
                    format!("failed to read portable artifact '{}'", path.display())
                })?;
                let artifact = decode_artifact_bytes(bytes)?;
                let module_root = include_root(root, path.parent())?;
                let entry = portable_artifact_entry_path(&module_root, artifact.manifest())?;
                compile_native_executable_from_file(
                    &entry,
                    Some(module_root.as_path()),
                    script,
                    verify_ir,
                    debug_codegen,
                    &options,
                    include,
                )
            } else if path_is_stdin(&path) {
                let (source, _) = read_input_for_parse(&path)?;
                let bytes = compile_source_with_identity(
                    SourceIdentity {
                        source: &source,
                        filename: Some("/wjsm-exec/eval.js"),
                        logical_url: "eval.js",
                        module_root: PathBuf::from(wjsm_module::SNAPSHOT_VIRTUAL_ROOT),
                    },
                    PipelineFlags {
                        script,
                        verify_ir,
                        debug_codegen,
                    },
                )?;
                let files = snapshot_inline_source(
                    "eval.js",
                    source.as_bytes(),
                    include_root(root, None)?,
                    include,
                )?;
                Ok((bytes, files))
            } else {
                compile_native_executable_from_file(
                    &path,
                    root,
                    script,
                    verify_ir,
                    debug_codegen,
                    &options,
                    include,
                )
            }
        }
    }
}

fn include_root(root: Option<&Path>, fallback: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = root {
        return Ok(resolved_module_root(root.to_path_buf()));
    }
    if let Some(fallback) = fallback {
        return Ok(resolved_module_root(fallback.to_path_buf()));
    }
    std::env::current_dir().context("failed to determine include root")
}

fn snapshot_inline_source(
    logical_url: &str,
    source: &[u8],
    root: PathBuf,
    include: &[PathBuf],
) -> Result<BTreeMap<String, Vec<u8>>> {
    let store = wjsm_module::ModuleSourceStore::recording(&root);
    store.record_logical(logical_url, source.to_vec())?;
    finish_snapshot(store, include)
}

fn finish_snapshot(
    store: wjsm_module::ModuleSourceStore,
    include: &[PathBuf],
) -> Result<BTreeMap<String, Vec<u8>>> {
    apply_snapshot_includes(&store, include)?;
    wjsm_module::include_static_runtime_entries(&store)?;
    Ok(store.recorded_files())
}

/// `.wjsm` 只有主机身份 IR；打包必须从清单入口重新 lowering，才能写入虚拟身份与快照。
fn portable_artifact_entry_path(root: &Path, manifest: &ModuleManifest) -> Result<PathBuf> {
    let entry = manifest
        .modules
        .iter()
        .find(|module| module.id == manifest.entry)
        .ok_or_else(|| anyhow::anyhow!("portable artifact is missing its entry module"))?;
    if !manifest_module_has_disk_source(entry) {
        bail!(
            "portable artifact entry '{}' has no on-disk source",
            entry.logical_url
        );
    }
    let path = wjsm_module::logical_url_path(root, &entry.logical_url)?;
    if !path.is_file() {
        bail!(
            "failed to include artifact module '{}' under '{}'; pass --root to the original source tree",
            entry.logical_url,
            root.display()
        );
    }
    Ok(path)
}

fn manifest_module_has_disk_source(module: &ManifestModule) -> bool {
    module.kind != ModuleKind::Builtin && !module.logical_url.starts_with("node:")
}

fn apply_snapshot_includes(
    store: &wjsm_module::ModuleSourceStore,
    include: &[PathBuf],
) -> Result<()> {
    for path in include {
        store.include_file(path).with_context(|| {
            format!(
                "failed to include '{}' under module root '{}'",
                path.display(),
                store.root().display()
            )
        })?;
    }
    Ok(())
}

fn compile_native_executable_from_file(
    input: &Path,
    root: Option<&Path>,
    script: bool,
    verify_ir: bool,
    debug_codegen: bool,
    resolution_options: &wjsm_module::ResolutionOptions,
    include: &[PathBuf],
) -> Result<NativeExecutableOutput> {
    let plan = build_compile_plan(input, root)?;
    match plan {
        CompilePlan::Bundle { entry, root } => {
            let store = wjsm_module::ModuleSourceStore::recording(&root);
            let input = wjsm_module::lower_artifact_input_with_store(
                &entry,
                store.clone(),
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
            apply_snapshot_includes(&store, include)?;
            wjsm_module::include_static_runtime_entries(&store)?;
            let bytes = PortableArtifact::from_input(&input)
                .map_err(|error| anyhow::anyhow!("portable artifact encoding failed: {error}"))?
                .bytes()
                .to_vec();
            Ok((bytes, store.recorded_files()))
        }
        CompilePlan::SingleSource {
            source,
            source_path,
            module_root,
            ..
        } => {
            let store = wjsm_module::ModuleSourceStore::recording(&module_root);
            let _ = store.read_to_string(&source_path)?;
            let (filename, _, _) = store.module_identity(&source_path)?;
            let logical_url = store.logical_url(&source_path)?;
            let bytes = compile_source_with_identity(
                SourceIdentity {
                    source: &source,
                    filename: Some(filename.as_str()),
                    logical_url: &logical_url,
                    module_root: PathBuf::from(wjsm_module::SNAPSHOT_VIRTUAL_ROOT),
                },
                PipelineFlags {
                    script,
                    verify_ir,
                    debug_codegen,
                },
            )?;
            apply_snapshot_includes(&store, include)?;
            wjsm_module::include_static_runtime_entries(&store)?;
            Ok((bytes, store.recorded_files()))
        }
    }
}

fn decode_artifact(path: &Path) -> Result<PortableArtifact> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read portable artifact '{}'", path.display()))?;
    PortableArtifact::decode(bytes.into(), &ArtifactLimits::default())
        .map_err(|error| anyhow::anyhow!("invalid portable artifact: {error}"))
}

fn decode_artifact_bytes(bytes: Vec<u8>) -> Result<PortableArtifact> {
    PortableArtifact::decode(bytes.into(), &ArtifactLimits::default())
        .map_err(|error| anyhow::anyhow!("invalid portable artifact: {error}"))
}

fn cmd_validate(cli: &Cli, input: &Path) -> Result<ExitCode> {
    let artifact = decode_artifact(input)?;
    if !cli.quiet {
        println!("valid: {}", hex_digest(artifact.digest()));
    }
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_dump_clif(
    cli: &Cli,
    input: &Option<PathBuf>,
    eval: &Option<String>,
    root: Option<&Path>,
    script: bool,
) -> Result<ExitCode> {
    let bytes = compile_cli_artifact(cli, input, eval, root, script)?;
    let artifact = decode_artifact_bytes(bytes)?;
    let diagnostics = wjsm_backend_native::NativeCompiler::new()?.diagnostics(&artifact)?;
    print!("{}", diagnostics.clif);
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_disasm(input: &Path) -> Result<ExitCode> {
    let artifact = decode_artifact(input)?;
    let diagnostics = wjsm_backend_native::NativeCompiler::new()?.diagnostics(&artifact)?;
    print!("{}", diagnostics.disassembly);
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_size(input: &Path) -> Result<ExitCode> {
    let artifact = decode_artifact(input)?;
    let diagnostics = wjsm_backend_native::NativeCompiler::new()?.diagnostics(&artifact)?;
    let blocks = artifact
        .program()
        .functions()
        .iter()
        .map(|function| function.blocks().len())
        .sum::<usize>();
    let instructions = artifact
        .program()
        .functions()
        .iter()
        .flat_map(|function| function.blocks())
        .map(|block| block.instructions().len())
        .sum::<usize>();
    println!("artifact_bytes: {}", artifact.bytes().len());
    println!("sections: {}", artifact.metadata().sections.len());
    println!("ir_functions: {}", artifact.program().functions().len());
    println!("ir_blocks: {blocks}");
    println!("ir_instructions: {instructions}");
    println!("native_object_bytes: {}", diagnostics.object.bytes().len());
    println!("native_functions: {}", diagnostics.object.function_count());
    println!(
        "native_frame_bytes: {}",
        diagnostics
            .object
            .frame_bytes()
            .iter()
            .map(|bytes| u64::from(*bytes))
            .sum::<u64>()
    );
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn hex_digest(bytes: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
fn create_native_runtime(cli: &Cli) -> Result<wjsm_host_native::NativeRuntime> {
    let cache_dir = wjsm_module::resolve_cache_dir();
    let runtime_config = wjsm_host_native::NativeRuntimeConfig::from_environment(cache_dir)
        .map_err(anyhow::Error::msg)?
        .with_max_heap_size(cli.max_heap_size)
        .with_output_mode(wjsm_host_native::OutputMode::Inherit);
    let inspector = cli
        .inspect_config()
        .map_err(anyhow::Error::msg)?
        .map(|config| wjsm_host_native::InspectorConfig {
            host: config.host,
            port: config.port,
            break_on_start: config.break_on_start,
        });
    let runtime =
        wjsm_host_native::NativeRuntime::new_with_config_and_inspector(runtime_config, inspector)
            .context("failed to initialize native runtime")?;
    if let Some(url) = runtime.inspector_url() {
        eprintln!("Debugger listening on {url}");
    }
    Ok(runtime)
}

fn run_portable_artifact(
    cli: &Cli,
    input: &Path,
    root: Option<&Path>,
    script_args: &[OsString],
) -> Result<ExitCode> {
    let bytes = fs::read(input)
        .with_context(|| format!("failed to read portable artifact '{}'", input.display()))?;
    let artifact = PortableArtifact::decode(
        bytes.into(),
        &wjsm_artifact_format::ArtifactLimits::default(),
    )
    .map_err(|error| anyhow::anyhow!("invalid portable artifact: {error}"))?;
    let mut native = create_native_runtime(cli)?;
    native.configure_process_arguments(
        script_args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned()),
    )?;
    let module_root = root
        .map(Path::to_path_buf)
        .or_else(|| input.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let execution = native
        .execute(&artifact, &module_root, &working_directory)
        .context("portable artifact execution failed")?;
    if cli.stats {
        print_native_cache_stats(&execution);
    }
    Ok(ExitCode::from(execution.exit_code.rem_euclid(256) as u8))
}

fn cmd_run(
    cli: &Cli,
    input: &Path,
    root: Option<&Path>,
    script: bool,
    script_args: &[OsString],
) -> Result<ExitCode> {
    if input.extension().and_then(|extension| extension.to_str()) == Some("wjsm") {
        return run_portable_artifact(cli, input, root, script_args);
    }
    let verbose_compile = cli.verbose_enabled(1);
    let result = if path_is_stdin(input) {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        compile_source_to_pipeline_result(
            &source,
            None,
            PipelineFlags::from_cli(cli, script),
            verbose_compile,
        )?
    } else {
        compile_file_input_to_pipeline_result(
            input,
            root,
            None,
            PipelineFlags::from_cli(cli, script),
            verbose_compile,
            &module_resolution_options(cli),
        )?
    };

    run_compile_then_execute_with_args(cli, result, script_args)
}

fn cmd_run_eval(
    cli: &Cli,
    code: &str,
    script: bool,
    _mode_tag: &str,
    _script_args: &[OsString],
) -> Result<ExitCode> {
    let result = compile_source_to_pipeline_result(
        code,
        None,
        PipelineFlags::from_cli(cli, script),
        cli.verbose_enabled(1),
    )?;
    run_compile_then_execute_with_args(cli, result, _script_args)
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
            PipelineFlags::from_cli(cli, script),
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
                    PipelineFlags::from_cli(cli, script),
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
    crate::repl::run(eval, |source| {
        if script {
            cmd_run_eval(cli, source, true, "[repl]", &[])
        } else {
            cmd_eval(cli, source)
        }
    })
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
            PipelineFlags::from_cli(cli, script),
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
                    PipelineFlags::from_cli(cli, script),
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
            PipelineFlags::from_cli(cli, script),
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
                    PipelineFlags::from_cli(cli, script),
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
            PipelineFlags::from_cli(cli, script),
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
                    PipelineFlags::from_cli(cli, script),
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

fn cmd_completions(shell: clap_complete::Shell) -> Result<ExitCode> {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, bin_name, &mut io::stdout());
    Ok(ExitCode::from(EXIT_SUCCESS))
}

fn cmd_init(path: &Path, force: bool) -> Result<ExitCode> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid path"))?
        .to_string_lossy();
    fs::create_dir_all(path)?;
    let main_path = path.join("main.js");
    let package_path = path.join("package.json");
    for file_path in [&main_path, &package_path] {
        if file_path.exists() && !force {
            bail!(
                "'{}' already exists. Use --force to overwrite.",
                file_path.display()
            );
        }
    }
    fs::write(
        &main_path,
        format!(
            "// {} - wjsm project\nconsole.log(\"Hello from {}!\");\n",
            name, name
        ),
    )?;
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
    }
    Ok(ExitCode::from(EXIT_SUCCESS))
}

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
fn build_portable_artifact(
    program: &Program,
    logical_url: impl Into<String>,
    script: bool,
    include_source_map: bool,
    source: Option<&str>,
) -> Result<PortableArtifact> {
    PortableArtifact::from_input(&ArtifactBuildInput {
        program: Arc::new(program.clone()),
        manifest: Arc::new(ModuleManifest::single(logical_url, script)),
        options: BuildOptions {
            include_source_map,
            include_source_text: include_source_map,
        },
        source_text: include_source_map
            .then(|| source.map(Arc::<str>::from))
            .flatten(),
    })
    .map_err(|error| anyhow::anyhow!("portable artifact encoding failed: {error}"))
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
    flags: PipelineFlags,
) -> Result<PipelineResult> {
    run_pipeline_with_identity(
        SourceIdentity::from_source(source, filename),
        stop_at,
        verbose,
        time,
        flags,
    )
}

fn run_pipeline_with_identity(
    identity: SourceIdentity<'_>,
    stop_at: Stage,
    verbose: u8,
    time: bool,
    flags: PipelineFlags,
) -> Result<PipelineResult> {
    let SourceIdentity {
        source,
        filename,
        logical_url,
        module_root,
    } = identity;
    let PipelineFlags {
        script,
        verify_ir,
        debug_codegen,
    } = flags;
    let mut result = PipelineResult {
        ast: None,
        program: None,
        artifact: None,
        module_root,
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
    let mut program = lower_parsed_module(
        source,
        filename,
        result.ast.take().unwrap(),
        script,
        verify_ir,
        debug_codegen,
    )?;
    program.set_source_file(logical_url);
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
        eprintln!("Compiling portable artifact...");
    }
    let start = Instant::now();
    let Some(program) = result.program.as_ref() else {
        bail!("lower stage produced no program");
    };
    result.artifact = Some(build_portable_artifact(
        program,
        logical_url,
        script,
        debug_codegen,
        Some(source),
    )?);
    result.timings.compile_us = start.elapsed().as_micros() as u64;

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
                artifact: None,
                module_root: root.clone(),
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
                    let program = wjsm_module::lower_bundle_cached_with_options(
                        &entry,
                        &root,
                        resolution_options.clone(),
                    )?;
                    verify_ir_for_pipeline(&program, cli.should_verify_ir())?;
                    result.timings.lower_us = start.elapsed().as_micros() as u64;
                    result.program = Some(program);
                }
                Stage::Compile | Stage::Execute => {
                    let input = lower_bundle_artifact_input(
                        &entry,
                        &root,
                        &resolution_options,
                        cli.wants_debug_codegen(),
                        None,
                    )?;
                    result.artifact =
                        Some(PortableArtifact::from_input(&input).map_err(|error| {
                            anyhow::anyhow!("portable artifact encoding failed: {error}")
                        })?);
                    result.timings.compile_us = start.elapsed().as_micros() as u64;
                }
            }
            if cli.time {
                result.timings.print(cli.effective_verbose());
            }
            Ok(result)
        }
        CompilePlan::SingleSource {
            source,
            filename,
            logical_url,
            source_path: _,
            module_root,
        } => run_pipeline_with_identity(
            SourceIdentity {
                source: &source,
                filename: Some(filename.as_str()),
                logical_url: &logical_url,
                module_root,
            },
            stop_at,
            cli.effective_verbose(),
            cli.time,
            PipelineFlags::from_cli(cli, script),
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
    Bundle {
        entry: PathBuf,
        root: PathBuf,
    },
    SingleSource {
        source: String,
        filename: String,
        logical_url: String,
        source_path: PathBuf,
        module_root: PathBuf,
    },
}

fn build_compile_plan(input: &Path, root: Option<&Path>) -> Result<CompilePlan> {
    if let Some(root_path) = root {
        return bundle_plan_from_root(input, root_path);
    }

    let source = read_source_file(input)?;
    let module = parser::parse_module_with_path(&source, input)?;
    let is_esm = wjsm_module::is_es_module(&module);
    let is_cjs = wjsm_module::is_commonjs_module(&module);

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

    if !is_esm && !is_cjs {
        return Ok(CompilePlan::SingleSource {
            source,
            filename: path_to_diagnostic_filename(&canonical_input),
            logical_url: wjsm_module::logical_url_from_path(Path::new(file_name))?,
            source_path: canonical_input.clone(),
            module_root: parent.to_path_buf(),
        });
    }

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

fn run_compile_then_execute(cli: &Cli, result: PipelineResult) -> Result<ExitCode> {
    run_compile_then_execute_with_args(cli, result, &[])
}

fn run_compile_then_execute_with_args(
    cli: &Cli,
    mut result: PipelineResult,
    script_args: &[OsString],
) -> Result<ExitCode> {
    let artifact = result
        .artifact
        .as_ref()
        .context("compile stage produced no portable artifact")?;
    if cli.stats {
        print_stats(&result);
        eprintln!("Artifact: {} bytes", artifact.bytes().len());
    }
    let mut native = create_native_runtime(cli)?;
    native.configure_process_arguments(
        script_args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned()),
    )?;
    let start = Instant::now();
    let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let execution = match native.execute(artifact, &result.module_root, &working_directory) {
        Ok(execution) => execution,
        Err(wjsm_host_native::NativeRuntimeError::FatalJavaScript(message)) => {
            eprintln!("{message}");
            return Ok(ExitCode::from(EXIT_COMPILE_ERROR));
        }
        Err(error) => {
            eprintln!("Runtime error: {error:#}");
            return Ok(ExitCode::from(EXIT_RUNTIME_ERROR));
        }
    };
    result.timings.execute_us = start.elapsed().as_micros() as u64;
    if cli.stats {
        print_native_cache_stats(&execution);
    }
    if cli.time {
        result.timings.print(cli.effective_verbose());
    }
    Ok(ExitCode::from(execution.exit_code.rem_euclid(256) as u8))
}

fn print_native_cache_stats(execution: &wjsm_host_native::NativeExecution) {
    eprintln!(
        "Native cache: entries={}, bytes={}, hits={}, misses={}, invalidated={}",
        execution.cache_entries,
        execution.cache_bytes,
        execution.cache_hit_count,
        execution.cache_miss_count,
        execution.cache_invalidated_count,
    );
}

fn compile_source_to_pipeline_result(
    source: &str,
    filename: Option<&str>,
    flags: PipelineFlags,
    verbose: bool,
) -> Result<PipelineResult> {
    compile_source_to_pipeline_result_with_identity(
        SourceIdentity::from_source(source, filename),
        flags,
        verbose,
    )
}

fn compile_source_to_pipeline_result_with_identity(
    identity: SourceIdentity<'_>,
    flags: PipelineFlags,
    verbose: bool,
) -> Result<PipelineResult> {
    let SourceIdentity {
        source,
        filename,
        logical_url,
        module_root,
    } = identity;
    let PipelineFlags {
        script,
        verify_ir,
        debug_codegen,
    } = flags;
    let mut result = PipelineResult {
        ast: None,
        program: None,
        artifact: None,
        module_root,
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
    let mut program = lower_parsed_module(
        source,
        filename,
        result.ast.take().unwrap(),
        script,
        verify_ir,
        debug_codegen,
    )?;
    program.set_source_file(logical_url);
    result.timings.lower_us = start.elapsed().as_micros() as u64;
    result.program = Some(program);

    if verbose {
        eprintln!("Compiling portable artifact...");
    }
    result.artifact = Some(build_portable_artifact(
        result
            .program
            .as_ref()
            .context("lower stage produced no program")?,
        logical_url,
        script,
        debug_codegen,
        Some(source),
    )?);

    Ok(result)
}

/// 输入寻址 artifact 缓存的管线盐：CLI 源码指纹 + 影响 artifact 字节的编译开关
/// （script/verify-ir/debug）。任一变化都会切换缓存命名空间（issue #376）。
fn artifact_pipeline_salt(flags: PipelineFlags) -> Vec<u8> {
    let mut salt = Vec::with_capacity(35);
    salt.extend_from_slice(&CLI_PIPELINE_SOURCE_HASH);
    salt.push(u8::from(flags.script));
    salt.push(u8::from(flags.verify_ir));
    salt.push(u8::from(flags.debug_codegen));
    salt
}

/// 命中路径：解码缓存的 portable artifact，parse/lower 完全跳过。
/// 解码失败按 miss 处理（冷路径重编译并覆盖写入）。
fn load_cached_pipeline_result(
    request: &wjsm_module::ArtifactCacheRequest,
    verbose: bool,
) -> Option<PipelineResult> {
    let hit = wjsm_module::lookup_portable_artifact(request)?;
    let artifact =
        PortableArtifact::decode(hit.artifact_bytes.into(), &ArtifactLimits::default()).ok()?;
    if verbose {
        eprintln!("Loaded portable artifact from input-addressed cache (parse/lower skipped)");
    }
    Some(PipelineResult {
        ast: None,
        program: None,
        artifact: Some(artifact),
        module_root: hit.module_root,
        timings: PipelineTimings::default(),
    })
}

fn compile_file_input_to_pipeline_result(
    input: &Path,
    root: Option<&Path>,
    logical_root: Option<&Path>,
    flags: PipelineFlags,
    verbose: bool,
    resolution_options: &wjsm_module::ResolutionOptions,
) -> Result<PipelineResult> {
    // 输入寻址缓存：入口 canonical 身份在 parse 前可得，命中即跳过 parse/lower。
    let cache_request = wjsm_module::ArtifactCacheRequest::for_entry(
        input,
        root,
        logical_root,
        resolution_options,
        &artifact_pipeline_salt(flags),
    );
    if let Some(request) = &cache_request
        && let Some(result) = load_cached_pipeline_result(request, verbose)
    {
        return Ok(result);
    }
    let trace = Arc::new(wjsm_module::SourceReadTrace::default());

    let plan = build_compile_plan(input, root)?;
    let result = match plan {
        CompilePlan::Bundle { entry, root } => {
            if verbose {
                eprintln!("Bundling modules...");
            }
            let start = Instant::now();
            let mut result = PipelineResult {
                ast: None,
                program: None,
                artifact: None,
                module_root: root.clone(),
                timings: PipelineTimings::default(),
            };
            let input = lower_bundle_artifact_input(
                &entry,
                &root,
                resolution_options,
                flags.debug_codegen,
                Some(Arc::clone(&trace)),
            )?;
            result.artifact =
                Some(PortableArtifact::from_input(&input).map_err(|error| {
                    anyhow::anyhow!("portable artifact encoding failed: {error}")
                })?);
            result.timings.compile_us = start.elapsed().as_micros() as u64;
            result
        }
        CompilePlan::SingleSource {
            source,
            filename,
            logical_url,
            source_path,
            module_root,
        } => {
            // 单文件计划不经 store：入口内容事实手动入读集（lossy 解码改变过
            // 字节的文件回放必然 miss——宁可永不命中，不可脏命中）。
            trace.record_content(&source_path, source.as_bytes());
            let (logical_url, module_root) = if let Some(root) = logical_root {
                let root = root.canonicalize().with_context(|| {
                    format!("Failed to canonicalize logical root '{}'", root.display())
                })?;
                let relative = source_path.strip_prefix(&root).map_err(|_| {
                    anyhow::anyhow!(
                        "input file '{}' is not under logical root '{}'",
                        source_path.display(),
                        root.display()
                    )
                })?;
                (wjsm_module::logical_url_from_path(relative)?, root)
            } else {
                (logical_url, module_root)
            };
            compile_source_to_pipeline_result_with_identity(
                SourceIdentity {
                    source: &source,
                    filename: Some(filename.as_str()),
                    logical_url: &logical_url,
                    module_root,
                },
                flags,
                verbose,
            )?
        }
    };

    if let (Some(request), Some(artifact)) = (&cache_request, result.artifact.as_ref()) {
        wjsm_module::store_portable_artifact(
            request,
            &trace,
            &result.module_root,
            artifact.bytes(),
        );
    }
    Ok(result)
}

fn compile_from_file_input(
    input: &Path,
    root: Option<&Path>,
    flags: PipelineFlags,
    resolution_options: &wjsm_module::ResolutionOptions,
) -> Result<(Vec<u8>, PathBuf)> {
    let result =
        compile_file_input_to_pipeline_result(input, root, None, flags, false, resolution_options)?;
    let bytes = result
        .artifact
        .as_ref()
        .context("compile stage produced no portable artifact")?
        .bytes()
        .to_vec();
    Ok((bytes, resolved_module_root(result.module_root)))
}

fn lower_bundle_artifact_input(
    entry: &Path,
    root: &Path,
    resolution_options: &wjsm_module::ResolutionOptions,
    debug_codegen: bool,
    trace: Option<Arc<wjsm_module::SourceReadTrace>>,
) -> Result<ArtifactBuildInput> {
    // 带读集追踪的磁盘 store 与普通磁盘 store 走同一 lower 路径，
    // 只多记录文件系统事实供 artifact 缓存回放校验。
    let store = match trace {
        Some(trace) => wjsm_module::ModuleSourceStore::disk_traced(root, trace),
        None => wjsm_module::ModuleSourceStore::disk(root),
    };
    wjsm_module::lower_artifact_input_with_store(
        entry,
        store,
        resolution_options.clone(),
        debug_codegen,
    )
    .with_context(|| {
        format!(
            "bundle entry {} from root {}",
            entry.display(),
            root.display()
        )
    })
}

fn compile_source_with_identity(
    identity: SourceIdentity<'_>,
    flags: PipelineFlags,
) -> Result<Vec<u8>> {
    let result = compile_source_to_pipeline_result_with_identity(identity, flags, false)?;
    Ok(result
        .artifact
        .context("compile stage produced no portable artifact")?
        .bytes()
        .to_vec())
}

fn compile_source(source: &str, filename: Option<&str>, flags: PipelineFlags) -> Result<Vec<u8>> {
    compile_source_with_identity(SourceIdentity::from_source(source, filename), flags)
}
thread_local! {
    /// Thread-local runtime 实例，供所有 in-process 测试路径共享。
    /// 每次 execute 前自动恢复 startup snapshot，确保测试隔离。
    static IN_PROCESS_RUNTIME: RefCell<Option<wjsm_host_native::NativeRuntime>> =
        const { RefCell::new(None) };
}

/// In-process 复现 `wjsm run <file>` 的可观测行为（stdout / stderr / exit_code），
/// 供 E2E fixture 测试在测试进程内直接调用，免去每个 fixture spawn 一个 wjsm 子进程
/// （省一层进程 + 510MB ELF 加载）。
///
/// 退出码 / stderr 契约必须与 `main_entry` + `cmd_run` 逐字一致：
/// - 编译错（parse/lower/bundle/compile）→ 退出码 1，stderr = `Error: {e:#}\n`
/// - 运行时错（native 执行失败）→ 退出码 2，stderr = `Runtime error: {e:#}\n`
/// - 成功 → 退出码 0，stdout 为程序输出，stderr 为空。
///
/// 偏离 CLI 的唯一点：stdout 写入返回的 buffer 而非真实 fd（测试需捕获）。
/// In-process 复现 `wjsm run <file>` 的可观测行为（stdout / stderr / exit_code）。
///
/// 测试入口与 CLI 共用同一 portable artifact、native cache 与 `NativeRuntime`，
/// 避免测试进程绕过生产执行路径。
pub fn run_file_in_process(input: &Path) -> (i32, Vec<u8>, Vec<u8>) {
    run_file_in_process_with_options(input, &[], &[], None)
}

/// 在显式 portable build root 下执行 fixture 文件。
pub fn run_file_in_process_with_root(input: &Path, root: &Path) -> (i32, Vec<u8>, Vec<u8>) {
    run_file_in_process_with_options_and_root(input, Some(root), &[], &[], None)
}

/// 同上，附加进程环境覆盖：fixture runner 经 `WJSM_TEST_STDIN` 注入确定性
/// stdin 内容（进程内运行无法用真实管道，钩子见 dispatch/process_stdin.rs）。
pub fn run_file_in_process_with_root_and_env(
    input: &Path,
    root: &Path,
    env_overrides: &[(&str, &str)],
) -> (i32, Vec<u8>, Vec<u8>) {
    run_file_in_process_with_options_and_root(input, Some(root), &[], env_overrides, None)
}

/// 在测试进程内执行一段 source，使用与 CLI 相同的 portable artifact/native runtime 路径。
pub fn run_source_in_process(source: &str) -> (i32, Vec<u8>, Vec<u8>) {
    run_source_in_process_with_flags(source, PipelineFlags::default())
}

/// 在测试进程内以脚本模式执行一段 source（等价 `run --script -e`）：
/// 顶层声明走全局环境记录（GDI），供脚本全局语义集成测试使用。
pub fn run_script_source_in_process(source: &str) -> (i32, Vec<u8>, Vec<u8>) {
    run_source_in_process_with_flags(
        source,
        PipelineFlags {
            script: true,
            ..PipelineFlags::default()
        },
    )
}

/// 同 [`run_source_in_process`]，但用显式 module root 模拟从该目录执行
/// `run -e`：无文件入口的动态 import/require 相对说明符以 root 为解析基址。
pub fn run_source_in_process_with_root(source: &str, root: &Path) -> (i32, Vec<u8>, Vec<u8>) {
    let artifact =
        match compile_source_to_pipeline_result(source, None, PipelineFlags::default(), false)
            .and_then(|result| {
                result
                    .artifact
                    .context("compile stage produced no portable artifact")
            }) {
            Ok(artifact) => artifact,
            Err(error) => {
                return (
                    EXIT_COMPILE_ERROR as i32,
                    Vec::new(),
                    format!("Error: {error:#}\n").into_bytes(),
                );
            }
        };
    execute_artifact_in_process(&artifact, root)
}

fn run_source_in_process_with_flags(source: &str, flags: PipelineFlags) -> (i32, Vec<u8>, Vec<u8>) {
    let (artifact, module_root) =
        match compile_source_to_pipeline_result(source, None, flags, false).and_then(|result| {
            let artifact = result
                .artifact
                .context("compile stage produced no portable artifact")?;
            Ok((artifact, result.module_root))
        }) {
            Ok(compiled) => compiled,
            Err(error) => {
                return (
                    EXIT_COMPILE_ERROR as i32,
                    Vec::new(),
                    format!("Error: {error:#}\n").into_bytes(),
                );
            }
        };
    execute_artifact_in_process(&artifact, &module_root)
}

/// 通用的 artifact 执行入口，支持配置 env/args/cwd 并重用 thread_local runtime。
/// 无条件重置 env/args 确保测试隔离（避免前一个测试的配置污染后续测试）。
fn execute_artifact_in_process_with_config(
    artifact: &PortableArtifact,
    module_root: &Path,
    script_args: &[&str],
    env_overrides: &[(&str, &str)],
    cwd_override: Option<&Path>,
) -> (i32, Vec<u8>, Vec<u8>) {
    IN_PROCESS_RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if runtime.is_none() {
            let cache_dir = wjsm_module::resolve_cache_dir();
            match wjsm_host_native::NativeRuntime::new(cache_dir) {
                Ok(created) => *runtime = Some(created),
                Err(error) => {
                    return (
                        EXIT_RUNTIME_ERROR.into(),
                        Vec::new(),
                        format!("Runtime error: {error}\n").into_bytes(),
                    );
                }
            }
        }
        let Some(native) = runtime.as_mut() else {
            unreachable!("runtime was initialized above")
        };

        // 无条件重置 env 和 args，确保每个测试都从干净状态开始
        if let Err(error) = native.configure_environment(
            true,
            env_overrides
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        ) {
            return runtime_error_result(native, EXIT_RUNTIME_ERROR, error);
        }
        if let Err(error) = native
            .configure_process_arguments(script_args.iter().map(|argument| (*argument).to_owned()))
        {
            return runtime_error_result(native, EXIT_RUNTIME_ERROR, error);
        }

        let working_directory = cwd_override
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        match native.execute(artifact, module_root, &working_directory) {
            Ok(execution) => (execution.exit_code, execution.stdout, execution.stderr),
            Err(wjsm_host_native::NativeRuntimeError::FatalJavaScript(message)) => {
                runtime_error_result(native, EXIT_COMPILE_ERROR, message)
            }
            Err(error) => runtime_error_result(
                native,
                EXIT_RUNTIME_ERROR,
                format!("Runtime error: {error}"),
            ),
        }
    })
}

fn execute_artifact_in_process(
    artifact: &PortableArtifact,
    module_root: &Path,
) -> (i32, Vec<u8>, Vec<u8>) {
    execute_artifact_in_process_with_config(artifact, module_root, &[], &[], None)
}

fn runtime_error_result(
    native: &mut wjsm_host_native::NativeRuntime,
    exit_code: u8,
    message: impl std::fmt::Display,
) -> (i32, Vec<u8>, Vec<u8>) {
    let mut stderr = native.take_stderr();
    stderr.extend_from_slice(format!("{message}\n").as_bytes());
    (exit_code.into(), native.take_output(), stderr)
}

pub fn run_file_in_process_with_options(
    input: &Path,
    script_args: &[&str],
    env_overrides: &[(&str, &str)],
    cwd_override: Option<&Path>,
) -> (i32, Vec<u8>, Vec<u8>) {
    run_file_in_process_with_options_and_root(input, None, script_args, env_overrides, cwd_override)
}

fn run_file_in_process_with_options_and_root(
    input: &Path,
    root: Option<&Path>,
    script_args: &[&str],
    env_overrides: &[(&str, &str)],
    cwd_override: Option<&Path>,
) -> (i32, Vec<u8>, Vec<u8>) {
    let (artifact, module_root) = match compile_file_input_to_pipeline_result(
        input,
        None,
        root,
        PipelineFlags::default(),
        false,
        &wjsm_module::ResolutionOptions::default(),
    )
    .and_then(|result| {
        let artifact = result
            .artifact
            .context("compile stage produced no portable artifact")?;
        Ok((artifact, result.module_root))
    }) {
        Ok(compiled) => compiled,
        Err(error) => {
            return (
                EXIT_COMPILE_ERROR as i32,
                Vec::new(),
                format!("Error: {error:#}\n").into_bytes(),
            );
        }
    };

    execute_artifact_in_process_with_config(
        &artifact,
        &module_root,
        script_args,
        env_overrides,
        cwd_override,
    )
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
            let path = std::env::temp_dir()
                .join("wjsm-test-cache")
                .join("cli")
                .join(format!("browser-{case}-{}-{id}", std::process::id()));
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
            Some(InspectConfig {
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
            Some(InspectConfig {
                host: "0.0.0.0".into(),
                port: 0,
                break_on_start: false,
            })
        );

        let port_only = parse_cli_for_test(&["wjsm", "--inspect=9230", "eval", "1"]);
        let cfg = port_only.inspect_config().expect("inspect parse");
        assert_eq!(
            cfg,
            Some(InspectConfig {
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
            Some(InspectConfig {
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
            Some(InspectConfig {
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
    /// 独占设置 `WJSM_CACHE_DIR` 的测试守卫；drop 时恢复原值。
    /// nextest 每个测试独立进程，进程内无并发写。
    struct CacheDirGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl CacheDirGuard {
        fn set(dir: &Path) -> Self {
            let previous = std::env::var_os("WJSM_CACHE_DIR");
            // SAFETY: 本测试进程独占该环境变量，结束时恢复。
            unsafe { std::env::set_var("WJSM_CACHE_DIR", dir) };
            Self { previous }
        }
    }

    impl Drop for CacheDirGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                // SAFETY: 恢复进入测试前的值。
                Some(value) => unsafe { std::env::set_var("WJSM_CACHE_DIR", value) },
                None => unsafe { std::env::remove_var("WJSM_CACHE_DIR") },
            }
        }
    }

    fn artifact_bytes(result: &PipelineResult) -> Vec<u8> {
        result
            .artifact
            .as_ref()
            .expect("compile should produce an artifact")
            .bytes()
            .to_vec()
    }

    #[test]
    fn artifact_cache_hits_skip_parse_lower_and_invalidate_on_edit() {
        let root = TestProject::new("artifact_cache_single");
        let cache_dir = root.join("cache");
        let _guard = CacheDirGuard::set(&cache_dir);
        write_file(&root, "main.js", "console.log(40 + 2);\n");
        let main = root.join("main.js");
        let options = wjsm_module::ResolutionOptions::default();
        let flags = PipelineFlags::default();

        // 冷路径：单文件计划走 parse/lower，program 保留在结果里。
        let cold = compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
            .expect("cold compile should succeed");
        assert!(cold.program.is_some(), "冷路径应经过 lower");
        let cold_bytes = artifact_bytes(&cold);

        // 冷路径落盘后，输入寻址查找必须命中。
        let request = wjsm_module::ArtifactCacheRequest::for_entry(
            &main,
            None,
            None,
            &options,
            &artifact_pipeline_salt(flags),
        )
        .expect("入口应可 canonical 化");
        assert!(
            wjsm_module::lookup_portable_artifact(&request).is_some(),
            "首次编译后缓存应命中"
        );

        // 命中路径：parse/lower 完全跳过（program 为 None），字节与冷路径一致。
        let hit = compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
            .expect("cached compile should succeed");
        assert!(hit.program.is_none(), "命中路径不得经过 lower");
        assert_eq!(artifact_bytes(&hit), cold_bytes, "命中字节必须与冷路径一致");

        // 编辑源文件 → 内容事实失效 → miss（重新 lower），产物随之更新。
        write_file(&root, "main.js", "console.log(43);\n");
        let edited =
            compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
                .expect("recompile should succeed");
        assert!(edited.program.is_some(), "源码编辑后必须重新 lower");
        assert_ne!(artifact_bytes(&edited), cold_bytes);

        // 恢复原内容：.dep 索引仍指向编辑版读集 → 回放失败 miss 一次
        // （宁可 miss 不可脏命中），重编译后索引指回原 content key。
        write_file(&root, "main.js", "console.log(40 + 2);\n");
        let restored =
            compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
                .expect("restored compile should succeed");
        assert!(
            restored.program.is_some(),
            "索引被编辑版覆盖后应 miss 重编译"
        );
        assert_eq!(artifact_bytes(&restored), cold_bytes);

        // miss 重编译已把索引写回原读集 → 再次编译重新命中。
        let rehit =
            compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
                .expect("re-hit compile should succeed");
        assert!(rehit.program.is_none(), "索引恢复后应重新命中");
        assert_eq!(artifact_bytes(&rehit), cold_bytes);
    }

    #[test]
    fn artifact_cache_bundle_invalidates_on_dependency_and_probe_changes() {
        let root = TestProject::new("artifact_cache_bundle");
        let cache_dir = root.join("cache");
        let _guard = CacheDirGuard::set(&cache_dir);
        write_file(&root, "package.json", r#"{"type":"module"}"#);
        write_file(
            &root,
            "main.js",
            "import { value } from './lib.js';\nconsole.log(value);\n",
        );
        write_file(&root, "lib.js", "export const value = 1;\n");
        let main = root.join("main.js");
        let options = wjsm_module::ResolutionOptions::default();
        let flags = PipelineFlags::default();

        let cold = compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
            .expect("cold bundle compile should succeed");
        let cold_bytes = artifact_bytes(&cold);
        let hit = compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
            .expect("cached bundle compile should succeed");
        assert_eq!(artifact_bytes(&hit), cold_bytes);

        // 依赖（非入口）编辑必须失效——读集覆盖整个源码闭包。
        write_file(&root, "lib.js", "export const value = 2;\n");
        let edited =
            compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
                .expect("dependency edit recompile should succeed");
        assert_ne!(artifact_bytes(&edited), cold_bytes, "依赖编辑后不得脏命中");

        // 恢复依赖 → 重新命中。
        write_file(&root, "lib.js", "export const value = 1;\n");
        let restored =
            compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
                .expect("restored bundle compile should succeed");
        assert_eq!(artifact_bytes(&restored), cold_bytes);

        // 解析探测事实：曾以 .js 命中的说明符出现同名 .ts 不影响解析结果，
        // 但 package.json（解析输入之一）变化必须失效。
        write_file(
            &root,
            "package.json",
            r#"{"type":"module","sideEffects":false}"#,
        );
        let request = wjsm_module::ArtifactCacheRequest::for_entry(
            &main,
            None,
            None,
            &options,
            &artifact_pipeline_salt(flags),
        )
        .expect("入口应可 canonical 化");
        assert!(
            wjsm_module::lookup_portable_artifact(&request).is_none(),
            "package.json 变化后必须 miss"
        );
    }

    #[test]
    fn artifact_cache_respects_no_builtin_cache_switch() {
        let root = TestProject::new("artifact_cache_switch");
        let cache_dir = root.join("cache");
        let _guard = CacheDirGuard::set(&cache_dir);
        write_file(&root, "main.js", "console.log(1);\n");
        let main = root.join("main.js");
        let options = wjsm_module::ResolutionOptions::default();
        let flags = PipelineFlags::default();

        compile_file_input_to_pipeline_result(&main, None, None, flags, false, &options)
            .expect("cold compile should succeed");
        let request = wjsm_module::ArtifactCacheRequest::for_entry(
            &main,
            None,
            None,
            &options,
            &artifact_pipeline_salt(flags),
        )
        .expect("入口应可 canonical 化");
        assert!(wjsm_module::lookup_portable_artifact(&request).is_some());

        // WJSM_NO_BUILTIN_CACHE 强制冷 lower 调试路径：artifact 缓存一并停用。
        // SAFETY: 本测试进程独占该环境变量，结束时清除。
        unsafe { std::env::set_var("WJSM_NO_BUILTIN_CACHE", "1") };
        let gated = wjsm_module::lookup_portable_artifact(&request);
        unsafe { std::env::remove_var("WJSM_NO_BUILTIN_CACHE") };
        assert!(gated.is_none(), "WJSM_NO_BUILTIN_CACHE 下不得命中");
    }

    #[test]
    fn cli_validates_artifacts_and_rejects_corruption() {
        let root = TestProject::new("validate_artifact");
        let artifact = root.join("input.wjsm");
        fs::write(
            &artifact,
            compile_source(
                "console.log(1)",
                None,
                PipelineFlags {
                    verify_ir: true,
                    ..PipelineFlags::default()
                },
            )
            .expect("source should compile"),
        )
        .expect("artifact should be writable");
        let input = artifact.to_str().expect("artifact path should be UTF-8");
        let valid = parse_cli_for_test(&["wjsm", "validate", input]);
        assert_eq!(
            execute(valid).expect("valid artifact should pass"),
            ExitCode::from(EXIT_SUCCESS)
        );

        fs::write(&artifact, b"corrupt").expect("artifact should be corruptible");
        let corrupt = parse_cli_for_test(&["wjsm", "validate", input]);
        let message = format!(
            "{:#}",
            execute(corrupt).expect_err("corrupt artifact should fail")
        );
        assert!(message.contains("invalid portable artifact"), "{message}");
    }

    #[test]
    fn native_executable_worker_file_collection_contracts() {
        let root = TestProject::new("native_executable_worker_files");
        let main = root.join("main.js");
        let worker_source = "console.log('worker');\n";
        write_file(
            &root,
            "main.js",
            "const { Worker } = require('worker_threads');\nnew Worker('./worker.js');\n",
        );
        write_file(&root, "worker.js", worker_source);

        let cli = parse_cli_for_test(&["wjsm", "build", "-e", "0"]);
        let eval = None;
        let (artifact, source_files) =
            compile_native_executable_artifact(&cli, &Some(main), &eval, Some(&root), false, &[])
                .expect("source input should compile for native executable");
        assert!(
            source_files
                .values()
                .any(|content| content.as_slice() == worker_source.as_bytes()),
            "static worker source should be collected from source input"
        );

        let portable = root.join("app.wjsm");
        fs::write(&portable, artifact).expect("portable artifact should be writable");
        let (_, portable_files) = compile_native_executable_artifact(
            &cli,
            &Some(portable),
            &eval,
            Some(&root),
            false,
            &[],
        )
        .expect("portable input should compile for native executable");
        assert!(
            portable_files
                .values()
                .any(|content| content.as_slice() == worker_source.as_bytes()),
            "static worker source should survive the portable artifact roundtrip"
        );

        let dynamic_main = root.join("dynamic-main.js");
        let dynamic_worker = root.join("dynamic-worker.js");
        let dynamic_worker_source = "console.log('dynamic worker');\n";
        write_file(
            &root,
            "dynamic-main.js",
            "const { Worker } = require('worker_threads');\nconst path = require('path');\nnew Worker(path.join(__dirname, 'dynamic-worker.js'));\n",
        );
        write_file(&root, "dynamic-worker.js", dynamic_worker_source);
        let (_, dynamic_files) = compile_native_executable_artifact(
            &cli,
            &Some(dynamic_main.clone()),
            &eval,
            Some(&root),
            false,
            &[],
        )
        .expect("dynamic worker input should compile");
        assert!(
            dynamic_files
                .values()
                .all(|content| content.as_slice() != dynamic_worker_source.as_bytes()),
            "dynamic worker source must require an explicit include"
        );

        let (_, included_files) = compile_native_executable_artifact(
            &cli,
            &Some(dynamic_main),
            &eval,
            Some(&root),
            false,
            &[dynamic_worker],
        )
        .expect("explicit worker include should compile");
        assert!(
            included_files
                .values()
                .any(|content| content.as_slice() == dynamic_worker_source.as_bytes()),
            "explicit worker source should be collected"
        );
    }

    #[test]
    fn native_executable_failure_preserves_existing_target() {
        let root = TestProject::new("native_executable_rejection");
        let output = root.join("output");
        fs::write(&output, b"sentinel").expect("target should be writable");
        let output_path = output.to_str().expect("output path should be UTF-8");
        let missing_stub = root.join("missing-wjsm-exec");
        let previous = std::env::var_os("WJSM_EXEC_STUB");
        // SAFETY: 本测试独占该环境变量，结束时恢复。
        unsafe {
            std::env::set_var("WJSM_EXEC_STUB", &missing_stub);
        }
        let cli = parse_cli_for_test(&[
            "wjsm",
            "build",
            "-e",
            "console.log(1)",
            "--format",
            "native-executable",
            "-o",
            output_path,
        ]);
        let result = execute(cli);
        match previous {
            Some(value) => unsafe { std::env::set_var("WJSM_EXEC_STUB", value) },
            None => unsafe { std::env::remove_var("WJSM_EXEC_STUB") },
        }
        let message = format!(
            "{:#}",
            result.expect_err("native executable output should fail without a stub")
        );
        assert!(
            message.contains("failed to locate wjsm-exec stub")
                || message.contains("WJSM_EXEC_STUB"),
            "{message}"
        );
        assert_eq!(
            fs::read(output).expect("target should remain readable"),
            b"sentinel"
        );
    }

    #[test]
    fn native_executable_include_outside_root_preserves_target() {
        let root = TestProject::new("native_executable_include");
        write_file(&root, "main.js", "console.log(1);\n");
        let outside = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("native-exec-include-outside")
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).expect("outside dir");
        fs::write(outside.join("secret.js"), "export {};\n").expect("outside file");
        let output = root.join("output");
        fs::write(&output, b"sentinel").expect("target should be writable");
        let stub = root.join("wjsm-exec");
        fs::write(&stub, b"stub").expect("stub");
        let previous = std::env::var_os("WJSM_EXEC_STUB");
        // SAFETY: 本测试独占该环境变量，结束时恢复。
        unsafe {
            std::env::set_var("WJSM_EXEC_STUB", &stub);
        }
        let cli = parse_cli_for_test(&[
            "wjsm",
            "build",
            "--format",
            "native-executable",
            "--root",
            root.to_str().expect("root"),
            "--include",
            outside.join("secret.js").to_str().expect("include"),
            "-o",
            output.to_str().expect("output"),
            root.join("main.js").to_str().expect("input"),
        ]);
        let result = execute(cli);
        match previous {
            Some(value) => unsafe { std::env::set_var("WJSM_EXEC_STUB", value) },
            None => unsafe { std::env::remove_var("WJSM_EXEC_STUB") },
        }
        let message = format!(
            "{:#}",
            result.expect_err("include outside root should fail")
        );
        let _ = fs::remove_dir_all(&outside);
        assert!(
            message.contains("outside module root") || message.contains("failed to include"),
            "{message}"
        );
        assert_eq!(
            fs::read(output).expect("target should remain readable"),
            b"sentinel"
        );
    }
}
