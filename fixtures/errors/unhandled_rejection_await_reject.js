// await 已处理内层 promise（不得重复报告），但外层 async promise
// 未被任何 handler 处理时仍须报告其 rejection——且只报告一次。
async function f() {
  await Promise.reject(new Error("await boom"));
}
f();
