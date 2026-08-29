// Set.prototype 七个集合运算方法（ES2025 §24.2.4）：union / intersection /
// difference / symmetricDifference / isSubsetOf / isSupersetOf / isDisjointFrom。
const s = (it) => new Set(it);
const show = (v) => (v instanceof Set ? "Set(" + JSON.stringify([...v]) + ")" : JSON.stringify(v));

// 基本运算与元素序（接收者序在前，参数新增序在后）。
console.log(show(s([1, 2, 3]).union(s([3, 4]))));
console.log(show(s([1, 2, 3]).intersection(s([2, 3, 4]))));
console.log(show(s([1, 2, 3]).difference(s([2]))));
console.log(show(s([1, 2, 3]).symmetricDifference(s([3, 4]))));
console.log(s([1, 2]).isSubsetOf(s([1, 2, 3])), s([1, 2]).isSubsetOf(s([1])));
console.log(s([1, 2, 3]).isSupersetOf(s([1, 2])), s([1]).isSupersetOf(s([1, 2])));
console.log(s([1]).isDisjointFrom(s([2])), s([1, 2]).isDisjointFrom(s([2, 3])));

// 空集与自身。
console.log(show(s([]).union(s([1]))), show(s([1]).intersection(s([]))));
const self = s([1, 2]);
console.log(show(self.union(self)), self.isSubsetOf(self), self.isDisjointFrom(self));

// SameValueZero 归一：-0/+0 合并，NaN 相等；结果集是新对象。
console.log(show(s([0]).union(s([-0]))), show(s([1]).union(s([NaN, NaN]))));
const base = s([1]);
const merged = base.union(s([2]));
console.log(merged !== base, show(base), merged instanceof Set);

// 字符串与对象键（对象按引用）。
const key = {};
const inter = s(["a", key]).intersection(s([key, "b"]));
console.log(inter.size === 1 && inter.has(key), s(["a"]).union(s(["a"])).size);

// Map 也是 set-like（size / has / keys 取键）。
const m = new Map([[1, "a"], [2, "b"]]);
console.log(show(s([1, 3]).union(m)), show(s([1, 3]).intersection(m)), s([1]).isSubsetOf(m));

// 自定义 set-like 协议对象。
const like = {
  size: 2,
  has: (v) => v === 1 || v === 9,
  keys() {
    let i = 0;
    const vals = [1, 9];
    return { next: () => (i < vals.length ? { value: vals[i++], done: false } : { value: undefined, done: true }) };
  },
};
console.log(show(s([1, 2]).union(like)), show(s([1, 2]).intersection(like)));
console.log(show(s([1, 2]).difference(like)), show(s([1, 2]).symmetricDifference(like)));
console.log(s([1]).isSubsetOf(like), s([1, 9]).isSupersetOf(like), s([2]).isDisjointFrom(like));

// size 为 Infinity 的 set-like（isSubsetOf 恒走 has 路径）。
const infinite = { size: Infinity, has: () => true, keys() { return { next() { return { done: true }; } }; } };
console.log(s([1, 2, 3]).isSubsetOf(infinite), s([1]).isDisjointFrom(infinite));

// 用户 has 重入删除接收者元素：以活数据为准。
{
  const receiver = s([1, 2, 3]);
  const trap = {
    size: 3,
    has(v) { receiver.delete(2); return v === 1; },
    keys() { const vals = [7]; let i = 0; return { next: () => (i < vals.length ? { value: vals[i++], done: false } : { done: true }) }; },
  };
  console.log(JSON.stringify([...receiver.difference(trap)]), JSON.stringify([...receiver]));
}

// 方法属性形态：name / length。
for (const n of ["union", "intersection", "difference", "symmetricDifference", "isSubsetOf", "isSupersetOf", "isDisjointFrom"]) {
  const f = Set.prototype[n];
  console.log(n, f.name, f.length, typeof f);
}
