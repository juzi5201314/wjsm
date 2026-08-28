// 字典 kind（索引位置有 accessor）真数组：迭代逐索引走完整
// HasProperty/[[Get]]，元素槽陈旧值不可观察；迭代/排序中升 kind 立即生效。
{
  const a = [1, 2];
  Object.defineProperty(a, "0", { get() { return 5; } });
  console.log("map:", JSON.stringify(a.map(x => x)));
}
{
  const holes = [, 2];
  Object.defineProperty(holes, "0", { get() { return 5; } });
  console.log("hole-accessor:", JSON.stringify(holes.map(x => x)), holes.length);
}
{
  const b = [1, 2, 3];
  const out = [];
  b.forEach((x, i) => {
    out.push(i + ":" + x);
    if (i === 0) Object.defineProperty(b, "2", { get() { return 99; } });
  });
  console.log("mid-raise:", out.join(","));
}
{
  // 比较器中升 kind：sort 写回退回规范 Set，经 setter 而非直写元素槽。
  const c = [3, 1];
  const sets = [];
  let defined = false;
  c.sort((x, y) => {
    if (!defined) {
      defined = true;
      Object.defineProperty(c, "0", {
        get() { return 0; },
        set(v) { sets.push(v); },
        configurable: true,
      });
    }
    return x - y;
  });
  console.log("sort-setter:", JSON.stringify(sets), c[0], c[1]);
}
{
  const d = [, 3];
  Object.defineProperty(d, "0", { get() { return 9; } });
  console.log("toSorted:", JSON.stringify(d.toSorted()));
}
