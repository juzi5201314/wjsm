// 数组字符串键索引（CanonicalNumericIndexString）：规范数字字符串 → 元素；
// 非规范（前导零 / 小数 / 非数字）或命名键 → 命名属性。
const a = [10, 20, 30];
console.log(a["0"], a["1"], a["2"]);
console.log(a["05"], a["x"], a["1.0"]);
console.log(a["length"], a.length);
a["1"] = 99;
console.log(a[1], a["1"]);
// 对象的数字字符串键仍是命名属性（对象非数组，无元素存储）。
const o = {};
o["5"] = 7;
console.log(o["5"], o[5]);
