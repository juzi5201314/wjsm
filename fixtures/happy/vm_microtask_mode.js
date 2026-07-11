// microtaskMode: default 不在 run 边界排空；afterEvaluate 会 drain 到稳态。
//
// 注：runInContext 编译出的嵌套函数挂在临时 eval Instance 上，不能作为
// 跨 run 边界的 microtask 回调；这里用主 realm queueMicrotask + sandbox 属性
// 验证 drain 边界语义（与 Node 一致）。
const vm = require("vm");

const def = vm.createContext({ n: 0 });
queueMicrotask(() => {
  def.n = 1;
});
// default：run 结束不 drain → 仍为 0
vm.runInContext("1", def);
console.log("default_after_run", def.n);

const after = vm.createContext({ n: 0 }, { microtaskMode: "afterEvaluate" });
queueMicrotask(() => {
  after.n = 1;
});
// afterEvaluate：run 边界 drain → 1
vm.runInContext("1", after);
console.log("afterEvaluate_after_run", after.n);

// nested microtasks 也 drain 到稳态
const nested = vm.createContext({ n: 0 }, { microtaskMode: "afterEvaluate" });
queueMicrotask(() => {
  nested.n = 1;
  queueMicrotask(() => {
    nested.n = 2;
  });
});
vm.runInContext("1", nested);
console.log("afterEvaluate_nested", nested.n);
