use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Once;

const UPDATE_SNAPSHOTS_ENV: &str = "WJSM_UPDATE_FIXTURES";
const VERIFY_ORACLE_ENV: &str = "WJSM_VERIFY_ORACLE";

/// 进程级一次性初始化测试环境：固定时区为 UTC，设置 wasm 编译缓存目录。
/// 旧子进程模型靠 `Command::env("TZ","UTC")` 保证 Date fixture 稳定；
/// in-process 调用无子进程，需在测试进程内设置。chrono 读 TZ 决定 Local 偏移，
/// 必须在任何 Date 逻辑运行前设好。
/// wasmtime 编译缓存指向 /tmp，避免相同 wasm bytes 的重复 Cranelift 编译。
static ENV_INIT: Once = Once::new();

fn ensure_test_env() {
    ENV_INIT.call_once(|| {
        // SAFETY: 在测试初始化早期、首个 fixture 运行前设置一次；
        // call_once 保证无并发写。后续只读。
        unsafe {
            env::set_var("TZ", "UTC");
            env::set_var("WJSM_CACHE_DIR", "/tmp/wjsm-test-cache");
            // OS child（cluster/child_process）继承 Winch + cache。
            if env::var_os("WJSM_COMPILER").is_none() {
                env::set_var("WJSM_COMPILER", "winch");
            }
            // 默认禁用 child_process，避免会话环境污染 fixture 期望。
            env::remove_var("WJSM_CHILD_PROCESS_ALLOW");
        }
    });
}

pub struct FixtureRunner {
    fixtures_root: PathBuf,
    update_snapshots: bool,
}

struct FixtureCase {
    input_path: PathBuf,
    expected_path: PathBuf,
}

struct FixtureOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

impl FixtureRunner {
    pub fn new() -> Result<Self> {
        ensure_test_env();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures_root = manifest_dir.join("fixtures");
        let update_snapshots = snapshot_updates_enabled();

        Ok(Self {
            fixtures_root,
            update_snapshots,
        })
    }

    /// Run a single fixture by its suite-relative path (without extension).
    /// Example: `run_single("happy/hello")` runs `fixtures/happy/hello.js`.
    pub fn run_single(&self, name: &str) -> Result<()> {
        let js_path = self.fixtures_root.join(format!("{name}.js"));
        let ts_path = self.fixtures_root.join(format!("{name}.ts"));
        let input_path;
        if js_path.exists() {
            input_path = js_path;
        } else if ts_path.exists() {
            input_path = ts_path;
        } else {
            bail!(
                "Fixture not found: {name}.js or {name}.ts (searched in {})",
                self.fixtures_root.display()
            );
        }

        let expected_path = input_path.with_extension("expected");

        let fixture = FixtureCase {
            input_path,
            expected_path,
        };

        self.run_fixture(&fixture)
    }

    fn run_fixture(&self, fixture: &FixtureCase) -> Result<()> {
        // 检查 KNOWN-NETWORK 注释
        if let Ok(content) = fs::read_to_string(&fixture.input_path)
            && content.contains("KNOWN-NETWORK")
            && env::var("WJSM_SKIP_NETWORK").unwrap_or_default() == "1"
        {
            return Ok(());
        }
        let (exit_code, stdout, stderr) = wjsm_cli::run_file_in_process(&fixture.input_path);
        let actual = FixtureOutput {
            stdout: normalize_output(&stdout),
            stderr: normalize_output(&stderr),
            exit_code,
        }
        .snapshot();
        if !fixture.expected_path.exists() {
            fs::write(&fixture.expected_path, &actual).with_context(|| {
                format!(
                    "Failed to create snapshot file {}",
                    fixture.expected_path.display()
                )
            })?;
            return Ok(());
        }

        let expected = fs::read_to_string(&fixture.expected_path).with_context(|| {
            format!(
                "Failed to read snapshot file {}",
                fixture.expected_path.display()
            )
        })?;

        if expected == actual {
            return Ok(());
        }

        if self.update_snapshots {
            if oracle_enabled() {
                verify_oracle(&fixture.input_path, exit_code, &stdout, &stderr)?;
            }
            fs::write(&fixture.expected_path, &actual).with_context(|| {
                format!(
                    "Failed to update snapshot file {}",
                    fixture.expected_path.display()
                )
            })?;
            return Ok(());
        }

        bail!(
            "snapshot mismatch: {}\n--- expected ---\n{}\n--- actual ---\n{}",
            fixture.expected_path.display(),
            expected,
            actual
        );
    }
}

