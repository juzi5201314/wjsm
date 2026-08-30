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

const SOURCE_HYPOT: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
}
const POINTS = Array.from({ length: 3 }, (_, i) => new Point(i + 1, i + 2));
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].norm;
}
"#;

const SOURCE_HYPOT_WORK: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
  scale(factor) {
    return new Point(this.x * factor, this.y * factor);
  }
}
const POINTS = Array.from({ length: 3 }, (_, i) => new Point(i, i + 1));
function work() {
  let total = 0;
  for (let i = 0; i < POINTS.length; i++) {
    const p = POINTS[i];
    total += p.norm;
    const scaled = p.scale(0.5);
    total += scaled.x + scaled.y;
  }
  return total;
}
"#;

fn program_has_latched_getprop(program: &wjsm_ir::Program) -> bool {
    program.functions().iter().any(|function| {
        function.blocks().iter().any(|block| {
            block.instructions().iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::GetProp {
                        latch: Some(_),
                        latch_template: Some(_),
                        ..
                    }
                )
            })
        })
    })
}

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

/// 守卫的 array 必须与 `GuardElementsKind` 同块定义（pre-header 已外提
/// LoadEnvSlot），不能仍留在循环体 GetElem 块里。
fn guard_array_defined_in_preheader(program: &wjsm_ir::Program) -> bool {
    program.functions().iter().all(|function| {
        let mut defs = std::collections::HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Some(dest) = wjsm_optimize::instruction_dest(instruction) {
                    defs.insert(dest, block.id());
                }
            }
        }
        function.blocks().iter().all(|block| {
            block.instructions().iter().all(|instruction| {
                let Instruction::GuardElementsKind { array, .. } = instruction else {
                    return true;
                };
                defs.get(array) == Some(&block.id())
            })
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
    assert!(
        guard_array_defined_in_preheader(&program),
        "嵌套 work 的守卫 array 必须在 pre-header 定义：{text}"
    );
}

#[test]
fn array_from_hypot_getter_loop_emits_elem_guard() {
    let module = parse_module(SOURCE_HYPOT).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let text = dump_instructions(&program);
    assert!(
        program_has_elem_guard(&program),
        "Array.from + hypot getter 循环应发出 GuardElementsKind：{text}"
    );
    assert!(
        program_has_latched_getprop(&program),
        "hypot getter GetProp 应带 latch：{text}"
    );
}

fn dump_function(program: &wjsm_ir::Program, name: &str) -> String {
    let Some(function) = program
        .functions()
        .iter()
        .find(|function| function.name() == name)
    else {
        return format!("missing function {name}");
    };
    let mut lines = Vec::new();
    for block in function.blocks() {
        lines.push(format!("bb{}:", block.id().0));
        for instruction in block.instructions() {
            lines.push(format!("  {instruction}"));
        }
        lines.push(format!("  {}", block.terminator()));
    }
    lines.join("\n")
}

#[test]
fn object_props_work_loop_emits_elem_guard() {
    let module = parse_module(SOURCE_HYPOT_WORK).expect("解析");
    let program = lower_module(module, false).expect("lowering");
    let text = dump_function(&program, "work");
    assert!(
        program_has_elem_guard(&program),
        "object-props 形态 work 循环应发出 GuardElementsKind：{text}"
    );
    assert!(
        program_has_latched_getprop(&program),
        "object-props work 应对 POINTS[i] 属性读取加 latch：{text}"
    );
    assert!(
        guard_array_defined_in_preheader(&program),
        "GuardElementsKind 的 array 必须在 pre-header 定义：{text}"
    );
}
