// 真数组迭代方法按 HasProperty/Get 观察 Array.prototype 上的继承索引：
// 洞与越界索引不再直接归约为「缺失/undefined」，而是沿原型链解析。
Array.prototype[0] = 7;
{
  let calls = 0;
  const r = [,].map(x => { calls++; return x * 2; });
  console.log("map:", JSON.stringify(r), calls);
}
{
  // 回调经属性读取（非静态闭包）：走宿主 builtin 路径而非内联展开。
  const fns = { double(x) { return x * 2; } };
  console.log("map-slow:", JSON.stringify([,].map(fns.double)));
}
{
  const out = [];
  [, 2].forEach((x, i) => out.push(i + ":" + x));
  console.log("forEach:", out.join(","));
}
console.log("filter:", JSON.stringify([, 2].filter(x => x > 2)));
console.log("some:", [, 2].some(x => x === 7), "every:", [, 2].every(x => x >= 2));
console.log("reduce:", [, 2].reduce((a, b) => a + b));
console.log("reduceRight:", [, 2].reduceRight((a, b) => a + "|" + b));
delete Array.prototype[0];

// getter 继承索引：以接收者为 this 调用。
Object.defineProperty(Array.prototype, "0", {
  get() { return this.length * 10; },
  configurable: true,
});
console.log("getter-map:", JSON.stringify([, 5].map(x => x)));
delete Array.prototype["0"];

// 迭代中的原型变更立即可观察（HasProperty 逐索引重查）。
Array.prototype[1] = 8;
{
  const a = [1, , 3];
  const out = [];
  a.forEach((x, i) => { out.push(i + ":" + x); if (i === 0) delete Array.prototype[1]; });
  console.log("proto-delete:", out.join(","));
}
{
  const a = [0, , 2];
  const out = [];
  a.forEach((x, i) => { out.push(i + ":" + x); if (i === 0) Array.prototype[1] = 9; });
  console.log("proto-add:", out.join(","));
  delete Array.prototype[1];
}
