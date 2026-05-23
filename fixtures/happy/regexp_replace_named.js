var re = /(?<first>\w+)\s+(?<last>\w+)/;
var s = "John Doe";
var r = s.replace(re, "$<last>, $<first>");
console.log(r);
// 测试不存在的命名组 → 空字符串
var r2 = s.replace(re, "$<unknown>");
console.log(r2);
// 函数替换接收 groups 参数
var re2 = /(?<a>\d+)\+(?<b>\d+)/;
var r3 = "3+5".replace(re2, function(match, p1, p2, offset, str, groups) {
    return (Number(groups.a) + Number(groups.b)).toString();
});
console.log(r3);
