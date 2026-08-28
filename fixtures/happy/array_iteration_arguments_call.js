// Array 迭代方法经 call/apply 作用于 arguments 对象的回归（独立于
// issue #397 的 arguments 语义问题）：sloppy 映射与 strict 非映射形态
// 均不得落 InternalInvariant，输出与 Node 一致。

function mapArgs() {
  return Array.prototype.map.call(arguments, (x) => x * 2);
}
console.log(JSON.stringify(mapArgs(1, 2, 3)));

function forEachArgs() {
  const parts = [];
  Array.prototype.forEach.call(arguments, (v, i, o) => parts.push(v + ":" + i + ":" + o.length));
  return parts.join("|");
}
console.log(forEachArgs("a", "b"));

function filterApply() {
  return Array.prototype.filter.apply(arguments, [(x) => x > 1]);
}
console.log(JSON.stringify(filterApply(1, 2, 3)));

function reduceArgs() {
  return Array.prototype.reduce.call(arguments, (acc, v) => acc + v, 0);
}
console.log(reduceArgs(1, 2, 3, 4));

function strictMap() {
  "use strict";
  return Array.prototype.map.call(arguments, (x, i) => x + i);
}
console.log(JSON.stringify(strictMap(10, 20)));

// 映射 arguments：sort 经 [[Set]] 写回索引属性，形参别名同步可见。
function aliasedSort(a, b) {
  Array.prototype.sort.call(arguments, (x, y) => y - x);
  return a + "," + b;
}
console.log(aliasedSort(1, 9));

function findArgs() {
  return Array.prototype.findLastIndex.call(arguments, (v) => v < 3);
}
console.log(findArgs(1, 2, 3, 2));

function someEveryArgs() {
  return [
    Array.prototype.some.call(arguments, (v) => v > 2),
    Array.prototype.every.call(arguments, (v) => v > 0),
  ].join(" ");
}
console.log(someEveryArgs(1, 2, 3));

function toSortedArgs() {
  return Array.prototype.toSorted.call(arguments);
}
console.log(JSON.stringify(toSortedArgs("c", "a", "b")));
