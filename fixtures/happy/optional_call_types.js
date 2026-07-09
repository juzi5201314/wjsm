// Issue #28: optional call must dispatch by callable tag like normal Call.

console.log?.("native_optional");

// bind 用户函数（非 native）验证 TAG_BOUND 可选调用；
// console.log.bind 依赖 native 可调用值上的 .bind 属性路径，单独覆盖。
function printMsg(msg) {
  console.log(msg);
}
const fn = printMsg.bind(null);
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
