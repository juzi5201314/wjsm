// 未捕获的 `"x" in null` TypeError（ES §13.10.1 步骤 5：RHS 非对象）必须
// 终止执行并以运行时错误退出，文案与 V8/Node 一致，不得吞掉返回 false。
const key = "x";
console.log(key in null);
console.log("unreachable");
