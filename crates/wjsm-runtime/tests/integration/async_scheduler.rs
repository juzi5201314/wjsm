use anyhow::Result;
use tokio::runtime::Builder;

use wjsm_runtime::{compile_source, execute_with_writer};

fn run_async(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let (out, _) = rt.block_on(async { execute_with_writer(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

fn run_async_source(source: &str) -> Result<String> {
    run_async(source)
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
    let rt = Builder::new_current_thread().enable_all().build()?;
    let err = rt
        .block_on(async { execute_with_writer(&wasm, Vec::new()).await })
        .expect_err("top-level throw must reject async execution");

    assert!(
        err.to_string().contains("Uncaught exception") || err.to_string().contains("Runtime error"),
        "unexpected async execution error: {err:#}"
    );
    Ok(())
}

#[test]
fn async_reentry_array_callbacks() -> Result<()> {
    let output = run_async_source(
        r#"
        const out = [1, 2, 3].map((x) => x * 10);
        console.log(out[0]);
        console.log(out[1]);
        console.log(out[2]);
        const sum = [1, 2, 3].reduce((a, b) => a + b, 0);
        console.log(sum);
        "#,
    )?;
    assert_eq!(output, "10\n20\n30\n6\n");
    Ok(())
}

#[test]
fn async_reentry_function_call_and_apply() -> Result<()> {
    let output = run_async_source(
        r#"
        function add(a, b) { return a + b; }
        console.log(add.call(null, 3, 4));
        "#,
    )?;
    assert_eq!(output, "7\n");
    Ok(())
}

#[test]
fn async_reentry_microtask_and_timer_callbacks() -> Result<()> {
    let output = run_async_source(
        r#"
        queueMicrotask(() => console.log("qm"));
        setTimeout(() => console.log("timer"), 0);
        "#,
    )?;
    assert_eq!(output, "qm\ntimer\n");
    Ok(())
}

#[test]
fn async_reentry_json_parse_reviver() -> Result<()> {
    let output = run_async_source(
        r#"
        const v = JSON.parse('{"a":1}', (key, val) => {
          if (key === "a") return val + 1;
          return val;
        });
        console.log(v.a);
        "#,
    )?;
    assert_eq!(output, "2\n");
    Ok(())
}

#[test]
fn async_reentry_proxy_reflect_traps() -> Result<()> {
    let output = run_async_source(
        r#"
        const handler = {
            get(t, p) { return p === 'foo' ? 42 : t[p]; },
            has(t, p) { return p === 'foo'; },
            apply(t, that, args) { return args[0] + 1; },
            construct(t, args) { return { val: args[0] }; },
            ownKeys(t) { return ['foo']; },
            defineProperty(t, p, d) { return true; },
        };
        const p = new Proxy({}, handler);
        const result = [];
        result.push(Reflect.get(p, 'foo'));
        result.push(Reflect.has(p, 'foo'));
        result.push(Reflect.apply(function(x){return x+1;}, null, [5]));
        result.push(Reflect.ownKeys(p).join(','));
        console.log(result.join(','));
        "#,
    )?;
    assert_eq!(output, "42,true,6,foo\n");
    Ok(())
}

#[test]
fn async_reentry_proxy_trap_imports() -> Result<()> {
    let output = run_async_source(
        r#"
        const target = { x: 10 };
        const handler = {
          get: function(t, p, r) {
            return 1;
          },
          has: function(t, p) {
            return p === 'x';
          }
        };
        const proxy = new Proxy(target, handler);
        console.log([proxy.x, 'x' in proxy].join(','));
        "#,
    )?;
    assert_eq!(output, "1,true\n");
    Ok(())
}
#[test]
fn async_reentry_eval_direct_indirect_and_native() -> Result<()> {
    let output = run_async_source(
        r#"
        let x = 1;
        let r1 = eval('x + 1');
        let r2 = eval('function f(y) { return y + 3; } f(1)');
        console.log(r1 + ',' + r2);
        "#,
    )?;
    assert_eq!(output, "2,4\n");
    Ok(())
}

#[test]
fn async_generator_return_resumes_suspended_yield_and_runs_finally() -> Result<()> {
    let output = run_async_source(
        r#"
        async function* gen() {
            try { yield 1; }
            finally { console.log("cleanup"); }
        }

        const g = gen();
        g.next()
          .then(() => g.return())
          .then(() => console.log("done"));
        "#,
    )?;
    assert_eq!(output, "cleanup\ndone\n");
    Ok(())
}

#[test]
fn async_generator_throw_is_catchable_inside_generator() -> Result<()> {
    let output = run_async_source(
        r#"
        async function* gen() {
            try {
                yield 1;
            } catch (e) {
                console.log("caught");
            } finally {
                console.log("finally");
            }
        }

        const g = gen();
        function inject() { g.throw("boom"); }
        g.next().then(inject);
        "#,
    )?;
    assert_eq!(output, "caught\nfinally\n");
    Ok(())
}

#[test]
fn promise_resolve_thenable_passes_reject_and_rejects_on_throw() -> Result<()> {
    let output = run_async_source(
        r#"
        const thenable = {
            marker: 7,
            then(resolve, reject) {
                console.log(this.marker);
                console.log(typeof reject);
                throw new Error("boom");
            }
        };

        Promise.resolve(thenable).then(
            () => console.log("fulfilled"),
            (err) => console.log("rejected " + err.message)
        );
        "#,
    )?;
    assert_eq!(output, "7\nfunction\nrejected boom\n");
    Ok(())
}

#[test]
fn finally_only_inner_try_propagates_to_outer_catch_after_finally() -> Result<()> {
    let output = run_async_source(
        r#"
        try {
            try { throw 1; }
            finally { console.log("inner"); }
        } catch (e) {
            console.log(e);
        }
        "#,
    )?;
    assert_eq!(output, "inner\n1\n");
    Ok(())
}

#[test]
fn catch_throw_runs_same_try_finally_before_outer_catch() -> Result<()> {
    let output = run_async_source(
        r#"
        try {
            try { throw 1; }
            catch (e) { throw 2; }
            finally { console.log("finally"); }
        } catch (e2) {
            console.log(e2);
        }
        "#,
    )?;
    assert_eq!(output, "finally\n2\n");
    Ok(())
}
