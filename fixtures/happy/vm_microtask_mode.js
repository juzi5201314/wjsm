// microtaskMode: default 不在 run 边界排空；afterEvaluate 会 drain 到稳态。
// 嵌套函数 FunctionRef 挂主 __table 并保活 eval Instance，可写回 sandbox free-var。
const vm = require("vm");

const def = vm.createContext({ n: 0 });
vm.runInContext("queueMicrotask(() => { n = 1 })", def);
console.log("default_after_run", def.n);

const after = vm.createContext({ n: 0 }, { microtaskMode: "afterEvaluate" });
vm.runInContext("queueMicrotask(() => { n = 1 })", after);
console.log("afterEvaluate_after_run", after.n);

const nested = vm.createContext({ n: 0 }, { microtaskMode: "afterEvaluate" });
vm.runInContext(
  "queueMicrotask(() => { n = 1; queueMicrotask(() => { n = 2 }) })",
  nested
);
console.log("afterEvaluate_nested", nested.n);

// function 表达式同样 durable
const afterFn = vm.createContext({ n: 0 }, { microtaskMode: "afterEvaluate" });
vm.runInContext("queueMicrotask(function () { n = 3 })", afterFn);
console.log("afterEvaluate_fn", afterFn.n);
