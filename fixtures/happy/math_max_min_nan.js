// Math.max / Math.min：任一参数为 NaN 时结果必须为 NaN（ECMA-262）
console.log(Math.max(1, NaN, 3));
console.log(Math.min(1, NaN, 3));
console.log(Math.max(NaN));
console.log(Math.max(1, 2, 3));
console.log(Math.min(1, 2, 3));
console.log(Math.max());
console.log(Math.min());