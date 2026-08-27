//! eval 完成值 lowering 的 IR 正确性测试。
//!
//! 完成值经内存槽线程化后，任意控制流（try/catch/finally、if、循环、switch、
//! 标签 break）下 eval 模块 IR 必须通过验证；具体运行值语义由
//! `fixtures/happy/eval_completion_*.js` 端到端覆盖。

fn lower_eval(source: &str) -> wjsm_ir::Program {
    let module = wjsm_parser::parse_script_as_module(source).unwrap();
    wjsm_semantic::lower_eval_module_with_scope(module, true, true)
        .unwrap_or_else(|error| panic!("lowering failed for {source:?}: {error}"))
}

fn assert_eval_ir_verifies(source: &str) {
    let program = lower_eval(source);
    if let Err(error) = program.verify() {
        panic!(
            "IR verification failed for eval source {source:?}: {error}\n{}",
            program.dump_text()
        );
    }
}

#[test]
fn eval_try_catch_completion_ir_verifies() {
    assert_eval_ir_verifies("try { 1 } catch (e) { 2 }");
    assert_eval_ir_verifies("try { throw 0 } catch (e) { 2 }");
    assert_eval_ir_verifies("try { 1; throw 0 } catch (e) {}");
    assert_eval_ir_verifies("try { 1 } catch (e) { 2 } finally { 3 }");
    assert_eval_ir_verifies("1; try { 2; throw 0 } catch (e) { } finally { 9 }");
    assert_eval_ir_verifies("try { try { 1; throw 0 } finally { 2 } } catch (e) { 5 }");
}

#[test]
fn eval_if_completion_ir_verifies() {
    assert_eval_ir_verifies("42; if (false) 1;");
    assert_eval_ir_verifies("1; if (true) { 2; } else { 3; }");
    assert_eval_ir_verifies("var c = 1; if (c) { if (c > 0) 1; } else { 2; }");
}

#[test]
fn eval_loop_completion_ir_verifies() {
    assert_eval_ir_verifies("1; while (false) { 2 }");
    assert_eval_ir_verifies("var c = 3; while (c--) { 5; }");
    assert_eval_ir_verifies("for (var i = 0; i < 3; i++) { i * 10 }");
    assert_eval_ir_verifies("for (const x of [1, 2, 3]) { x }");
    assert_eval_ir_verifies("for (const k in { a: 1 }) { k }");
    assert_eval_ir_verifies("do { 1 } while (false)");
    assert_eval_ir_verifies("var i = 0; while (i < 3) { i++; if (i === 2) continue; i * 100 }");
}

#[test]
fn eval_switch_completion_ir_verifies() {
    assert_eval_ir_verifies("switch (1) { case 1: 10; break; default: 20 }");
    assert_eval_ir_verifies("switch (2) { case 1: 10; case 2: 20; case 3: 30 }");
}

#[test]
fn eval_labeled_break_completion_ir_verifies() {
    assert_eval_ir_verifies("l: { 1; break l; 2 }");
    assert_eval_ir_verifies("l: { 1; try { 2 } finally { break l; } 4; }");
    assert_eval_ir_verifies("l: { 1; try { throw 0 } finally { break l; } 2; }");
    assert_eval_ir_verifies("outer: while (true) { 1; break outer }");
}

#[test]
fn eval_call_statement_forks_exception_path() {
    // 顶层表达式语句的调用返回值可能是 TAG_EXCEPTION：
    // 必须分叉异常路径（throw 传播），不得把异常标签写入完成值槽继续执行。
    let program = lower_eval("function f() { throw 7 } f(); 2");
    program.verify().unwrap();
    let dump = program.dump_text();
    assert!(
        dump.contains("is_exception"),
        "top-level call statement must fork on TAG_EXCEPTION:\n{dump}"
    );
}

#[test]
fn eval_nested_function_body_does_not_touch_completion() {
    // 嵌套函数体内的语句不参与 eval 完成值：函数体不得写模块入口的完成值槽。
    let program = lower_eval("7; function g() { 42; }");
    program.verify().unwrap();
    let dump = program.dump_text();
    let main = dump
        .split("fn @")
        .find(|part| part.starts_with("$module_main"))
        .expect("module main function present");
    // 完成值槽是模块入口第一个 $tmp 槽；嵌套函数体不应出现对它的 store。
    let slot = main
        .lines()
        .find_map(|line| {
            line.split([' ', ','])
                .find(|token| token.starts_with("$tmp."))
                .map(str::to_string)
        })
        .expect("completion slot store in module main");
    let nested = dump
        .split("fn @")
        .find(|part| part.starts_with("g "))
        .expect("nested function g present");
    assert!(
        !nested.contains(&slot),
        "nested function must not store eval completion slot {slot}:\n{dump}"
    );
}