impl FixtureOutput {
    fn snapshot(&self) -> String {
        let mut snapshot = String::new();
        snapshot.push_str(&format!("exit_code: {}\n", self.exit_code));
        snapshot.push_str("--- stdout ---\n");
        snapshot.push_str(&self.stdout);
        if !self.stdout.is_empty() && !self.stdout.ends_with('\n') {
            snapshot.push('\n');
        }

        snapshot.push_str("--- stderr ---\n");
        snapshot.push_str(&self.stderr);
        if !self.stderr.is_empty() && !self.stderr.ends_with('\n') {
            snapshot.push('\n');
        }

        snapshot
    }
}

fn snapshot_updates_enabled() -> bool {
    matches!(
        env::var(UPDATE_SNAPSHOTS_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// 检查是否启用 oracle 验证。
/// UPDATE_FIXTURES=1 时默认启用（除非显式设 VERIFY_ORACLE=0）；
/// 单独设 VERIFY_ORACLE=1 也可启用。
fn oracle_enabled() -> bool {
    let updating = snapshot_updates_enabled();
    match env::var(VERIFY_ORACLE_ENV).as_deref() {
        Ok("0") | Ok("false") | Ok("FALSE") => false,
        Ok("1") | Ok("true") | Ok("TRUE") => true,
        Ok(_) => updating,
        Err(_) => updating,
    }
}

/// 使用 Node.js 作为 oracle 验证 wjsm 输出。
/// 如果 wjsm 输出与 Node.js 不一致，拒绝自动更新 .expected。
/// 返回 Ok 表示通过（可以安全更新），Err 表示被拒绝。
fn verify_oracle(
    fixture_path: &PathBuf,
    wjsm_exit: i32,
    wjsm_stdout: &[u8],
    wjsm_stderr: &[u8],
) -> Result<()> {
    // 检查 Node.js 是否可用
    let node_check = std::process::Command::new("node").arg("--version").output();
    if node_check.is_err() {
        // Node.js 未安装，打印警告但不阻止更新
        eprintln!(
            "wjsm: oracle warning: node is not installed, skipping oracle verification for {}",
            fixture_path.display()
        );
        return Ok(());
    }

    let node_output = std::process::Command::new("node")
        .arg("--no-warnings")
        .arg(fixture_path)
        .env("TZ", "UTC")
        .output()
        .with_context(|| format!("oracle: failed to run node on {}", fixture_path.display()))?;

    let node_exit = node_output.status.code().unwrap_or(-1);
    let node_stdout = String::from_utf8_lossy(&node_output.stdout).replace("\r\n", "\n");
    let node_stderr = String::from_utf8_lossy(&node_output.stderr).replace("\r\n", "\n");

    // 归一化 wjsm 输出用于对比：
    // - 去除对象句柄数字（[object Type:NNN] → [object Type]）
    // - 去除 WASM 地址和回溯
    let wjsm_stdout_raw = String::from_utf8_lossy(wjsm_stdout).replace("\r\n", "\n");
    let wjsm_stderr_raw = String::from_utf8_lossy(wjsm_stderr).replace("\r\n", "\n");

    let wjsm_stdout_cmp = normalize_for_oracle(&wjsm_stdout_raw);
    let wjsm_stderr_cmp = normalize_for_oracle(&wjsm_stderr_raw);
    let node_stdout_cmp = node_stdout.trim().to_string();
    let node_stderr_cmp = node_stderr.trim().to_string();

    let mut errors: Vec<String> = Vec::new();

    if wjsm_exit != node_exit {
        errors.push(format!("exit code: wjsm={}, node={}", wjsm_exit, node_exit));
    }

    if wjsm_stdout_cmp.trim() != node_stdout_cmp {
        errors.push(format!(
            "stdout:\n--- wjsm ---\n{}\n--- node ---\n{}",
            wjsm_stdout_cmp.trim(),
            node_stdout_cmp
        ));
    }

    if wjsm_stderr_cmp.trim() != node_stderr_cmp {
        errors.push(format!(
            "stderr:\n--- wjsm ---\n{}\n--- node ---\n{}",
            wjsm_stderr_cmp.trim(),
            node_stderr_cmp
        ));
    }

    if errors.is_empty() {
        return Ok(());
    }

    bail!(
        "oracle verification failed for {}:\n{}\n\n\
         wjsm 输出与 Node.js 不一致，UPDATE_FIXTURES 已拒绝自动更新。\n\
         如果这是预期内的 wjsm 专有行为，请手动编辑 .expected 文件。\n\
         要跳过 oracle 验证：WJSM_VERIFY_ORACLE=0",
        fixture_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        errors.join("\n")
    );
}

/// 为 oracle 对比做归一化：去除 wjsm 特有的渲染差异。
fn normalize_for_oracle(text: &str) -> String {
    // 去除对象句柄数字：[object Type:123] → [object Type]
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if i + 8 <= len && bytes[i] == b'[' && &bytes[i + 1..i + 8] == b"object "
            && let Some(close) = bytes[i..].iter().position(|&b| b == b']') {
                let close_abs = i + close;
                let inner = &text[i + 1..close_abs];
                if let Some(colon) = inner.rfind(':') {
                    let suffix = &inner[colon + 1..];
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        result.push('[');
                        result.push_str(&inner[..colon]);
                        result.push(']');
                        i = close_abs + 1;
                        continue;
                    }
                }
            }
        result.push(bytes[i] as char);
        i += 1;
    }

    // 去除 WASM 地址 0xNNNN
    let mut out = String::with_capacity(result.len());
    let mut chars = result.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next();
            let mut had_hex = false;
            while let Some(&h) = chars.peek() {
                if h.is_ascii_hexdigit() {
                    had_hex = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if had_hex {
                out.push_str("<addr>");
            } else {
                out.push('0');
                out.push('x');
            }
            continue;
        }
        out.push(c);
    }
    result = out;

    // 去除 WASM 回溯行（含 "wasm backtrace"、"wasm function" 的行）
    let lines: Vec<&str> = result.lines().collect();
    let filtered: Vec<&str> = lines
        .into_iter()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.contains("wasm backtrace")
                && !trimmed.contains("wasm function ")
                && !trimmed.contains("wasm trap:")
                && trimmed != "<addr>"
        })
        .collect();
    result = filtered.join("\n");

    result
}

