// 内建迭代器实例的真实原型链（%ArrayIteratorPrototype% / %StringIteratorPrototype%
// / %MapIteratorPrototype% / %SetIteratorPrototype% / %RegExpStringIteratorPrototype%，
// §23.1.5 / §22.1.5 / §24.1.5 / §24.2.5 / §22.2.9）：实例 [[Prototype]] 接线家族
// 原型对象，helper 方法与 @@iterator 沿链继承 %Iterator.prototype%（§27.1.2）。

// --- %ArrayIteratorPrototype% 形态 ---
const arrayIterProto = Object.getPrototypeOf([].values());
console.log(arrayIterProto === Object.prototype);
console.log(Object.getPrototypeOf(arrayIterProto) === Iterator.prototype);
console.log(Object.getPrototypeOf(Iterator.prototype) === Object.prototype);
console.log(Object.getOwnPropertyNames(arrayIterProto).join(","));
console.log(Object.getOwnPropertySymbols(arrayIterProto).map(String).join(","));
const nextDesc = Object.getOwnPropertyDescriptor(arrayIterProto, "next");
console.log(nextDesc.writable, nextDesc.enumerable, nextDesc.configurable);
const tagDesc = Object.getOwnPropertyDescriptor(arrayIterProto, Symbol.toStringTag);
console.log(tagDesc.value, tagDesc.writable, tagDesc.enumerable, tagDesc.configurable);
console.log(arrayIterProto.next.name, arrayIterProto.next.length);

// 同族实例共享同一原型与同一 next。
console.log(Object.getPrototypeOf([].keys()) === arrayIterProto);
console.log(Object.getPrototypeOf([1, 2].entries()) === arrayIterProto);
console.log([].values().next === [1].keys().next);
console.log(Reflect.ownKeys([].values()).length);

// arguments 与 TypedArray 迭代器同属 Array Iterator 家族（§23.2.3.38 按
// CreateArrayIterator 建实例）。
(function () {
  console.log(Object.getPrototypeOf(arguments[Symbol.iterator]()) === arrayIterProto);
})(1, 2);
console.log(Object.getPrototypeOf(new Uint8Array(2).values()) === arrayIterProto);
console.log(Object.getPrototypeOf(new Float64Array(1).keys()) === arrayIterProto);

// --- 字符串 / Map / Set / RegExp 家族 ---
const stringIterProto = Object.getPrototypeOf(""[Symbol.iterator]());
const mapIterProto = Object.getPrototypeOf(new Map().entries());
const setIterProto = Object.getPrototypeOf(new Set().values());
const regexpIterProto = Object.getPrototypeOf("a".matchAll(/a/g));
console.log(stringIterProto !== arrayIterProto, mapIterProto !== setIterProto);
console.log(Object.getPrototypeOf(stringIterProto) === Iterator.prototype);
console.log(Object.getPrototypeOf(mapIterProto) === Iterator.prototype);
console.log(Object.getPrototypeOf(setIterProto) === Iterator.prototype);
console.log(Object.getPrototypeOf(regexpIterProto) === Iterator.prototype);
console.log(Object.getPrototypeOf(new Map().keys()) === mapIterProto);
console.log(Object.getPrototypeOf(new Map().values()) === mapIterProto);
console.log(Object.getPrototypeOf(new Set().entries()) === setIterProto);
console.log(Object.getOwnPropertyDescriptor(stringIterProto, Symbol.toStringTag).value);
console.log(Object.getOwnPropertyDescriptor(mapIterProto, Symbol.toStringTag).value);
console.log(Object.getOwnPropertyDescriptor(setIterProto, Symbol.toStringTag).value);
console.log(Object.getOwnPropertyDescriptor(regexpIterProto, Symbol.toStringTag).value);

// Object.prototype.toString 经链上 @@toStringTag 定牌（§20.1.3.6）。
console.log(Object.prototype.toString.call([].values()));
console.log(Object.prototype.toString.call("ab"[Symbol.iterator]()));
console.log(Object.prototype.toString.call(new Map().keys()));
console.log(Object.prototype.toString.call(new Set().values()));
console.log(Object.prototype.toString.call("a".matchAll(/a/g)));

// --- Iterator.prototype 继承（helper 方法与 @@iterator 是链上真实属性）---
console.log([].values().map === Iterator.prototype.map);
console.log("ab"[Symbol.iterator]().take === Iterator.prototype.take);
console.log(new Map().keys().filter === Iterator.prototype.filter);
console.log(new Set().values().toArray === Iterator.prototype.toArray);
console.log("a".matchAll(/a/g).drop === Iterator.prototype.drop);
console.log([].values()[Symbol.iterator] === Iterator.prototype[Symbol.iterator]);
const roundtrip = [7].values();
console.log(roundtrip[Symbol.iterator]() === roundtrip);
console.log([].values() instanceof Iterator);
console.log(new Map().entries() instanceof Iterator);

