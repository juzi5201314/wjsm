// s 标志下 . 匹配换行
var re = /a.b/s;
console.log(re.test("a\nb"));  // true

// 无 s 标志下 . 不匹配换行
var re2 = /a.b/;
console.log(re2.test("a\nb"));  // false
