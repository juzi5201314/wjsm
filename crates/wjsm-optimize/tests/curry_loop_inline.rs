//! 柯里化 `add(1)(2)` 在循环内应经 inline + 逃逸标量替换，不再每轮 create_closure。

use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const SOURCE: &str = r#"
function work() {
  const add = (a) => (b) => a + b;
  for (let i = 0; i < 2; i++) {
    add(1)(2);
  }
}
"#;

#[test]
fn curry_in_loop_eliminates_per_iteration_create_closure() {
    let module = parse_module(SOURCE).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let work = program
        .functions()
        .iter()
        .find(|function| function.name() == "work")
        .expect("work 函数");
    let text: String = work
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .map(|instruction| format!("{instruction:?}"))
        .collect();
    assert!(
        !text.contains("CreateClosure"),
        "循环体不应残留 create_closure：{text}"
    );
    assert!(
        !text.contains("NewObject"),
        "循环体不应每轮分配闭包 env 对象：{text}"
    );
}
