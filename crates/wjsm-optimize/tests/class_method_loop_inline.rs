//! 循环内 `p.scale()` 累加结果时，阶段 C 应发出 GuardSameFunction，
//! 且 `new Point` 的构造器 Throw 菱形不得挡住阶段 A 内联。

use wjsm_ir::Instruction;
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const SOURCE: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  scale(factor) {
    return new Point(this.x * factor, this.y * factor);
  }
}
function work(points) {
  let total = 0;
  for (let i = 0; i < points.length; i++) {
    const scaled = points[i].scale(0.5);
    total += scaled.x + scaled.y;
  }
  return total;
}
"#;

#[test]
fn loop_method_and_constructor_inline() {
    let module = parse_module(SOURCE).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let work = program
        .functions()
        .iter()
        .find(|function| function.name() == "work")
        .expect("work 函数");
    let has_guard = work.blocks().iter().any(|block| {
        block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::GuardSameFunction { .. }))
    });
    let text: String = work
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .map(|instruction| format!("{instruction}"))
        .collect::<Vec<_>>()
        .join("\n");
    let has_set_prop = work.blocks().iter().any(|block| {
        block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::SetProp { .. }))
    });
    assert!(
        has_guard,
        "work 应对 p.scale 发出 GuardSameFunction：{text}"
    );
    assert!(
        has_set_prop,
        "内联 scale 后应级联内联 Point.constructor 的 SetProp：{text}"
    );
}
