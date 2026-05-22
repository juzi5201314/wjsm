var re = /(?<first>\w+)\s+(?<last>\w+)/;
var s = "John Doe";
var r = s.replace(re, "$<last>, $<first>");
console.log(r);
// 测试不存在的命名组 → 空字符串
var r2 = s.replace(re, "$<unknown>");
console.log(r2);
