// 未捕获的 async 函数异常必须报告为 unhandled rejection 并影响退出码，
// 不得静默吞掉（此前 async 外层 promise 被整体豁免报告）。
async function boom() {
  throw new Error("async boom");
}
boom();
console.log("after-call");
