// ECMAScript §6.1.5.1 — 曾缺失的 6 个 well-known Symbol 静态属性
const names = [
  "isConcatSpreadable",
  "replace",
  "search",
  "split",
  "matchAll",
  "unscopables",
];
for (const n of names) {
  const s = Symbol[n];
  console.log(n, typeof s, s === Symbol[n]);
}
// 点访问与 issue 中曾缺失项
console.log(typeof Symbol.isConcatSpreadable, typeof Symbol.iterator);