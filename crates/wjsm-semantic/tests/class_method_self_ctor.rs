//! 顶层一次性求值的类：方法体内 `new ClassName` 应折叠为构造器 FunctionRef，
//! 不捕获 classEnv。

use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const POINT: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  scale(factor) {
    return new Point(this.x * factor, this.y * factor);
  }
}
"#;

#[test]
fn toplevel_class_method_new_self_is_direct_callable() {
    let module = parse_module(POINT).expect("解析 Point");
    let program = lower_module(module, false).expect("lowering Point");
    let scale = program
        .functions()
        .iter()
        .find(|function| function.name() == "Point.scale")
        .expect("Point.scale 应存在");
    assert!(
        scale.captured_names().is_empty(),
        "顶层类方法不应捕获类名: {:?}",
        scale.captured_names()
    );
    let uses_env_get = scale.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(instruction, wjsm_ir::Instruction::GetProp { .. })
                && block.instructions().iter().any(|ins| {
                    matches!(
                        ins,
                        wjsm_ir::Instruction::LoadVar { name, .. } if name == "$env"
                    )
                })
        })
    });
    assert!(!uses_env_get, "new Point 不应经 env GetProp 解析类名");
}

#[test]
fn class_in_function_still_captures_self_name() {
    let source = r#"
function make() {
  class C {
    m() { return new C(); }
  }
  return C;
}
"#;
    let module = parse_module(source).expect("解析 make/C");
    let program = lower_module(module, false).expect("lowering make/C");
    let method = program
        .functions()
        .iter()
        .find(|function| function.name() == "C.m")
        .expect("C.m 应存在");
    assert!(
        !method.captured_names().is_empty(),
        "函数内的类每次求值不同，方法仍须捕获类名"
    );
}

fn method_named<'a>(program: &'a wjsm_ir::Program, name: &str) -> &'a wjsm_ir::Function {
    program
        .functions()
        .iter()
        .find(|function| function.name() == name)
        .unwrap_or_else(|| panic!("{name} 应存在"))
}

#[test]
fn class_in_for_loop_still_captures_self_name() {
    let source = r#"
for (let i = 0; i < 3; i++) {
  const K = class C { tag() { return C; } };
}
"#;
    let module = parse_module(source).expect("解析循环内类");
    let program = lower_module(module, false).expect("lowering 循环内类");
    let method = method_named(&program, "C.tag");
    assert!(
        !method.captured_names().is_empty(),
        "循环内每次求值新建构造器，方法须捕获类名: {:?}",
        method.captured_names()
    );
}

#[test]
fn class_in_while_test_still_captures_self_name() {
    // while 条件在 label_stack 压 Loop 之前求值；仅靠 break 标签栈会误折 FunctionRef。
    let source = r#"
let i = 0;
const ks = [];
while (ks.push(class C { tag() { return C; } }) && i++ < 2) {}
"#;
    let module = parse_module(source).expect("解析 while 条件类");
    let program = lower_module(module, false).expect("lowering while 条件类");
    let method = method_named(&program, "C.tag");
    assert!(
        !method.captured_names().is_empty(),
        "while 条件每次求值新建构造器，方法须捕获类名: {:?}",
        method.captured_names()
    );
}

#[test]
fn computed_key_arrow_keeps_class_name_tdz() {
    let source = "class G { [(() => G)()]() {} }";
    let module = parse_module(source).expect("解析计算键箭头");
    let program = lower_module(module, false).expect("lowering 计算键箭头");
    let arrow = program
        .functions()
        .iter()
        .find(|function| function.name().contains("arrow"))
        .expect("计算键箭头应存在");
    assert!(
        arrow
            .captured_names()
            .iter()
            .any(|name| name.contains(".G")),
        "计算键箭头必须保留类名 TDZ 捕获: {:?}",
        arrow.captured_names()
    );
}
