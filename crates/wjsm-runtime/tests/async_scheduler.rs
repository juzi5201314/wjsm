use anyhow::Result;
use tokio::runtime::Runtime;

use wjsm_runtime::{execute_with_writer, execute_with_writer_async};

fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

fn run_sync(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let out = execute_with_writer(&wasm, Vec::new())?;
    Ok(String::from_utf8(out)?)
}

fn run_async(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Runtime::new()?;
    let out = rt.block_on(async { execute_with_writer_async(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

#[test]
fn async_wrapper_matches_sync_for_sync_program() -> Result<()> {
    let source = r#"
        console.log("parity");
        console.log(1 + 2 * 3);
    "#;

    assert_eq!(run_async(source)?, run_sync(source)?);
    Ok(())
}

#[test]
fn promise_microtasks_drain_after_main_in_order() -> Result<()> {
    let output = run_async(
        r#"
        Promise.resolve(1).then(() => console.log("p1"));
        Promise.resolve(2).then(() => console.log("p2"));
        console.log("sync");
        "#,
    )?;

    assert_eq!(output, "sync\np1\np2\n");
    Ok(())
}

#[test]
fn timer_callback_drains_microtasks_before_next_timer() -> Result<()> {
    let output = run_async(
        r#"
        setTimeout(() => {
            console.log("t1");
            Promise.resolve().then(() => console.log("mt-after-t1"));
        }, 0);
        setTimeout(() => console.log("t2"), 0);
        "#,
    )?;

    assert_eq!(output, "t1\nmt-after-t1\nt2\n");
    Ok(())
}

#[test]
fn settled_fetch_promise_drains_before_async_execute_returns() -> Result<()> {
    let output = run_async(
        r#"
        fetch("data:text/plain,Hello%20World")
          .then(response => response.text())
          .then(text => console.log(text));
        "#,
    )?;

    assert_eq!(output, "Hello World\n");
    Ok(())
}

#[test]
fn async_main_exception_is_reported() -> Result<()> {
    let wasm = compile_source(r#"console.log("before"); throw new Error("top");"#)?;
    let rt = Runtime::new()?;
    let err = rt
        .block_on(async { execute_with_writer_async(&wasm, Vec::new()).await })
        .expect_err("top-level throw must reject async execution");

    assert!(
        err.to_string().contains("Uncaught exception") || err.to_string().contains("Runtime error"),
        "unexpected async execution error: {err:#}"
    );
    Ok(())
}
