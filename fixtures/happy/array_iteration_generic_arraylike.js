// Array.prototype 迭代方法族（§23.1.3）对 generic array-like 的回归：
// 非数组接收者（{length, 索引} 普通对象、含洞 array-like、盒装字符串）经
// call/apply 不得落 InternalInvariant，行为与 Node 一致。

const arrayLike = { length: 3, 0: "a", 1: "b", 2: "c" };
console.log(JSON.stringify(Array.prototype.map.call(arrayLike, (v, i) => v + i)));
const seen = [];
Array.prototype.forEach.call(arrayLike, (v, i, o) => seen.push(v + i + (o === arrayLike)));
console.log(seen.join("|"));
console.log(JSON.stringify(Array.prototype.filter.call(arrayLike, (v) => v !== "b")));
console.log(
  Array.prototype.every.call(arrayLike, (v) => typeof v === "string"),
  Array.prototype.some.call(arrayLike, (v) => v === "c"),
);
console.log(Array.prototype.reduce.call(arrayLike, (acc, v) => acc + v));
console.log(Array.prototype.reduce.call(arrayLike, (acc, v) => acc + v, ">"));
console.log(Array.prototype.reduceRight.call(arrayLike, (acc, v) => acc + v));
console.log(JSON.stringify(Array.prototype.flatMap.call({ length: 2, 0: [1, 2], 1: 3 }, (v) => v)));

// 洞语义：跳洞方法不访问缺失索引，find 族读穿洞。
const holey = { length: 4, 1: "x", 3: "y" };
const visited = [];
Array.prototype.forEach.call(holey, (v, i) => visited.push(i));
console.log(visited.join(","));
console.log(JSON.stringify(Array.prototype.map.call(holey, (v) => v + "!")));
console.log(
  Array.prototype.find.call(holey, (v) => v === undefined) === undefined,
  Array.prototype.findIndex.call(holey, (v) => v === undefined),
);
console.log(
  Array.prototype.findLast.call(holey, (v) => typeof v === "string"),
  Array.prototype.findLastIndex.call(holey, (v) => typeof v === "string"),
);

// sort 原地 [[Set]] 写回 + 尾部 DeletePropertyOrThrow；toSorted 读穿洞产出真数组。
const sortable = { length: 3, 0: "b", 2: "a" };
const sorted = Array.prototype.sort.call(sortable);
console.log(sorted === sortable, JSON.stringify(sortable), "2" in sortable);
console.log(JSON.stringify(Array.prototype.toSorted.call({ length: 3, 0: 2, 2: 1 }, (x, y) => x - y)));

// 盒装原语与 length 的 ToLength 强转。
console.log(JSON.stringify(Array.prototype.map.call("ab", (c, i, o) => c + i + typeof o)));
console.log(JSON.stringify(Array.prototype.map.call({ length: "2", 0: "a", 1: "b" }, (v) => v)));
console.log(JSON.stringify(Array.prototype.filter.call({ length: 2.9, 0: "a", 1: "b", 2: "c" }, () => true)));
console.log(JSON.stringify(Array.prototype.map.call({ length: -1 }, (v) => v)));

// 错误路径（V8 口径文案）：null 接收者、非 callable 回调 / comparator、
// 空 array-like reduce、超长 length 的 ArrayCreate。
try { Array.prototype.map.call(null, (v) => v); } catch (e) { console.log(e.constructor.name, e.message); }
try { Array.prototype.sort.call(null); } catch (e) { console.log(e.constructor.name, e.message); }
try { Array.prototype.map.call(arrayLike, 5); } catch (e) { console.log(e.constructor.name, e.message); }
try { Array.prototype.sort.call(arrayLike, 5); } catch (e) { console.log(e.constructor.name, e.message); }
try { Array.prototype.reduce.call({ length: 2 }, (a, b) => a + b); } catch (e) { console.log(e.constructor.name, e.message); }
try { Array.prototype.map.call({ length: 4294967296 }, (v) => v); } catch (e) { console.log(e.constructor.name, e.message); }
