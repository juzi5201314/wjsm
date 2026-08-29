// String.prototype.isWellFormed / toWellFormed（ES2024 §22.1.3.10 / §22.1.3.33）：
// UTF-16 孤立代理判定与 U+FFFD 替换。
const cases = [
  "abc",
  "",
  "a\uD800", // 末尾孤立 leading surrogate
  "\uDC00b", // 开头孤立 trailing surrogate
  "\uD800\uDC00", // 合法代理对
  "\uDC00\uD800", // 逆序代理：两个都孤立
  "a\uD800\uD800b", // 连续 leading
  "\uD83D\uDE00", // 合法 emoji 代理对
  "x\uDFFF\uD83D\uDE00\uD800", // 混合：孤立 + 合法对 + 孤立
];
for (const s of cases) {
  const w = s.toWellFormed();
  console.log(JSON.stringify(s), s.isWellFormed(), JSON.stringify(w), w.isWellFormed(), w.length === s.length);
}

// 属性形态：name / length。
console.log(String.prototype.isWellFormed.name, String.prototype.isWellFormed.length);
console.log(String.prototype.toWellFormed.name, String.prototype.toWellFormed.length);

// 接收者强转：RequireObjectCoercible + ToString（对象走 toString）。
console.log(String.prototype.isWellFormed.call(123), JSON.stringify(String.prototype.toWellFormed.call(456)));
console.log(String.prototype.isWellFormed.call({ toString() { return "\uD800"; } }));
console.log(JSON.stringify(String.prototype.toWellFormed.call({ toString() { return "a\uD800"; } })));

// nullish 接收者：TypeError。
try { String.prototype.isWellFormed.call(null); } catch (e) { console.log(e.constructor.name, e.message); }
try { String.prototype.toWellFormed.call(undefined); } catch (e) { console.log(e.constructor.name, e.message); }

// 已 well-formed 的输入原样返回。
console.log("hello".toWellFormed(), "hello".isWellFormed());
