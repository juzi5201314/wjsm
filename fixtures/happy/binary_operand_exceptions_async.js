// Binary 操作数异常在 generator / async 体内的传播：generator 体内可本地
// 捕获；async 体内沿状态机约定以 promise rejection 传播。同步场景在
// binary_operand_exceptions.js 与 binary_operand_exceptions_compare.js
//（拆分控制顶层函数编译耗时）。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

function throwError(message) {
  throw new Error(message);
}

// —— generator 体内本地捕获（同款分叉在 sync generator 内可用） ——
function* gen() {
  try {
    yield "g: " + throwError("gen-boom");
  } catch (e) {
    yield "gen-caught: " + e.message;
  }
}
console.log(String(gen().next().value));

// —— async 体内：沿状态机约定，异常以 promise rejection 传播 ——
async function asyncAdd() {
  await Promise.resolve();
  return "x: " + throwError("async-add");
}
async function asyncEq() {
  await Promise.resolve();
  return throwError("async-eq") == 1;
}
async function asyncNeq() {
  await Promise.resolve();
  return throwError("async-neq") != 1;
}
async function asyncSeq() {
  await Promise.resolve();
  return throwError("async-seq") === 1;
}
async function asyncLt() {
  await Promise.resolve();
  return throwError("async-lt") < 1;
}
asyncAdd()
  .then(
    () => console.log("asyncAdd resolved"),
    (e) => caught("asyncAdd rejected:", e),
  )
  .then(() => asyncEq())
  .then(
    () => console.log("asyncEq resolved"),
    (e) => caught("asyncEq rejected:", e),
  )
  .then(() => asyncNeq())
  .then(
    () => console.log("asyncNeq resolved"),
    (e) => caught("asyncNeq rejected:", e),
  )
  .then(() => asyncSeq())
  .then(
    () => console.log("asyncSeq resolved"),
    (e) => caught("asyncSeq rejected:", e),
  )
  .then(() => asyncLt())
  .then(
    () => console.log("asyncLt resolved"),
    (e) => caught("asyncLt rejected:", e),
  );
