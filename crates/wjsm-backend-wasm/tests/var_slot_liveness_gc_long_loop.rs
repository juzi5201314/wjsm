//! Regression: handle in `const tmp = { ... }` must occupy the slot at later safepoints
//! (e.g. `get_prop tmp.x`) even when there is no `load var tmp` between store and safepoint.

use anyhow::Result;
use wjsm_backend_wasm::analysis_liveness::compute_var_liveness;
use wjsm_backend_wasm::analysis_value_ty::infer_value_and_var_ty;
use wjsm_ir::{BasicBlockId, Function, Instruction, Program};
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

const SOURCE: &str = r#"
let total = 0;
for (let i = 0; i < 200000; i++) {
  const tmp = { x: i, y: i + 1 };
  total += tmp.x;
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

    let at_get_prop = var_live
        .get(&(bb2, get_prop_idx))
        .cloned()
        .unwrap_or_default();
    assert!(
        at_get_prop.contains("$1.tmp"),
        "tmp slot must be occupied before get_prop (was {:?})",
        at_get_prop
    );
    assert_eq!(
        var_ty.get("$1.tmp"),
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
        at_new_obj.contains("$1.tmp"),
        "prior iteration tmp must occupy slot at next new_object safepoint: {:?}",
        at_new_obj
    );
    Ok(())
}