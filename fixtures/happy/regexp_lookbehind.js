// 正向后顾断言
var re1 = /(?<=a)b/;
console.log(re1.test("ab"));  // true
console.log(re1.test("cb"));  // false

// 反向后顾断言
var re2 = /(?<!a)b/;
console.log(re2.test("ab"));  // false
console.log(re2.test("cb"));  // true

// lookbehind 中的捕获组
var re3 = /(?<=(a))(b)/;
var m = re3.exec("ab");
console.log(m[0]);  // b
console.log(m[1]);  // a
console.log(m[2]);  // b
