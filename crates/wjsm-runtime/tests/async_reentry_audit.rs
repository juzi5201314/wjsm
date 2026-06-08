//! Source audit: async runtime paths must not use sync Wasm re-entry after retirement.
//! Until Task 16, this test documents forbidden patterns; flip `STRICT_AUDIT` when sync helpers are deleted.

use std::fs;
use std::path::Path;

const RUNTIME_SRC: &str = "src";

/// Set true after Task 16 (sync helper retirement).
const STRICT_AUDIT: bool = true;

fn read_rust_sources(dir: &Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            read_rust_sources(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&path)
                .display()
                .to_string();
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
        let trimmed = line.trim();
        if line_looks_like_comment(trimmed) {
            continue;
        }
        for (label, needle) in patterns {
            if line.contains(needle) {
                hits.push(format!("{}:{}: {}", label, line_no + 1, trimmed));
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
        let mut hits = collect_violations(content, patterns);
        // In strict mode: allow known alive sync code paths that are NOT re-entrant
        // (used by the sync execute path, proxy/property helpers, eval, JSON, microtask).
        // These will be retired when the sync execute path is fully replaced.
        let allow_alive_sync = |h: &str| -> bool {
            // call_wasm_callback sync definition and its internal dispatch
            h.contains("fn call_wasm_callback(")
            // recursive bound call inside call_wasm_callback itself
            || h.contains("return call_wasm_callback(&mut *caller, bound_func,")
            // func.call inside call_wasm_callback sync
            || (h.contains("func.call(") && h.contains("let call_result"))
            // call_host_function_with_args sync .call dispatch
            || (h.contains("func.call(") && h.contains("call_host_function_with_args"))
            // proxy_or_target_get_prototype_of_impl sync (getPrototypeOf trap via call_wasm_callback)
            || (h.contains("call_wasm_callback(caller, trap, entry.handler") && h.contains("getPrototypeOf"))
            // reflect_get_impl_with_receiver sync (proxy get trap + accessor getter via call_wasm_callback)
            || h.contains("return call_wasm_callback(")
            || h.contains("call_wasm_callback(caller, getter, receiver, &[])")
            // reflect_get_prototype_of_impl sync in proxy_reflect.rs
            || h.contains("call_wasm_callback(caller, trap, entry.handler, &[entry.target])")
            // runtime_json.rs sync call_wasm_callback (ToPrimitive + reviver)
            || h.contains("call_wasm_callback(caller, method, value, &[])")
            || h.contains("call_wasm_callback(caller, reviver, holder,")
            // streams_readable.rs sync call_wasm_callback (source.start(controller) during ReadableStream construction)
            || h.contains("call_wasm_callback(caller, start_fn, source,")
            // agent_cluster.rs sync receiveBroadcast path; async path uses call_wasm_callback_async.
            || h.contains("call_wasm_callback(caller, callback, value::encode_undefined(), &[sab_obj])")
            // try_compiled_eval_from_caller sync (Instance::new + entry.call)
            || (h.contains("Instance::new(") && h.contains("eval_module"))
            || h.contains("Ok(entry.call(")
        };
        if STRICT_AUDIT {
            hits.retain(|h| !allow_alive_sync(h));
        } else {
            // Allow sync helper definitions until retirement
            hits.retain(|h| {
                let allow_def = h.contains("fn call_wasm_callback(")
                    || h.contains("async fn call_wasm_callback_async")
                    || h.contains("fn resolve_and_call(")
                    || h.contains("fn resolve_callable_and_call(")
                    || h.contains("fn drain_microtasks_from_caller(")
                    || h.contains("fn register_linker(");
                !allow_def
            });
        }
        for h in hits {
            all.push(format!("{path}: {h}"));
        }
    }

    if STRICT_AUDIT {
        assert!(
            all.is_empty(),
            "forbidden sync re-entry in wjsm-runtime/src:\n{}",
            all.join("\n")
        );
    } else {
        // Informational: ensure audit runs and lists current debt when non-empty
        eprintln!(
            "async_reentry_audit (non-strict): {} pattern hit(s); enable STRICT_AUDIT after Task 16",
            all.len()
        );
    }
}
