// fixture 路径用 `__` 分段映射到测试名，刻意保留非 snake_case。
#![allow(non_snake_case)]

use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

#[test]
fn modules_respects_explicit_root_flag() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("modules")
        .join("named_exports");
    let entry = root.join("main.js");

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .arg("--root")
        .arg(&root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn modules__runtime_loading__cjs_computed_require() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_computed_require/main")
}

#[test]
fn modules__runtime_loading__esm_dynamic_import_variable() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/esm_dynamic_import_variable/main")
}

#[test]
fn modules__runtime_loading__esm_dynamic_import_module_id_collision() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/esm_dynamic_import_module_id_collision/main")
}

#[test]
fn modules__runtime_loading__esm_dynamic_import_builtin() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/esm_dynamic_import_builtin/main")
}

#[test]
fn modules__runtime_loading__cjs_require_builtin() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_require_builtin/main")
}

#[test]
fn modules__runtime_loading__cjs_try_optional_missing() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_try_optional_missing/main")
}

#[test]
fn modules__runtime_loading__cjs_require_json() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_require_json/main")
}

#[test]
fn modules__runtime_loading__cjs_require_cache_delete() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_require_cache_delete/main")
}

#[test]
fn modules__runtime_loading__cjs_circular_partial_exports() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_circular_partial_exports/main")
}

#[test]
fn modules__runtime_loading__require_resolve_paths() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/require_resolve_paths/main")
}

#[test]
fn modules__runtime_loading__extensionless_cjs_require() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/extensionless_cjs_require/main")
}

#[test]
fn modules__runtime_loading__explicit_esm_require_resolve_paths() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/explicit_esm_require_resolve_paths/main")
}

#[test]
fn modules__runtime_loading__esm_dynamic_import_json_rejected() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/esm_dynamic_import_json_rejected/main")
}

#[test]
fn modules__runtime_loading__cjs_errored_cache_delete_retry() -> Result<()> {
    crate::fixture_runner::FixtureRunner::new()?
        .run_single("modules/runtime_loading/cjs_errored_cache_delete_retry/main")
}

#[test]
fn modules__runtime_loading__rejects_runtime_ts_tsx_jsx() -> Result<()> {
    for extension in ["ts", "tsx", "jsx"] {
        assert_runtime_loader_rejects_extension(extension)?;
    }
    Ok(())
}

