// &&= — 左值为真时赋值
let a = true;
a &&= false;
console.log(a);

// ||= — 左值为假时赋值
let b = false;
b ||= true;
console.log(b);

// ??= — 左值为 null/undefined 时赋值
let c = null;
c ??= 42;
console.log(c);

// ??= — 左值非 nullish 时短路
let d = 1;
d ??= 2;
console.log(d);

// 短路验证：右侧不应被求值
let sideEffect = false;
let e = true;
e ||= (sideEffect = true, true);  // 短路，不应执行右侧
console.log(sideEffect);
console.log(e);

let f = false;
f &&= (sideEffect = true, false);  // 短路，不应执行右侧
console.log(sideEffect);
console.log(f);
