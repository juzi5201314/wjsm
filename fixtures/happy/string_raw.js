// String.raw（§22.1.2.4）：标签模板与显式 array-like 调用形态，按
// literals.raw 的 LengthOfArrayLike 交替拼接原始段与替换值；ToObject
// 边界抛 TypeError，输出与 Node v22 逐字节一致。
console.log(String.raw`a\nb${1 + 1}c\t`);
console.log(String.raw`\u00e9 ${"x"} \x41`);
console.log(String.raw``);
console.log(String.raw`${1}${2}${3}`);
console.log(String.raw({ raw: ["a", "b", "c"] }, 1, 2, 3));
console.log(String.raw({ raw: "xyz" }, "-", "+"));
console.log(String.raw({ raw: { length: 2, 0: "L", 1: "R" } }, "mid"));
console.log(String.raw({ raw: [] }));
console.log(String.raw({ raw: { length: -5 } }));
console.log(String.raw({ raw: { length: NaN } }));
console.log(JSON.stringify(String.raw({ raw: [undefined, null] }, "s")));
try { String.raw(); } catch (e) { console.log(e.name, e.message); }
try { String.raw({}); } catch (e) { console.log(e.name, e.message); }
try { String.raw({ raw: null }); } catch (e) { console.log(e.name, e.message); }
console.log(String.raw.name, String.raw.length, typeof String.raw);
const raw = String.raw;
console.log(raw`p${0}q`);
console.log(String.raw.call(undefined, { raw: ["r", "s"] }, "|"));
console.log(String.raw`嵌套${String.raw`inner\n${"!"}`}外`);
