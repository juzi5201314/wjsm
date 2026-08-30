//! 函数声明自引用：体内读取自身名应走外层 `LoadVar` + `direct_call`，
//! 不得登记闭包捕获（否则自递归经 env `GetProp` 动态分派）。

use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const FIB: &str = r#"
function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
"#;

#[test]
fn fn_decl_self_reference_is_direct_callable_without_captures() {
    let module = parse_module(FIB).expect("解析 fib 源码");
    let program = lower_module(module, false).expect("lowering fib");
    let fib = program
        .functions()
        .iter()
        .find(|function| function.name() == "fib")
        .expect("fib 函数应存在");
    assert!(
        fib.captured_names().is_empty(),
        "自引用不应产生闭包捕获: {:?}",
        fib.captured_names()
    );
    assert!(
        fib.direct_callable(),
        "无捕获的 fib 应标记 direct_callable"
    );
    let uses_env_get = fib.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                wjsm_ir::Instruction::GetProp { .. }
            ) && block.instructions().iter().any(|ins| {
                matches!(
                    ins,
                    wjsm_ir::Instruction::LoadVar { name, .. } if name == "$env"
                )
            })
        })
    });
    assert!(
        !uses_env_get,
        "自递归 fib 不应经 env GetProp 解析自身"
    );
}
