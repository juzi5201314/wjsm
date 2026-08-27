// 未捕获的 `obj instanceof 非对象` TypeError（InstanceofOperator 步骤 1）
// 必须终止执行并以运行时错误退出，文案与 V8/Node 一致。
const target = null;
console.log({} instanceof target);
console.log("unreachable");
