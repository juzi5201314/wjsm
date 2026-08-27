// 跨函数前向引用 TDZ：getter 在声明执行前触发读取，未捕获时以
// ReferenceError 终止进程。
let x = { get self() { return x; } }.self;
console.log("unreachable", x);
