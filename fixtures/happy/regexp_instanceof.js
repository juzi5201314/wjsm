// #319: RegExp 值（字面量与 new RegExp(...)）对 instanceof RegExp 必须返回 true。
// TAG_REGEXP 是 NaN-boxed 句柄，其 [[Prototype]] 是 RegExp.prototype 对象，
// OrdinaryHasInstance 须将其原型链起点识别为 RegExp.prototype。
console.log(/x/ instanceof RegExp);            // true
console.log(new RegExp("y") instanceof RegExp); // true
console.log(/a/gi instanceof RegExp);          // true（带 flags 的字面量）
// 非 RegExp 值对 instanceof RegExp 必须返回 false。
console.log(({}) instanceof RegExp);           // false
console.log(([]) instanceof RegExp);           // false
console.log(null instanceof RegExp);           // false
console.log(undefined instanceof RegExp);      // false
console.log(42 instanceof RegExp);             // false
console.log("/x/" instanceof RegExp);          // false
