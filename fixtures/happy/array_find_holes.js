// find 族按 FindViaPredicate（§23.1.3.12.1）读穿洞：洞索引同样调用回调，
// 元素值经 Get 解析（undefined 或原型链继承值）。
{
  const calls = [];
  const idx = [, 1].findIndex(x => { calls.push(x); return x === undefined; });
  console.log("findIndex:", JSON.stringify(calls), idx);
}
{
  const calls = [];
  const idx = [0, , 2].findLastIndex(x => { calls.push(x); return x === undefined; });
  console.log("findLastIndex:", JSON.stringify(calls), idx);
}
Object.defineProperty(Array.prototype, "1", {
  get() { return 42; },
  configurable: true,
});
{
  const found = [0, , 2].find(x => x === 42);
  const foundLast = [0, , 2].findLast(x => x === 42);
  delete Array.prototype["1"];
  console.log("find-inherited:", found, foundLast);
}
