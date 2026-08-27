// Elem-guard 混合 shape 回归：两个元素模板键序不同 → 静态模板收集
// 放弃，无守卫，通用路径。sum = 1 + 2 = 3。
const ITEMS = [{ a: 1 }, { a: 2, b: 3 }];
let sum = 0;
for (let i = 0; i < ITEMS.length; i++) {
  sum = sum + ITEMS[i].a;
}
console.log(sum);
