// Regression tests for the Layer 3 module-level GC analysis that gates
// safepoint-spill omission around `Call` instructions.
//
// These tests assert the decision the backend actually consults rather than
// pattern-matching generated WAT. In the WAT dump a safepoint-spill prologue is
// byte-for-byte ambiguous with ordinary call-argument marshalling: the spill
// prologue (`compiler_instructions.rs::emit_safepoint_spill_prologue`) and the
// per-argument shadow-stack push inside `compile_call_with_new_target` both emit
// the same `global.get`/`local.set`/`i64.store`/`i32.const`/`i32.add`/`global.set`
// sequence, and several other safepoints (the implicit `arguments` object,
// object/array literals) emit their own spills in the same function body. There
// is no robust textual signature that isolates "the spill for this one call".
//
// The single branch that decides whether the spill is emitted is
// `GcAnalysis::call_may_trigger_gc(caller, callee_value)` (consulted in
// `compiler_instructions.rs`, `Instruction::Call` arm). Asserting that predicate
// directly is precise and immune to codegen formatting / function-index drift.
// End-to-end spill *correctness* is covered separately by the runtime GC fixtures
// (`happy__gc_spill_stress`, `happy__gc_async_await`, `happy__gc_safepoint_local`,
// `happy__gc_long_loop`), which execute and would miscompute or trap if a needed
// spill were dropped.
//
// Each snippet gives its callee an `arguments` parameter. That suppresses the
// implicit arguments-object materialisation (`CollectRestArgs` +
// `create_mapped_arguments_object`, both may-GC), which otherwise marks *every*
// function as may-GC and would mask the property under test.

use anyhow::Result;
use wjsm_backend_wasm::GcAnalysis;
use wjsm_ir::{FunctionId, Instruction, Program, ValueId};
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

fn lower(source: &str) -> Result<Program> {
    Ok(lower_module(parse_module(source)?, false)?)
}

/// FunctionId of the function with the given (declared) IR name.
fn function_id(program: &Program, name: &str) -> Option<FunctionId> {
    program
        .functions()
        .iter()
        .position(|f| f.name() == name)
        .map(|i| FunctionId(i as u32))
}

/// The callee `ValueId` of the single `Call` instruction inside `outer`.
/// Panics unless `outer` exists and contains exactly one `Call`, which keeps the
/// snippets honest (a stray extra call would otherwise silently pass).
fn sole_outer_call(program: &Program) -> (FunctionId, ValueId) {
    let outer = function_id(program, "outer").expect("snippet must define `outer`");
    let callees: Vec<ValueId> = program.functions()[outer.0 as usize]
        .blocks()
        .iter()
        .flat_map(|bb| bb.instructions())
        .filter_map(|ins| match ins {
            Instruction::Call { callee, .. } => Some(*callee),
            _ => None,
        })
        .collect();
    assert_eq!(
        callees.len(),
        1,
        "expected exactly one Call in `outer`, found {}",
        callees.len()
    );
    (outer, callees[0])
}

/// Does `outer` resolve `callee_fn` as a known (hoisted-declaration) callee?
fn outer_knows(program: &Program, outer: FunctionId, callee_fn: FunctionId) -> bool {
    program.functions()[outer.0 as usize]
        .known_callee_vars()
        .values()
        .any(|&f| f == callee_fn)
}

#[test]
fn known_no_gc_callee_omits_safepoint_spill() -> Result<()> {
    // `inc` is a nested function declaration (so `outer` records it in
    // `known_callee_vars`) whose body is pure arithmetic — genuinely
    // non-allocating. The call must be flagged no-GC so the spill is dropped.
    let program = lower(
        r#"
function outer(o) {
  function inc(arguments) { return arguments + 1; }
  let a = { v: 1 };
  return inc(o) + a.v;
}
console.log(outer(1));
"#,
    )?;
    let (outer, callee) = sole_outer_call(&program);
    let analysis = GcAnalysis::analyze(&program);
    let inc = function_id(&program, "inc").expect("`inc` declaration");

    assert!(
        !analysis.function_may_gc(inc),
        "`inc` is pure arithmetic and must be analysed as non-allocating"
    );
    assert!(
        outer_knows(&program, outer, inc),
        "`outer` must resolve `inc` as a known callee for the optimization to apply"
    );
    assert!(
        !analysis.call_may_trigger_gc(outer, callee),
        "known no-GC callee must be flagged so the backend omits the safepoint spill"
    );
    Ok(())
}

#[test]
fn unknown_function_expression_callee_forces_conservative_spill() -> Result<()> {
    // `f` is a function *expression* assigned to a local. It is itself
    // non-allocating, but a function expression never enters `known_callee_vars`,
    // so the callee is unknown and must conservatively spill — proving the spill
    // is forced by unknown-ness, not by the target being may-GC.
    let program = lower(
        r#"
function outer(o) {
  let f = function (arguments) { return arguments + 1; };
  let a = { v: 1 };
  return f(o) + a.v;
}
console.log(outer(1));
"#,
    )?;
    let (outer, callee) = sole_outer_call(&program);
    let analysis = GcAnalysis::analyze(&program);

    assert!(
        program.functions()[outer.0 as usize]
            .known_callee_vars()
            .is_empty(),
        "function-expression callee must not be recorded as a known callee"
    );
    // The callee target is the only user function besides `outer`/`$module_main`.
    let target = (0..program.functions().len() as u32)
        .map(FunctionId)
        .find(|&f| {
            let n = program.functions()[f.0 as usize].name();
            n != "outer" && n != "$module_main"
        })
        .expect("function-expression body");
    assert!(
        !analysis.function_may_gc(target),
        "the callee target is itself non-allocating (so the spill is due to unknown-ness)"
    );
    assert!(
        analysis.call_may_trigger_gc(outer, callee),
        "unknown callee must conservatively force a safepoint spill"
    );
    Ok(())
}

#[test]
fn known_but_transitively_allocating_callee_spills() -> Result<()> {
    // `inner` IS a known callee (nested declaration → recorded in
    // `outer.known_callee_vars`), and its `arguments` parameter suppresses the
    // implicit arguments object, so its may-GC status comes purely from the object
    // literal it allocates. The fixed-point analysis must propagate that across
    // the known-callee edge and still require a spill at the call site.
    let program = lower(
        r#"
function outer(o) {
  function inner(arguments) { return { v: arguments }; }
  let a = { v: 1 };
  return inner(o).v + a.v;
}
console.log(outer(1));
"#,
    )?;
    let (outer, callee) = sole_outer_call(&program);
    let analysis = GcAnalysis::analyze(&program);
    let inner = function_id(&program, "inner").expect("`inner` declaration");

    assert!(
        analysis.function_may_gc(inner),
        "`inner` allocates an object literal and must be analysed as may-GC"
    );
    assert!(
        outer_knows(&program, outer, inner),
        "`inner` must be resolved as a known callee so the transitive edge is exercised"
    );
    assert!(
        analysis.call_may_trigger_gc(outer, callee),
        "known but transitively-allocating callee must still spill"
    );
    Ok(())
}
