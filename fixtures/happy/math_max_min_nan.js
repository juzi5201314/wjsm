// Math.max / Math.min：普通值、空参数、NaN 与有符号零语义。
console.log(Math.max(1, 2, 3));
console.log(Math.min(1, 2, 3));
console.log(Math.max(-1, -5, 0));
console.log(Math.min(-1, -5, 0));
console.log(Math.max());
console.log(Math.min());
console.log(Math.max(1, NaN, 3));
console.log(Math.min(1, NaN, 3));
console.log(Math.max(NaN));
console.log(Object.is(Math.max(-0, +0), +0));
console.log(Object.is(Math.min(-0, +0), -0));