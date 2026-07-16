//! Source audit: async runtime paths must not use sync Wasm re-entry.
//!
//! ## 作用
//!
//! 静态扫描 `crates/wjsm-runtime/src/**/*.rs`，禁止下列 sync 模式（doc/line 注释除外）：
//! `call_wasm_callback(` / `resolve_and_call(` / `resolve_callable_and_call(` /
//! `drain_microtasks_from_caller(` / `Instance::new(` / `.call(`。
//!
//! ## 为什么
//!
//! 启用 `Store::epoch_deadline_async_yield_and_update` 之后（docs/async-scheduler.md），
//! 任何经该 Store 的 Wasm 实例化或调用都必须走 async API；否则 epoch 抢占无法 yield，
//! 轻则丢微任务调度公平性，重则在重入路径上死锁/panic。本测试是这条架构契约的硬卡点。
//!
//! ## 失败时怎么做
//!
//! **默认假设是你写错了**——把命中的 sync helper 改成 `_async` 孪生 + `.await`，
//! 调用栈一路 await 到 `linker.func_wrap_async` 或已有的 async 入口。多数情况存在
//! `_async` 版本（runtime_host_helpers / runtime_eval / runtime_json / proxy_reflect 都齐了）。
//!
//! ## 何时可以「绕过」
//!
//! 仅当下面三种情况之一成立，且 reviewer 同意：
//!
//! 1. **误报**：新加的函数名巧合包含 `.call(`（比如 `obj.call_count` 字段、`registry.call_site()`
//!    方法），但**不**进行 Wasm 调用。修复方式：换名（最佳）或把命中行整理成单行 doc 注释样式
//!    让 `line_looks_like_comment` 跳过——**禁止**为此修改 `patterns` 数组放宽全局规则。
//!
//! 2. **真同步、绝不重入**：某 sync helper 命名仍含禁词，但路径上保证不会触达 Wasm
//!    （例如纯内存读、句柄表查表、错误对象构造）。这种情况几乎不存在；如有，请改名以
//!    避免与重入助手混淆。
//!
//! 3. **架构演进废弃本测试**：Wasmtime 模型变更或 epoch yielding 被替换。这种情况下
//!    *删除*本测试连同 docs/async-scheduler.md 一并更新，**而不是**给某条 pattern 加白名单
//!    或加 `#[ignore]`。曾经的临时白名单（`STRICT_AUDIT` + `allow_alive_sync`）已在
//!    Task 16 完工时删除——不要重新引入。
//!
//! 出现「我先 `#[ignore]` 跑通别的再说」的冲动时，先翻 `git log` 看上一个迁移
//! commit 是怎么做的；再决定是否真的要绕。

use std::fs;
use std::path::Path;

const RUNTIME_SRC: &str = "src";

fn read_rust_sources(dir: &Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            read_rust_sources(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let rel = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let content = fs::read_to_string(&path)?;
            out.push((rel, content));
        }
    }
    Ok(())
}

fn line_looks_like_comment(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("///")
        || trimmed.starts_with("//!")
}

fn collect_violations(content: &str, patterns: &[(&str, &str)]) -> Vec<String> {
    let mut hits = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if line_looks_like_comment(trimmed) {
            continue;
        }
        for (label, needle) in patterns {
            if line.contains(needle) {
                hits.push(format!("{label}:{}: {}", line_no + 1, line.trim()));
            }
        }
    }
    hits
}

#[test]
fn async_reentry_audit_forbidden_sync_patterns() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_SRC);
    let mut files = Vec::new();
    read_rust_sources(&src_root, &mut files).expect("read runtime src");

    let patterns: &[(&str, &str)] = &[
        ("sync call_wasm_callback", "call_wasm_callback("),
        ("sync resolve_and_call", "resolve_and_call("),
        (
            "sync resolve_callable_and_call",
            "resolve_callable_and_call(",
        ),
        (
            "sync drain_microtasks_from_caller",
            "drain_microtasks_from_caller(",
        ),
        ("Instance::new", "Instance::new("),
        ("TypedFunc::call ", ".call("),
    ];

    let mut all = Vec::new();
    for (path, content) in &files {
        for hit in collect_violations(content, patterns) {
            all.push(format!("{path}: {hit}"));
        }
    }

    assert!(
        all.is_empty(),
        "forbidden sync re-entry in wjsm-runtime/src:\n{}",
        all.join("\n")
    );
}
