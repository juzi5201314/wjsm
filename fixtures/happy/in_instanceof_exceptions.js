// 普通 `in` / `instanceof` 的异常传播（ES §13.10.1 RelationalExpression /
// InstanceofOperator）：LHS/RHS 求值异常先传播并短路后续求值；`in` 的 RHS
// 非对象抛 TypeError（先于 ToPropertyKey）；instanceof 的 RHS 非对象/非可
// 调用与非对象 prototype 抛 TypeError；Proxy has trap 与 @@hasInstance
// 用户码异常原样传播；文案与 V8/Node 对齐。generator 体内可本地捕获；
// async 体内沿状态机约定以 promise rejection 传播。

function show(label, fn) {
  try {
    const result = fn();
    console.log(label + " ok: " + result);
  } catch (e) {
    console.log(label + " " + e.constructor.name + " | " + e.message);
  }
}

// —— in：RHS 非对象（null/undefined/原始值）抛 TypeError ——
show("in-null", () => "x" in null);
show("in-undef", () => "x" in undefined);
show("in-num", () => "x" in 5);
show("in-str", () => "x" in "str");
show("in-bool", () => "x" in true);
show("in-sym-key", () => Symbol("k") in null);
show("in-num-key", () => 1 in null);

// —— in：RHS/LHS 求值异常传播，LHS 抛错短路 RHS ——
show("in-rhs-throw", () => "k" in (() => { throw new RangeError("rhs-boom"); })());
let rhsRan = false;
show("in-lhs-throw", () => (() => { throw new Error("lhs-first"); })() in ((rhsRan = true), {}));
console.log("rhsRan: " + rhsRan);

// —— instanceof：RHS 非对象 / 非可调用 ——
show("inst-null", () => ({}) instanceof null);
show("inst-undef", () => ({}) instanceof undefined);
show("inst-num", () => ({}) instanceof 5);
show("inst-str", () => ({}) instanceof "s");
show("inst-plain", () => ({}) instanceof {});
show("inst-regexp-rhs", () => ({}) instanceof /re/);

// —— instanceof：RHS/LHS 求值异常传播，LHS 抛错短路 RHS ——
show("inst-rhs-throw", () => ({}) instanceof (() => { throw new Error("rhs-inst"); })());
let instRhsRan = false;
show("inst-lhs-throw", () =>
  (() => { throw new Error("inst-lhs-first"); })() instanceof ((instRhsRan = true), Object));
console.log("instRhsRan: " + instRhsRan);

// —— instanceof：OrdinaryHasInstance 的非对象 prototype ——
const arrow = () => {};
show("inst-arrow", () => ({}) instanceof arrow);
function badProto() {}
badProto.prototype = 5;
show("inst-bad-proto", () => ({}) instanceof badProto);
// prototype 为 RegExp（exotic 对象）合法：沿原型链返回 false，不抛。
function reProto() {}
reProto.prototype = /x/;
show("inst-re-proto", () => ({}) instanceof reProto);

// —— Proxy has trap / @@hasInstance 用户码异常传播 ——
show("in-proxy-trap-throw", () => "x" in new Proxy({}, { has() { throw new RangeError("trap-boom"); } }));
show("inst-hasinstance-throw", () =>
  ({}) instanceof ({ [Symbol.hasInstance]() { throw new RangeError("hi-boom"); } }));

// —— 正常路径不回归 ——
show("in-ok", () => "a" in { a: 1 });
show("in-arr", () => 0 in [1]);
show("in-proto", () => "toString" in {});
show("inst-ok", () => [] instanceof Array);
show("inst-neg", () => ({}) instanceof Array);
show("inst-null-lhs", () => null instanceof Object);

// —— generator 体内本地捕获（sync 状态外同款分叉） ——
function* gen() {
  try {
    yield "x" in null;
  } catch (e) {
    yield "gen-caught: " + e.message;
  }
}
console.log(String(gen().next().value));

// —— async 体内：沿状态机约定，异常以 promise rejection 传播 ——
async function asyncIn() {
  await Promise.resolve();
  return "x" in null;
}
async function asyncInstanceof() {
  await Promise.resolve();
  return ({}) instanceof 5;
}
async function asyncRhsThrow() {
  await Promise.resolve();
  return "k" in (() => { throw new RangeError("async-rhs"); })();
}
asyncIn()
  .then(
    () => console.log("asyncIn resolved"),
    (e) => console.log("asyncIn rejected: " + e.constructor.name + " | " + e.message),
  )
  .then(() => asyncInstanceof())
  .then(
    () => console.log("asyncInstanceof resolved"),
    (e) => console.log("asyncInstanceof rejected: " + e.constructor.name + " | " + e.message),
  )
  .then(() => asyncRhsThrow())
  .then(
    () => console.log("asyncRhsThrow resolved"),
    (e) => console.log("asyncRhsThrow rejected: " + e.constructor.name + " | " + e.message),
  );
