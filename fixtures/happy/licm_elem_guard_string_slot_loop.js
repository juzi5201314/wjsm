// Elem-guard 字符串槽正向用例：元素值槽是字符串（原始值），守卫
// 命中，循环体字符串拼接的操作数均已证明原始。text = "abc"。
const WORDS = [{ s: "a" }, { s: "b" }, { s: "c" }];
let text = "";
for (let i = 0; i < WORDS.length; i++) {
  text = text + WORDS[i].s;
}
console.log(text);
