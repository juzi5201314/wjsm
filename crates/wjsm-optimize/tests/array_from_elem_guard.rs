//! `Array.from` + 统一类构造器的数组绑定应带上 elem-guard。

use wjsm_ir::Instruction;
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const SOURCE: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
}
const POINTS = Array.from({ length: 3 }, (_, i) => new Point(i + 1, i + 2));
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].x;
}
"#;

const SOURCE_NESTED: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
}
const POINTS = Array.from({ length: 3 }, (_, i) => new Point(i + 1, i + 2));
function work() {
  let sum = 0;
  for (let i = 0; i < POINTS.length; i++) {
    sum += POINTS[i].x;
  }
  return sum;
}
"#;

fn program_has_elem_guard(program: &wjsm_ir::Program) -> bool {
    program.functions().iter().any(|function| {
        function.blocks().iter().any(|block| {
            block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::GuardElementsKind { .. }))
        })
    })
}

fn dump_instructions(program: &wjsm_ir::Program) -> String {
    program
        .functions()
        .iter()
        .flat_map(|function| function.blocks())
        .flat_map(|block| block.instructions())
        .map(|instruction| format!("{instruction}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn array_from_ctor_loop_emits_elem_guard() {
    let module = parse_module(SOURCE).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let text = dump_instructions(&program);
    assert!(
        program_has_elem_guard(&program),
        "Array.from + new Point 循环应发出 GuardElementsKind：{text}"
    );
}

#[test]
fn array_from_ctor_nested_work_emits_elem_guard() {
    let module = parse_module(SOURCE_NESTED).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let text = dump_instructions(&program);
    assert!(
        program_has_elem_guard(&program),
        "捕获 POINTS 的 work 循环也应发出 GuardElementsKind：{text}"
    );
}
