// Array.prototype.sort：稳定排序、比较次数门槛与异常原子性（issue #385）

// 重复键稳定排序：key 相等时保持原相对顺序
var a = [
  { k: 1, id: "a" },
  { k: 0, id: "b" },
  { k: 1, id: "c" },
  { k: 0, id: "d" },
];
console.log(a.sort((x, y) => x.k - y.k).map((x) => x.id).join(","));

// 1000 个确定性随机元素：比较次数必须为线性对数级（<= 12000），且排序正确
var input = Array.from({ length: 1000 }, (_, i) => (i * 7919) % 10007);
var comparisons = 0;
var desc = input.slice().sort((x, y) => {
  comparisons++;
  return y - x;
});
console.log(desc[0], desc[999], comparisons <= 12000);

// sort：comparator 第 3 次调用抛错 → 异常消息、调用次数与未修改的源数组
var sortSource = [5, 4, 3, 2, 1];
var sortCalls = 0;
try {
  sortSource.sort((x, y) => {
    sortCalls++;
    if (sortCalls === 3) throw new Error("boom");
    return x - y;
  });
} catch (e) {
  console.log(e.message, sortCalls, sortSource.join(","));
}

// toSorted：comparator 首次调用抛错 → 异常消息与未修改的源数组
var copySource = [3, 2, 1];
try {
  copySource.toSorted(() => {
    throw new Error("copy boom");
  });
} catch (e) {
  console.log(e.message, copySource.join(","));
}
