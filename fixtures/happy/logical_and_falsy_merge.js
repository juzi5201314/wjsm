// 回归：结构化编译器对"分支直接指向 phi 合并块"错码——`a && b` 在 a 为
// falsy 非零值时曾返回 0（false 路径 phi move 被跳过，读到陈旧 phi_local）。
function f(a, b) { return a && b; }
console.log(f(0, 42) === 0);
console.log(f(null, 42) === null);
console.log(f("", 42) === "");
console.log(f(false, 42) === false);
console.log(f(1, 42) === 42);
console.log(f("x", "y") === "y");
console.log(f(NaN, 7) !== 7);
