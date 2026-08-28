// Array.prototype.values / keys / entries 与 @@iterator 同为 CreateArrayIterator
// 入口（§23.1.3.5 / §23.1.3.19 / §23.1.3.35 / §23.1.3.40），对 ToObject(this)
// 通用：普通对象按 array-like 读 length 与索引属性，不回落 GetIterator 协议。

// --- 函数身份（§23.1.3.40：values 与 @@iterator 为同一函数；§10.4.4.6：
// arguments 对象的 @@iterator 初值亦为 %Array.prototype.values%）---
console.log([].values === [][Symbol.iterator]);
console.log([].values === [].keys, [].keys === [].entries);
console.log((function () { return arguments[Symbol.iterator]; })() === [].values);

// --- 普通对象 receiver：按 length / 索引迭代 ---
const arrayLike = { length: 2, 0: "a", 1: "b" };
const values = Array.prototype.values.call(arrayLike);
console.log(JSON.stringify(values.next()), JSON.stringify(values.next()), JSON.stringify(values.next()));
console.log(JSON.stringify([...Array.prototype.keys.call(arrayLike)]));
console.log(JSON.stringify(Array.prototype.entries.call(arrayLike).next()));

// 迭代器实例挂 %ArrayIteratorPrototype%，helper 沿链可用。
console.log(Object.getPrototypeOf(Array.prototype.values.call(arrayLike)) === Object.getPrototypeOf([].values()));
console.log(Array.prototype.values.call(arrayLike).map(x => x + "!").toArray().join(","));

// length 经 ToLength 截断；空洞产出 undefined。
console.log(JSON.stringify([...Array.prototype.values.call({ length: 2.7, 0: "x", 1: "y", 2: "z" })]));
console.log(JSON.stringify([...Array.prototype.values.call({ length: 3, 1: "mid" })]));

// length 惰性读取：迭代中增长可见。
const grow = { length: 1, 0: "a" };
const growIt = Array.prototype.values.call(grow);
console.log(JSON.stringify(growIt.next()));
grow.length = 2;
grow[1] = "b";
console.log(JSON.stringify(growIt.next()), JSON.stringify(growIt.next()));

// 无 length 的对象（如 Map 实例）按 undefined → 0 立即完成，不消费其 @@iterator。
console.log(JSON.stringify(Array.prototype.values.call(new Map([["k", 1]])).next()));

// 字符串 receiver 按字符序产出。
console.log(JSON.stringify([...Array.prototype.values.call("ab")]));

// nullish receiver 按 ToObject 失败抛 TypeError。
try {
  Array.prototype.values.call(null);
} catch (error) {
  console.log(error.constructor.name);
}
try {
  Array.prototype.keys.call(undefined);
} catch (error) {
  console.log(error.constructor.name);
}
