// \p{Letter} 匹配字母
var re1 = /\p{Letter}+/u;
console.log(re1.test("hello"));  // true
console.log(re1.test("123"));    // false

// \P{Letter} 匹配非字母
var re2 = /\P{Letter}+/u;
console.log(re2.test("123"));    // true

// \p{Script=Latin}
var re3 = /\p{Script=Latin}+/u;
console.log(re3.test("café"));   // true
