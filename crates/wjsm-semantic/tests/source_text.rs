//! [[SourceText]]（ES §20.2.3.5 Function.prototype.toString 步骤 2）在语义层
//! 的捕获：各函数定义形态 lowering 后 IR Function 记录精确的源码切片；
//! 未提供源文本时（HostHasSourceTextAvailable=false）全部为 None。

use std::sync::Arc;

use wjsm_parser::parse_module;
use wjsm_semantic::{lower_module, lower_module_with_source};

const SOURCE: &str = r#"function decl(a, b = 1) { return a; }
const expr = function named(x) { return x; };
const arrow = (a) => a + 1;
async function af(x) { return x; }
function* gf(y) { yield y; }
async function* agf(z) { yield z; }
class C {
  constructor(a) { this.a = a; }
  m(x) { return x; }
  static s() {}
  get g() { return 1; }
  set g(v) {}
  #p() { return 2; }
  static #sp() { return 3; }
}
const obj = { om(x) { return x; }, get p() { return 1; }, set p(v) {} };
"#;

/// 收集 lowering 结果里全部非空 source_text。
fn collected_source_texts(source: &str) -> Vec<String> {
    let module = parse_module(source).expect("解析测试源码");
    let program = lower_module_with_source(module, false, Some(Arc::from(source)), "input")
        .expect("lowering 测试源码");
    let mut texts: Vec<String> = program
        .functions()
        .iter()
        .filter_map(|function| function.source_text().map(str::to_owned))
        .collect();
    texts.sort();
    texts
}

#[test]
fn source_text_captured_per_function_form() {
    let texts = collected_source_texts(SOURCE);
    let class_text = SOURCE
        .lines()
        .skip_while(|line| !line.starts_with("class C"))
        .take_while(|line| *line != "}")
        .chain(std::iter::once("}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut expected = vec![
        "function decl(a, b = 1) { return a; }".to_owned(),
        "function named(x) { return x; }".to_owned(),
        "(a) => a + 1".to_owned(),
        "async function af(x) { return x; }".to_owned(),
        "function* gf(y) { yield y; }".to_owned(),
        "async function* agf(z) { yield z; }".to_owned(),
        // 类构造器的 [[SourceText]] 为整个 class 定义（V8/Node 行为）。
        class_text,
        "m(x) { return x; }".to_owned(),
        // static 前缀不属于 MethodDefinition 的 [[SourceText]]。
        "s() {}".to_owned(),
        "get g() { return 1; }".to_owned(),
        "set g(v) {}".to_owned(),
        "#p() { return 2; }".to_owned(),
        "#sp() { return 3; }".to_owned(),
        "om(x) { return x; }".to_owned(),
        "get p() { return 1; }".to_owned(),
        "set p(v) {}".to_owned(),
    ];
    expected.sort();
    assert_eq!(texts, expected);
}

#[test]
fn source_text_absent_without_module_source() {
    let module = parse_module(SOURCE).expect("解析测试源码");
    let program = lower_module(module, false).expect("lowering 测试源码");
    assert!(
        program
            .functions()
            .iter()
            .all(|function| function.source_text().is_none()),
        "未提供源文本时不应有任何 [[SourceText]]"
    );
}
