use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Once;

const UPDATE_SNAPSHOTS_ENV: &str = "WJSM_UPDATE_FIXTURES";

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
    s
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
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // 寻找 "[object " 模式
        if i + 8 <= len && bytes[i] == b'[' && &bytes[i + 1..i + 8] == b"object " {
            // 找到匹配的 ']'
            if let Some(close) = bytes[i..].iter().position(|&b| b == b']') {
                let close_abs = i + close;
                let inner = &text[i + 1..close_abs]; // "object XXX:123"
                // 检查末尾是否有冒号+数字
                if let Some(colon) = inner.rfind(':') {
                    let suffix = &inner[colon + 1..];
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        // 是对象句柄格式，替换数字为 <id>
                        result.push('[');
                        result.push_str(&inner[..colon]);
                        result.push_str(":<id>]");
                        i = close_abs + 1;
                        continue;
                    }
                }
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}
