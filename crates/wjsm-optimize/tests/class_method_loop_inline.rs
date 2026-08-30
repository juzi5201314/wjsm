//! 循环内 `p.scale()` 累加结果时，阶段 C 应发出 GuardSameFunction，
//! 且内联构造器的 `NewObject` 被标量替换（`scaled.x` / `scaled.y` 不再读堆）。

use wjsm_ir::Instruction;
use wjsm_optimize::terminator_uses;
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
    let new_object_dests: Vec<_> = work
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::NewObject { dest, .. } => Some(*dest),
            _ => None,
        })
        .collect();
    let terminator_hits: Vec<String> = work
        .blocks()
        .iter()
        .filter(|block| {
            terminator_uses(block.terminator())
                .iter()
                .any(|value| new_object_dests.contains(value))
        })
        .map(|block| format!("{}: {}", block.id(), block.terminator()))
        .collect();
    let text: String = work
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .map(|instruction| format!("{instruction}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        has_guard,
        "work 应对 p.scale 发出 GuardSameFunction：{text}"
    );
    assert!(
        new_object_dests.is_empty(),
        "内联构造器的 NewObject 应被标量替换，不应留在 work（terminator 使用={terminator_hits:?}）：\n{text}"
    );
}