fn assert_runtime_loader_rejects_extension(extension: &str) -> Result<()> {
    let root = unique_temp_dir(&format!("wjsm_runtime_reject_{extension}"));
    fs::create_dir_all(&root)?;
    let entry = root.join("main.js");
    let dep = root.join(format!("dep.{extension}"));
    fs::write(
        &entry,
        format!(
            concat!(
                "const ext = '{extension}';\n",
                "try {{\n",
                "  require('./dep.' + ext);\n",
                "  console.log('not rejected');\n",
                "}} catch (e) {{\n",
                "  console.log(e.message.indexOf('runtime loader does not compile TypeScript/JSX modules') !== -1);\n",
                "  console.log(e.message.indexOf('dep.{extension}') !== -1);\n",
                "  console.log(e.message.indexOf('Cannot find module') === -1);\n",
                "}}\n",
            ),
            extension = extension,
        ),
    )?;
    fs::write(&dep, "export const value = 1;\n")?;

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .arg("--root")
        .arg(&root)
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "true\ntrue\ntrue\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn run_accepts_non_utf8_file_path() -> Result<()> {
    let root = unique_temp_dir("wjsm_non_utf8_run");
    fs::create_dir_all(&root)?;
    let entry = root.join(std::ffi::OsString::from_vec(b"entry_\xFF.js".to_vec()));
    fs::write(&entry, "console.log(7);\n")?;

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "7\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn explicit_root_accepts_non_utf8_module_entry_path() -> Result<()> {
    let root = unique_temp_dir("wjsm_non_utf8_root");
    fs::create_dir_all(&root)?;
    let entry = root.join(std::ffi::OsString::from_vec(b"main_\xFF.js".to_vec()));
    fs::write(
        &entry,
        "import { answer } from './dep.js';\nconsole.log(answer);\n",
    )?;
    fs::write(root.join("dep.js"), "export const answer = 42;\n")?;

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .arg("--root")
        .arg(&root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn quiet_suppresses_verbose_progress_but_keeps_program_output() -> Result<()> {
    let output = Command::new(resolve_binary_path())
        .args(["-v", "--quiet", "run", "-e", "console.log(7);"])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "7\n");
    assert!(
        stderr.is_empty(),
        "quiet should suppress progress: {stderr}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn config_file_supplies_defaults_and_cli_overrides_target() -> Result<()> {
    let root = unique_temp_dir("wjsm_config");
    fs::create_dir_all(&root)?;
    let config = root.join("wjsm.toml");
    fs::write(&config, "script = true\ntarget = 'jit'\nquiet = true\n")?;

    let output = Command::new(resolve_binary_path())
        .arg("--config")
        .arg(&config)
        .args(["--target", "wasm", "check", "-e", "var await = 1;"])
        .output()?;

    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.is_empty(),
        "quiet config should suppress check progress: {stderr}"
    );
    Ok(())
}

#[test]
fn color_env_force_and_no_color_are_respected() -> Result<()> {
    let forced = Command::new(resolve_binary_path())
        .env("CLICOLOR_FORCE", "1")
        .args(["dump-ir", "-e", "console.log(1);"])
        .output()?;
    let forced_stdout = normalized_stdout(&forced);
    assert!(
        forced.status.success(),
        "stderr: {}",
        normalized_stderr(&forced)
    );
    assert!(
        forced_stdout.contains('\u{1b}'),
        "CLICOLOR_FORCE should enable ANSI color: {forced_stdout}"
    );

    let plain = Command::new(resolve_binary_path())
        .env("CLICOLOR_FORCE", "1")
        .args(["--no-color", "dump-ir", "-e", "console.log(1);"])
        .output()?;
    let plain_stdout = normalized_stdout(&plain);
    assert!(
        plain.status.success(),
        "stderr: {}",
        normalized_stderr(&plain)
    );
    assert!(
        !plain_stdout.contains('\u{1b}'),
        "--no-color should disable ANSI color: {plain_stdout}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn added_cli_commands_have_observable_contracts() -> Result<()> {
    let cache_dir = unique_temp_dir("wjsm_cache_cmd");
    fs::create_dir_all(&cache_dir)?;

    let completions = Command::new(resolve_binary_path())
        .args(["completions", "bash"])
        .output()?;
    assert!(
        completions.status.success(),
        "stderr: {}",
        normalized_stderr(&completions)
    );
    assert!(normalized_stdout(&completions).contains("wjsm"));

    let stats = Command::new(resolve_binary_path())
        .env("WJSM_CACHE_DIR", &cache_dir)
        .args(["cache", "stats"])
        .output()?;
    assert!(
        stats.status.success(),
        "stderr: {}",
        normalized_stderr(&stats)
    );
    assert!(normalized_stdout(&stats).contains("Entries:"));

    let clear = Command::new(resolve_binary_path())
        .env("WJSM_CACHE_DIR", &cache_dir)
        .args(["cache", "clear"])
        .output()?;
    assert!(
        clear.status.success(),
        "stderr: {}",
        normalized_stderr(&clear)
    );
    assert!(normalized_stdout(&clear).contains("Cleared"));

    let lint = Command::new(resolve_binary_path())
        .args(["lint", "-e", "const ok = 1 === 1;"])
        .output()?;
    assert!(
        lint.status.success(),
        "stderr: {}",
        normalized_stderr(&lint)
    );

    let test = Command::new(resolve_binary_path())
        .args(["test", "-e", "console.log(11);"])
        .output()?;
    assert!(
        test.status.success(),
        "stderr: {}",
        normalized_stderr(&test)
    );
    assert_eq!(normalized_stdout(&test), "11\n");

    let repl = Command::new(resolve_binary_path())
        .args(["repl", "--eval", "1 + 2"])
        .output()?;
    assert!(
        repl.status.success(),
        "stderr: {}",
        normalized_stderr(&repl)
    );
    assert_eq!(normalized_stdout(&repl), "3\n");
    Ok(())
}

#[test]
fn process_argv_receives_script_args_after_separator() -> Result<()> {
    let root = unique_temp_dir("wjsm_process_argv");
    fs::create_dir_all(&root)?;
    let entry = root.join("main.js");
    fs::write(
        &entry,
        "console.log(process.argv[2] + ',' + process.argv[3]);\n",
    )?;

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .arg("--")
        .args(["alpha", "beta"])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "alpha,beta\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn process_env_reads_command_environment() -> Result<()> {
    let output = Command::new(resolve_binary_path())
        .env("WJSM_PROCESS_TEST", "1")
        .args(["run", "-e", "console.log(process.env.WJSM_PROCESS_TEST);"])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "1\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn process_cwd_uses_command_current_dir() -> Result<()> {
    let root = unique_temp_dir("wjsm_process_cwd");
    fs::create_dir_all(&root)?;
    let expected = root.canonicalize()?.to_string_lossy().into_owned();
    let output = Command::new(resolve_binary_path())
        .current_dir(&root)
        .args(["run", "-e", "console.log(process.cwd());"])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, format!("{expected}\n"));
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn process_exit_returns_requested_code_without_runtime_error() -> Result<()> {
    let output = Command::new(resolve_binary_path())
        .args([
            "run",
            "-e",
            "console.log('before'); process.exit(7); console.log('after');",
        ])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(stdout, "before\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn process_exit_from_next_tick_skips_timer_and_preserves_stderr() -> Result<()> {
    let output = Command::new(resolve_binary_path())
        .args([
            "run",
            "-e",
            "setTimeout(() => console.log('timer'), 0); process.nextTick(() => { process.stderr.write('err'); process.exit(7); });",
        ])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert_eq!(output.status.code(), Some(7));
    assert!(stdout.is_empty());
    assert_eq!(stderr, "err");
    Ok(())
}

#[test]
fn backend_control_flow_compiles_loop_and_if_without_region_tree() -> Result<()> {
    let output = Command::new(resolve_binary_path())
        .args([
            "dump-wat",
            "--skeleton",
            "-e",
            "let i = 0; while (i < 3) { if (i < 2) { i = i + 1; } else { i = i + 1; } }",
        ])
        .output()?;

    let stdout = normalized_stdout(&output);
    let stderr = normalized_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("(module"), "wat output: {stdout}");
    Ok(())
}

fn normalized_stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn normalized_stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

#[cfg(unix)]
fn unique_temp_dir(name: &str) -> PathBuf {
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(name).join(suffix)
}

fn resolve_binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_wjsm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("debug")
                .join(binary_name())
        })
}

fn binary_name() -> &'static str {
    if cfg!(windows) { "wjsm.exe" } else { "wjsm" }
}