fn normalize_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let text = normalize_object_handles(&text);
    let text = normalize_paths(&text);
    normalize_wasm_backtrace(&text)
}

/// 归一化 wasm trap 回溯中的不稳定地址/函数索引。
fn normalize_wasm_backtrace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next();
            let mut had_hex = false;
            while let Some(&h) = chars.peek() {
                if h.is_ascii_hexdigit() {
                    had_hex = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if had_hex {
                out.push_str("<wasm-addr>");
            } else {
                out.push('0');
                out.push('x');
            }
            continue;
        }
        out.push(c);
    }
    let mut s = out;
    while let Some(idx) = s.find("wasm function ") {
        let start = idx + "wasm function ".len();
        let digit_len = s[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digit_len == 0 {
            break;
        }
        s.replace_range(start..start + digit_len, "<idx>");
    }
    normalize_generated_anonymous_frames(&s)
}

fn normalize_generated_anonymous_frames(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut rest = text;
    let needle = "at <anonymous> (";

    while let Some(idx) = rest.find(needle) {
        let frame_start = idx + needle.len();
        normalized.push_str(&rest[..frame_start]);
        rest = &rest[frame_start..];

        let Some(close_idx) = rest.find(')') else {
            normalized.push_str(rest);
            return normalized;
        };
        let frame = &rest[..close_idx];
        normalized.push_str(&normalize_generated_anonymous_frame(frame));
        normalized.push(')');
        rest = &rest[close_idx + 1..];
    }

    normalized.push_str(rest);
    normalized
}