// helper 管道跨家族工作（值经真实链上的方法）。
console.log(new Map([["a", 1], ["b", 2]]).values().map(x => x * 10).toArray().join(","));
console.log(new Set([3, 4]).values().drop(1).toArray().join(","));
console.log("xyz"[Symbol.iterator]().take(2).toArray().join(""));

// 链上可变性：删除 Iterator.prototype.map 后实例读取同步缺失，恢复后可见。
const savedMap = Iterator.prototype.map;
delete Iterator.prototype.map;
console.log([].values().map === undefined);
Iterator.prototype.map = savedMap;
console.log([].values().map === savedMap);

// 覆盖家族原型 next：实例经链读取覆盖值。
const savedNext = arrayIterProto.next;
arrayIterProto.next = function () { return { value: "patched", done: false }; };
const patched = [1].values();
console.log(patched.next().value);
arrayIterProto.next = savedNext;
console.log(JSON.stringify(patched.next()));

// 断链：Object.setPrototypeOf(it, null) 后 next 缺失。
const orphan = [1].values();
Object.setPrototypeOf(orphan, null);
console.log(orphan.next === undefined);

// --- 共享 next 的 brand 检查（V8 口径 incompatible receiver 文案）---
function fails(fn) {
  try {
    fn();
    return "no throw";
  } catch (error) {
    return error.constructor.name + ": " + error.message;
  }
}
console.log(fails(() => arrayIterProto.next.call({})));
console.log(fails(() => arrayIterProto.next.call([])));
console.log(fails(() => arrayIterProto.next.call(null)));
console.log(fails(() => arrayIterProto.next.call(undefined)));
console.log(fails(() => arrayIterProto.next.call(1)));
console.log(fails(() => arrayIterProto.next.call("s")));
console.log(fails(() => arrayIterProto.next.call(new Map().entries())));
console.log(fails(() => stringIterProto.next.call([].values())));
console.log(fails(() => mapIterProto.next.call(new Set().values())));
console.log(fails(() => setIterProto.next.call(new Map().keys())));
console.log(fails(() => regexpIterProto.next.call({})));
console.log(fails(() => mapIterProto.next.call([].values().map(x => x))));
// 家族内跨实例调用合法（brand 按内部槽而非对象身份）。
console.log(JSON.stringify(arrayIterProto.next.call(new Uint8Array([5]).values())));
console.log(JSON.stringify(setIterProto.next.call(new Set([6]).values())));

// --- Map/Set 迭代形态（键值集合不做 index 包装）---
const map = new Map([["a", 1], ["b", 2]]);
for (const entry of map) console.log(JSON.stringify(entry));
for (const [key, stored] of map) console.log(key, stored);
console.log(JSON.stringify(map.keys().next()));
console.log(JSON.stringify(map.values().next()));
console.log(JSON.stringify(map.entries().next()));
console.log(JSON.stringify([...map.keys()]));
const set = new Set([7, 8]);
console.log(JSON.stringify([...set.entries()]));
console.log(JSON.stringify(set.keys().next()));
console.log(JSON.stringify([...new Uint8Array([9, 10]).entries()]));

// --- 提前退出后实例位置与 Node 一致（next() 已推进过被消费元素）---
const brokenLoop = [1, 2, 3].values();
for (const item of brokenLoop) {
  break;
}
console.log(JSON.stringify(brokenLoop.next()));
const [first] = [10, 20, 30].values();
console.log(first);
const destructured = [10, 20, 30].values();
const [head] = destructured;
console.log(head, JSON.stringify(destructured.next()));
const mapBreak = new Map([["k1", 1], ["k2", 2]]).entries();
for (const entry of mapBreak) {
  break;
}
console.log(JSON.stringify(mapBreak.next()));

// 耗尽后 [[Done]] 粘住：底层数组再增长也不复活（§23.1.5.2.1 步骤 8.a）。
const source = [1];
const sticky = source.values();
console.log(JSON.stringify(sticky.next()), JSON.stringify(sticky.next()));
source.push(2);
console.log(JSON.stringify(sticky.next()));

// matchAll 迭代器行为不变。
console.log([..."aba".matchAll(/a/g)].map(match => match.index).join(","));
