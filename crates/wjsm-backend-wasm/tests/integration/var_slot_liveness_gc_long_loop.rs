//! Regression: handle in `const tmp = { ... }` must occupy the slot at later safepoints
//! (e.g. `get_prop tmp.x`) even when there is no `load var tmp` between store and safepoint.

use anyhow::Result;
use wjsm_backend_wasm::analysis_liveness::compute_var_liveness;
use wjsm_backend_wasm::analysis_value_ty::infer_value_and_var_ty;
use wjsm_ir::{BasicBlockId, Function, Instruction, Program};
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

// `tmp` 逃逸（写入外层 saved）阻止 escape_scalar 消除，保留下面的 liveness
// 回归场景；若 tmp 不逃逸，EA 会把对象标量替换掉，循环体不再有 get_prop/NewObject。
const SOURCE: &str = r#"
let total = 0;
let saved = null;
for (let i = 0; i < 200000; i++) {
  const tmp = { x: i, y: i + 1 };
  total += tmp.x;
  saved = tmp;
}
console.log("done", total > 0);
"#;
fn main_fn(program: &Program) -> &Function {
    program
        .functions()
        .iter()
        .find(|f| f.name().contains("module_main"))
        .expect("module_main")
}

/// `tmp` 的 IR 名带作用域前缀（`$<scope>.tmp`），而 for 头部绑定自持一个词法
/// 作用域，故循环体的作用域号会随词法嵌套变化。这里按后缀解析实际名字，
/// 使断言锁定「tmp 槽位在安全点被占用」这一契约本身，而非某个具体作用域编号。
fn tmp_var_name(function: &Function) -> String {
    function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .find_map(|ins| match ins {
            Instruction::StoreVar { name, .. } if name.ends_with(".tmp") => Some(name.clone()),
            _ => None,
        })
        .expect("loop body must store the tmp binding")
}

#[test]
fn gc_long_loop_tmp_slot_occupied_at_get_prop() -> Result<()> {
    let program = lower_module(parse_module(SOURCE)?, false)?;
    let f = main_fn(&program);
    let var_live = compute_var_liveness(f);
    let (_vty, var_ty) = infer_value_and_var_ty(&program, f);

    let bb2 = BasicBlockId(2);
    let get_prop_idx = f
        .blocks()
        .iter()
        .find(|b| b.id() == bb2)
        .expect("bb2")
        .instructions()
        .iter()
        .position(|ins| matches!(ins, Instruction::GetProp { .. }))
        .expect("get_prop in loop body");

    let tmp = tmp_var_name(f);
    let at_get_prop = var_live
        .get(&(bb2, get_prop_idx))
        .cloned()
        .unwrap_or_default();
    assert!(
        at_get_prop.contains(&tmp),
        "tmp slot must be occupied before get_prop (was {:?})",
        at_get_prop
    );
    assert_eq!(
        var_ty.get(&tmp),
        Some(&wjsm_backend_wasm::analysis_value_ty::ValueTy::Handle)
    );
    let new_obj_idx = f
        .blocks()
        .iter()
        .find(|b| b.id() == bb2)
        .unwrap()
        .instructions()
        .iter()
        .position(|ins| matches!(ins, Instruction::NewObject { .. }))
        .expect("new_object");
    let at_new_obj = var_live
        .get(&(bb2, new_obj_idx))
        .cloned()
        .unwrap_or_default();
    assert!(
        at_new_obj.contains(&tmp),
        "prior iteration tmp must occupy slot at next new_object safepoint: {:?}",
        at_new_obj
    );
    Ok(())
}
