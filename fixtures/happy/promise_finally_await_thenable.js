// #140: Promise.prototype.finally 必须 await onFinally 返回的 thenable
// onFinally 返回 reject 的 thenable -> 结果 promise 以该原因 reject
Promise.resolve(1)
  .finally(() => Promise.reject("boom"))
  .then((v) => console.log("a-resolved", v), (e) => console.log("a-rejected", e));
// onFinally 返回 fulfill 的 thenable -> 保留原始值
Promise.resolve(2)
  .finally(() => Promise.resolve("ignored"))
  .then((v) => console.log("b-resolved", v), (e) => console.log("b-rejected", e));
// onFinally 抛异常的 thenable（then 内 throw）-> 以抛出值 reject
Promise.resolve(3)
  .finally(() => Promise.resolve().then(() => { throw new Error("late"); }))
  .then((v) => console.log("c-resolved", v), (e) => console.log("c-rejected", e.message));