fn normalize_generated_anonymous_frame(frame: &str) -> String {
    let Some((path, line)) = frame.rsplit_once(':') else {
        return frame.to_string();
    };
    if path.ends_with(".js") && line.chars().all(|c| c.is_ascii_digit()) && line != "0" {
        format!("{path}:<generated>")
    } else {
        frame.to_string()
    }
}

/// 归一化绝对路径为相对路径（去除 /fixtures/ 前缀）
fn normalize_paths(text: &str) -> String {
    // 将包含 /fixtures/ 或 \fixtures\ 的绝对路径转换为相对路径
    // 例如: /workspace/fixtures/modules/foo.js -> modules/foo.js
    let mut normalized = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut i = 0;

    while i < text.len() {
        let remaining = &text[i..];
        let unix_match = remaining.find("/fixtures/");
        let windows_match = remaining.find("\\fixtures\\");

        let match_offset = match (unix_match, windows_match) {
            (Some(u), Some(w)) => Some(u.min(w)),
            (Some(u), None) => Some(u),
            (None, Some(w)) => Some(w),
            (None, None) => None,
        };

        if let Some(offset) = match_offset {
            let fixtures_pos = i + offset;

            // 向前查找到路径起始（空格、引号、括号或行首）
            let mut path_start = fixtures_pos;
            for j in (0..fixtures_pos).rev() {
                let c = text.as_bytes()[j];
                if c == b' ' || c == b'\'' || c == b'"' || c == b'`' || c == b'(' || c == b'\n' {
                    path_start = j + 1;
                    break;
                }
                if j == 0 {
                    path_start = 0;
                }
            }

            // 向后查找到路径结束（空格、引号、括号或行尾）
            let after_fixtures = fixtures_pos + 10; // "/fixtures/" 或 "\fixtures\" 长度为 10
            let mut path_end = text.len();
            for j in after_fixtures..text.len() {
                let c = text.as_bytes()[j];
                if c == b' ' || c == b'\'' || c == b'"' || c == b'`' || c == b')' || c == b'\n' {
                    path_end = j;
                    break;
                }
            }

            // 输出路径之前的内容
            normalized.push_str(&text[last_end..path_start]);
            // 输出相对路径（去掉 /fixtures/ 前缀）
            normalized.push_str(&text[after_fixtures..path_end]);

            last_end = path_end;
            i = path_end;
        } else {
            break;
        }
    }

    normalized.push_str(&text[last_end..]);
    normalized
}

/// 归一化对象句柄数字（如 [object Object:208] → [object Object:<id>]）
fn normalize_object_handles(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < text.len() {
        if text[i..].starts_with("[object ")
            && let Some(close_rel) = text[i..].find(']') {
                let close_abs = i + close_rel;
                let inner = &text[i + 1..close_abs];
                if let Some(colon) = inner.rfind(':') {
                    let suffix = &inner[colon + 1..];
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        result.push('[');
                        result.push_str(&inner[..colon]);
                        result.push_str(":<id>]");
                        i = close_abs + 1;
                        continue;
                    }
                }
            }

        let ch = text[i..].chars().next().expect("valid UTF-8 boundary");
        result.push(ch);
        i += ch.len_utf8();
    }

    result
}
