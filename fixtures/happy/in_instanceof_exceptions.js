// 普通 `in` / `instanceof` 的异常传播（ES §13.10.1 RelationalExpression /
// InstanceofOperator）：LHS/RHS 求值异常先传播并短路后续求值；`in` 的 RHS
// 非对象抛 TypeError（先于 ToPropertyKey）；instanceof 的 RHS 非对象/非可
// 调用与非对象 prototype 抛 TypeError；bound function 委托
// [[BoundTargetFunction]]；Proxy has trap 与 @@hasInstance 用户码异常原样
// 传播；文案与 V8/Node 对齐。generator 体内可本地捕获；async 体内沿状态机
// 约定以 promise rejection 传播。
// 用例写成顶层 try/catch 语句（不用箭头回调包装）以保持编译耗时可控。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

// —— in：RHS 非对象（null/undefined/原始值）抛 TypeError ——
try { console.log("in-null ok: " + ("x" in null)); } catch (e) { caught("in-null", e); }
try { console.log("in-undef ok: " + ("x" in undefined)); } catch (e) { caught("in-undef", e); }
try { console.log("in-num ok: " + ("x" in 5)); } catch (e) { caught("in-num", e); }
try { console.log("in-str ok: " + ("x" in "str")); } catch (e) { caught("in-str", e); }
try { console.log("in-bool ok: " + ("x" in true)); } catch (e) { caught("in-bool", e); }
try { console.log("in-sym-key ok: " + (Symbol("k") in null)); } catch (e) { caught("in-sym-key", e); }
try { console.log("in-num-key ok: " + (1 in null)); } catch (e) { caught("in-num-key", e); }

// —— in：RHS/LHS 求值异常传播，LHS 抛错短路 RHS ——
function throwRange(message) {
  throw new RangeError(message);
}
function throwError(message) {
  throw new Error(message);
}
try { console.log("in-rhs-throw ok: " + ("k" in throwRange("rhs-boom"))); } catch (e) { caught("in-rhs-throw", e); }
let rhsRan = false;
try { console.log("in-lhs-throw ok: " + (throwError("lhs-first") in ((rhsRan = true), {}))); } catch (e) { caught("in-lhs-throw", e); }
console.log("rhsRan: " + rhsRan);

// —— instanceof：RHS 非对象 / 非可调用 ——
try { console.log("inst-null ok: " + ({} instanceof null)); } catch (e) { caught("inst-null", e); }
try { console.log("inst-undef ok: " + ({} instanceof undefined)); } catch (e) { caught("inst-undef", e); }
try { console.log("inst-num ok: " + ({} instanceof 5)); } catch (e) { caught("inst-num", e); }
try { console.log("inst-str ok: " + ({} instanceof "s")); } catch (e) { caught("inst-str", e); }
try { console.log("inst-plain ok: " + ({} instanceof {})); } catch (e) { caught("inst-plain", e); }
try { console.log("inst-regexp-rhs ok: " + ({} instanceof /re/)); } catch (e) { caught("inst-regexp-rhs", e); }

// —— instanceof：RHS/LHS 求值异常传播，LHS 抛错短路 RHS ——
try { console.log("inst-rhs-throw ok: " + ({} instanceof throwError("rhs-inst"))); } catch (e) { caught("inst-rhs-throw", e); }
let instRhsRan = false;
try { console.log("inst-lhs-throw ok: " + (throwError("inst-lhs-first") instanceof ((instRhsRan = true), Object))); } catch (e) { caught("inst-lhs-throw", e); }
console.log("instRhsRan: " + instRhsRan);

// —— instanceof：OrdinaryHasInstance 的非对象 prototype ——
const arrow = () => {};
try { console.log("inst-arrow ok: " + ({} instanceof arrow)); } catch (e) { caught("inst-arrow", e); }
function badProto() {}
badProto.prototype = 5;
try { console.log("inst-bad-proto ok: " + ({} instanceof badProto)); } catch (e) { caught("inst-bad-proto", e); }
// prototype 为 RegExp（exotic 对象）合法：沿原型链返回 false，不抛。
function reProto() {}
reProto.prototype = /x/;
try { console.log("inst-re-proto ok: " + ({} instanceof reProto)); } catch (e) { caught("inst-re-proto", e); }

// —— OrdinaryHasInstance 步骤 2：bound function 委托 [[BoundTargetFunction]] ——
class BoundBase {}
const BoundC = BoundBase.bind(null);
try { console.log("inst-bound-neg ok: " + ({} instanceof BoundC)); } catch (e) { caught("inst-bound-neg", e); }
try { console.log("inst-bound-pos ok: " + (new BoundBase() instanceof BoundC)); } catch (e) { caught("inst-bound-pos", e); }
try { console.log("inst-bound-bound ok: " + (new BoundBase() instanceof BoundC.bind(null))); } catch (e) { caught("inst-bound-bound", e); }

// —— Proxy has trap / @@hasInstance 用户码异常传播 ——
const trapProxy = new Proxy({}, { has() { throw new RangeError("trap-boom"); } });
try { console.log("in-proxy-trap-throw ok: " + ("x" in trapProxy)); } catch (e) { caught("in-proxy-trap-throw", e); }
const hiThrow = { [Symbol.hasInstance]() { throw new RangeError("hi-boom"); } };
try { console.log("inst-hasinstance-throw ok: " + ({} instanceof hiThrow)); } catch (e) { caught("inst-hasinstance-throw", e); }

// —— 正常路径不回归 ——
console.log("in-ok ok: " + ("a" in { a: 1 }));
console.log("in-arr ok: " + (0 in [1]));
console.log("in-proto ok: " + ("toString" in {}));
console.log("inst-ok ok: " + ([] instanceof Array));
console.log("inst-neg ok: " + ({} instanceof Array));
console.log("inst-null-lhs ok: " + (null instanceof Object));

// —— generator 体内本地捕获（同款分叉在 sync generator 内可用） ——
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
  return {} instanceof 5;
}
async function asyncRhsThrow() {
  await Promise.resolve();
  return "k" in throwRange("async-rhs");
}
asyncIn()
  .then(
    () => console.log("asyncIn resolved"),
    (e) => caught("asyncIn rejected:", e),
  )
  .then(() => asyncInstanceof())
  .then(
    () => console.log("asyncInstanceof resolved"),
    (e) => caught("asyncInstanceof rejected:", e),
  )
  .then(() => asyncRhsThrow())
  .then(
    () => console.log("asyncRhsThrow resolved"),
    (e) => caught("asyncRhsThrow rejected:", e),
  );
