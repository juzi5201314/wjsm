// String.raw / 字符串字面量的 UTF-16 孤立代理项保真（§22.1.2.4、§6.1.4）：
// ECMAScript 字符串是任意 UTF-16 码元序列，`\ud800` 类转义产生的孤立代理项
// 不得在 lower（WTF-8 → 常量）或宿主聚合（RuntimeString 拼接）中被替换成
// U+FFFD。输出统一为码元十六进制，与 Node v22 逐字节一致。
const hex = (s) =>
  Array.from({ length: s.length }, (_, i) => s.charCodeAt(i).toString(16)).join(" ");

// 字面量直通：普通串、模板 cooked、插值拼接。
console.log(hex("\ud800"), hex("\udfff"));
console.log(hex(`\ud800`), hex(`${"\udc00"}x`));
console.log(hex("\ud800" + "x" + "\udc00"));

// 显式 array-like 形态：高/低孤立代理项、跨段拼出合法代理对。
console.log(hex(String.raw({ raw: ["\ud800"] })));
console.log(hex(String.raw({ raw: ["\udfff"] })));
console.log(hex(String.raw({ raw: ["\ud800", "\udc00"] }, "-")));

// 混合替换值：字面量段 + 运行时构造的孤立代理项 + ToString 协议。
const high = String.fromCharCode(0xd800);
const low = String.fromCharCode(0xdc00);
console.log(hex(String.raw({ raw: ["a", "b", "c"] }, high, low + "x")));
console.log(hex(String.raw({ raw: [high] })));
console.log(hex(String.raw({ raw: ["L", "R"] }, { toString: () => "\udbff" })));

// 标签模板形态：raw 段是源文本（`\ud800` 保持 6 个 ASCII 码元），
// 替换值按 ToString 逐码元进入结果。
console.log(hex(String.raw`pre${"\ud800"}mid${low}post`));
console.log(String.raw`\ud800`.length, hex(String.raw`\ud800`));

// 自定义 tag 观察 cooked quasi：模板转义产生的孤立代理项逐码元可见。
const cooked = (strings) => strings[0];
console.log(hex(cooked`\ud800x\udc00`));

// 键/访问一致性：对象字面量与类成员的字符串键和成员访问同一转换。
console.log(({ "\ud800": 42 })["\ud800"]);
class Probe {
  "\udc00"() {
    return 7;
  }
}
console.log(new Probe()["\udc00"]());
