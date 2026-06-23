// Issue #28: optional call must dispatch by callable tag like normal Call.

console.log?.("native_optional");
const fn = console.log.bind(null);
fn?.("bound_optional");

const f = function () {
  return "regular_ok";
};
console.log(f?.());

const nullish = null?.();
const undefish = undefined?.();
console.log(nullish === undefined, undefish === undefined);

function makeClosure() {
  return function () {
    return "closure_ok";
  };
}
const g = makeClosure();
console.log(g?.());