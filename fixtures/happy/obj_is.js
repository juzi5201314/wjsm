console.log(Object.is(1, 1));
console.log(Object.is(1, 2));
console.log(Object.is(NaN, NaN));
console.log(Object.is(0, -0));
// 验证 NaN-boxed 值不会错误地匹配为 NaN
console.log(Object.is(null, undefined));
console.log(Object.is(true, false));
console.log(Object.is(true, true));
