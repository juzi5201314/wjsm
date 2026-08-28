//! 脚本模式全局环境记录（ES §9.1.1.4 / §16.1.7 GDI）集成测试。
//!
//! 覆盖：顶层 var/函数 → 全局对象属性（对象记录）、顶层 let/const/class →
//! 全局声明式记录（TDZ / const 检查）、间接 eval 与 `new Function` 的
//! 全局边界名字解析、delete/typeof/update 的运行时语义。
//! 期望值全部经 Node v22 `node -e` 逐例核对。

use wjsm_cli::run_script_source_in_process;

fn run_script(source: &str) -> (i32, String, String) {
    let (exit, stdout, stderr) = run_script_source_in_process(source);
    (
        exit,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

fn assert_stdout(source: &str, expected: &str) {
    let (exit, stdout, stderr) = run_script(source);
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, expected, "stderr: {stderr}");
}

#[test]
fn top_level_var_becomes_global_this_property() {
    assert_stdout(
        r#"
var x = 1;
console.log(globalThis.x, x);
x = 2;
console.log(globalThis.x);
globalThis.x = 3;
console.log(x);
const d = Object.getOwnPropertyDescriptor(globalThis, "x");
console.log(d.writable, d.enumerable, d.configurable);
"#,
        "1 1\n2\n3\ntrue true false\n",
    );
}

#[test]
fn top_level_lexicals_visible_to_indirect_eval_and_function() {
    assert_stdout(
        r#"
let y = 41;
const c = 7;
console.log((0, eval)("y + 1"), (0, eval)("c"));
console.log(new Function("return y")());
console.log("y" in globalThis, Object.prototype.hasOwnProperty.call(globalThis, "c"));
(0, eval)("y = 100");
console.log(y);
"#,
        "42 7\n41\nfalse false\n100\n",
    );
}

#[test]
fn class_declaration_enters_declarative_record() {
    assert_stdout(
        r#"
class Klass { static tag = "K" }
console.log((0, eval)("Klass.tag"), "Klass" in globalThis);
console.log(new Function("return new Klass()")() instanceof Klass);
"#,
        "K false\ntrue\n",
    );
}

#[test]
fn lexical_tdz_reads_writes_and_typeof_throw() {
    assert_stdout(
        r#"
try { console.log(z) } catch (e) { console.log(e.constructor.name, e.message) }
try { z = 5 } catch (e) { console.log(e.constructor.name, e.message) }
try { typeof z } catch (e) { console.log(e.constructor.name, e.message) }
let z = 1;
console.log(z);
"#,
        "ReferenceError Cannot access 'z' before initialization\n\
         ReferenceError Cannot access 'z' before initialization\n\
         ReferenceError Cannot access 'z' before initialization\n\
         1\n",
    );
}

#[test]
fn const_reassignment_is_runtime_type_error() {
    assert_stdout(
        r#"
const cc = 2;
try { cc = 3 } catch (e) { console.log(e.constructor.name, e.message) }
try { cc++ } catch (e) { console.log(e.constructor.name, e.message) }
try { cc += 1 } catch (e) { console.log(e.constructor.name, e.message) }
console.log(cc);
"#,
        "TypeError Assignment to constant variable.\n\
         TypeError Assignment to constant variable.\n\
         TypeError Assignment to constant variable.\n\
         2\n",
    );
}

#[test]
fn function_declarations_create_global_function_bindings() {
    assert_stdout(
        r#"
console.log(hoisted(), typeof globalThis.hoisted);
function hoisted() { return "fn" }
const d = Object.getOwnPropertyDescriptor(globalThis, "hoisted");
console.log(d.writable, d.enumerable, d.configurable);
console.log((0, eval)("hoisted()"), new Function("return hoisted()")());
"#,
        "fn function\ntrue true false\nfn fn\n",
    );
}

#[test]
fn delete_semantics_follow_global_environment_record() {
    assert_stdout(
        r#"
var v = 1;
function f() {}
let l = 2;
console.log(delete v, delete f, delete l);
console.log(v, typeof f, l);
implicit = 9;
console.log(delete implicit, typeof implicit);
console.log(delete neverDeclared);
"#,
        "false false false\n1 function 2\ntrue undefined\ntrue\n",
    );
}

#[test]
fn sloppy_implicit_globals_and_dynamic_typeof() {
    assert_stdout(
        r#"
console.log(typeof notYet);
(0, eval)("created = 5");
console.log(created, typeof created);
implicit = 6;
console.log(globalThis.implicit);
"#,
        "undefined\n5 number\n6\n",
    );
}

#[test]
fn strict_script_undeclared_assignment_throws() {
    assert_stdout(
        r#"
"use strict";
try { undeclared = 1 } catch (e) { console.log(e.constructor.name, e.message) }
try { missing } catch (e) { console.log(e.constructor.name, e.message) }
var sx = 1; let sy = 2;
console.log((0, eval)("sx + sy"));
"#,
        "ReferenceError undeclared is not defined\n\
         ReferenceError missing is not defined\n\
         3\n",
    );
}

#[test]
fn update_and_compound_assignments_route_through_global_env() {
    assert_stdout(
        r#"
let n = 10;
console.log(n++, ++n, n--, --n, n);
let s;
s ??= 5;
s &&= 6;
s ||= 7;
console.log(s);
var m = 1;
m **= 3;
console.log(m, globalThis.m);
"#,
        "10 12 12 10 10\n6\n1 1\n",
    );
}

#[test]
fn closures_share_global_bindings_with_eval() {
    assert_stdout(
        r#"
let counter = 0;
function inc() { counter++; return counter }
console.log(inc(), inc());
const arrow = () => counter * 10;
console.log(arrow());
eval("counter = 50");
console.log(counter, (0, eval)("counter"), new Function("return counter")());
function outer() { return function () { counter = 77; return counter } }
console.log(outer()(), counter);
"#,
        "1 2\n20\n50 50 50\n77 77\n",
    );
}

#[test]
fn destructuring_declarations_initialize_global_bindings() {
    assert_stdout(
        r#"
let { a, b: [c] } = { a: 1, b: [2] };
const { d = 4 } = {};
var [e, ...rest] = [5, 6, 7];
console.log(a, c, d, e, rest.join(","));
console.log((0, eval)("a + c + d"), globalThis.e, "a" in globalThis);
"#,
        "1 2 4 5 6,7\n7 5 false\n",
    );
}

#[test]
fn for_loop_var_heads_leak_to_global_this() {
    assert_stdout(
        r#"
for (var i = 0; i < 3; i++) {}
console.log(i, globalThis.i);
for (var [d, e] of [[1, 2]]) {}
console.log(d, e, globalThis.d);
"#,
        "3 3\n1 2 1\n",
    );
}

#[test]
fn annex_b_block_function_hoists_as_var_binding() {
    assert_stdout(
        r#"
if (true) { function annexB() { return "b" } }
console.log(typeof annexB, annexB(), "annexB" in globalThis);
"#,
        "function b true\n",
    );
}

#[test]
fn direct_eval_sees_and_mutates_global_lexicals() {
    assert_stdout(
        r#"
let g = 1;
console.log(eval("typeof g"), eval("g + 1"));
eval("g = 3");
console.log(g);
var vv = "outer";
console.log(eval("vv"));
"#,
        "number 2\n3\nouter\n",
    );
}

#[test]
fn strict_direct_eval_assigns_existing_script_globals() {
    // 严格 eval 体内写脚本全局 var：绑定经全局对象记录解析后按 [[Set]] 写入；
    // 确实未声明的名字才抛 ReferenceError（Node 口径 "x is not defined"）。
    assert_stdout(
        r#"
var x = 1;
eval('"use strict"; x = 5');
console.log(x, globalThis.x);
try { eval('"use strict"; zz = 9') } catch (e) { console.log(e.constructor.name, e.message) }
"#,
        "5 5\nReferenceError zz is not defined\n",
    );
}

#[test]
fn direct_eval_var_creates_configurable_global_property() {
    // EvalDeclarationInstantiation：直接 eval 引入的 var 是可删除全局属性
    // （CreateGlobalVarBinding(N, true)）；显式 var 保持不可配置。
    assert_stdout(
        r#"
eval("var q = 1");
console.log(globalThis.q, Object.getOwnPropertyDescriptor(globalThis, "q").configurable);
var w = 2;
console.log(Object.getOwnPropertyDescriptor(globalThis, "w").configurable);
console.log(delete q, typeof q, delete w, typeof w);
"#,
        "1 true\nfalse\ntrue undefined false number\n",
    );
}

#[test]
fn module_mode_keeps_module_scoped_var() {
    // 模块模式（默认 `run -e`）：顶层 var 是模块作用域绑定，不进全局对象。
    let (exit, stdout, stderr) = {
        let (exit, stdout, stderr) =
            wjsm_cli::run_source_in_process("var x = 1; console.log(typeof globalThis.x, x);");
        (
            exit,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )
    };
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, "undefined 1\n");
}

#[test]
fn redeclaration_of_lexical_name_is_rejected() {
    // 同脚本内 let 重复声明：编译期 SyntaxError（早错误）。
    let (exit, _stdout, stderr) = run_script("let dup = 1; let dup = 2;");
    assert_ne!(exit, 0);
    assert!(stderr.contains("dup"), "stderr: {stderr}");
}
