//! Host fetch resource timing 正确性测试（in-process + 测试替身 transport）。
//!
//! 不启动真实 HTTP 服务器、不做任何真实网络 I/O：通过 `WJSM_TEST_FAKE_FETCH`
//! 让宿主 fetch transport 返回确定性响应，测试只断言 fetch → resource timing
//! 完成/抑制协议的状态机行为。

use anyhow::Result;
use wjsm_runtime::{
    RuntimeInput, RuntimeOptions, compile_source, execute_with_writer_with_options,
};

const FAKE_FETCH_ENV: &str = "WJSM_TEST_FAKE_FETCH";

fn run_source(source: &str, base_url: &str) -> Result<(String, String)> {
    let artifact = compile_source(source)?;
    let options = RuntimeOptions {
        env: vec![("WJSM_TEST_URL".to_string(), base_url.to_string())],
        ..RuntimeOptions::default()
    };
    let mut stdout = Vec::new();
    execute_with_writer_with_options(RuntimeInput::Artifact(&artifact), &mut stdout, options)?;
    Ok((String::from_utf8(stdout)?, String::new()))
}

#[test]
fn host_fetch_owns_resource_timing_completion() -> Result<()> {
    // 本二进制只有这一个测试；先记录原值，测试结束恢复，避免污染进程环境。
    let previous = std::env::var_os(FAKE_FETCH_ENV);
    // SAFETY: 本二进制只有这一个测试，测试期间无其它线程 set/remove 环境变量。
    unsafe { std::env::set_var(FAKE_FETCH_ENV, "1") };

    let base_url = "http://fake.test";
    let result = run_source(
        r#"
const perfHost = globalThis.__wjsm_node_perf_hooks;
const entries = [];

function drainEntries() {
  let entry = perfHost.drainNativeEntry();
  while (entry !== undefined) {
    entries.push(entry);
    entry = perfHost.drainNativeEntry();
  }
}

function nextImmediate() {
  return new Promise((resolve) => setImmediate(resolve));
}

function drainReader(reader) {
  return reader.read().then((result) => {
    if (result.done) return;
    return drainReader(reader);
  });
}

const baseUrl = process.env.WJSM_TEST_URL;
perfHost.setObserverState(64, drainEntries);

const clonedResponse = await fetch(baseUrl + '/clone');
const clone = clonedResponse.clone();
await clone.text();
await nextImmediate();
const cloneCount = entries.length;
const originalBody = await clonedResponse.text();
await nextImmediate();
console.log(cloneCount === 1 && entries.length === 1 && originalBody === 'hello');

const readerResponse = await fetch(baseUrl + '/reader');
const reader = readerResponse.body.getReader();
await drainReader(reader);
await nextImmediate();
const readerTiming = entries[1].detail.timingInfo;
console.log(
  entries.length === 2 &&
  entries[1].name === baseUrl + '/reader' &&
  readerTiming.encodedBodySize === 5 &&
  readerTiming.decodedBodySize === 5
);

const cancelledResponse = await fetch(baseUrl + '/cancel');
await cancelledResponse.body.cancel();
await nextImmediate();
console.log(entries.length === 3 && entries[2].name === baseUrl + '/cancel');

const emptyResponse = await fetch(baseUrl + '/empty');
await nextImmediate();
console.log(
  emptyResponse.body === null &&
  entries.length === 4 &&
  entries[3].detail.responseStatus === 204
);

const failedResponse = await fetch(baseUrl + '/error');
try {
  await failedResponse.text();
} catch {}
await nextImmediate();
console.log(entries.length === 5 && entries[4].name === baseUrl + '/error');

const beforeInternal = entries.length;
const internalResponse = await fetch(baseUrl + '/internal', {
  __wjsm_internal_no_resource_timing: true,
});
await internalResponse.text();
await nextImmediate();
console.log(entries.length === beforeInternal);
perfHost.setObserverState(0, undefined);
"#,
        base_url,
    );

    match previous {
        // SAFETY: 本二进制只有这一个测试，测试期间无其它线程 set/remove 环境变量。
        Some(value) => unsafe { std::env::set_var(FAKE_FETCH_ENV, value) },
        // SAFETY: 本二进制只有这一个测试，测试期间无其它线程 set/remove 环境变量。
        None => unsafe { std::env::remove_var(FAKE_FETCH_ENV) },
    }

    let (stdout, stderr) = result?;
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
    Ok(())
}
