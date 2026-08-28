// Iterator Helpers（ES2025 §27.1）：%Iterator% 抽象构造器 / %Iterator.prototype%
// 的 11 个原型方法（map/filter/take/drop/flatMap/reduce/toArray/forEach/some/
// every/find）与 Iterator.from；helper 惰性求值、底层迭代器关闭（return 传播）、
// %IteratorHelperPrototype% / %WrapForValidIteratorPrototype% 形态与错误文案
// 全部对拍 Node v22。

// ── intrinsic 形态（§27.1.3–27.1.4）──
console.log("ctor: " + typeof Iterator + " " + Iterator.name + " " + Iterator.length);
console.log("ctor own: " + Object.getOwnPropertyNames(Iterator).join(","));
console.log("proto own: " + Object.getOwnPropertyNames(Iterator.prototype).join(","));
console.log("proto syms: " + Object.getOwnPropertySymbols(Iterator.prototype).map(String).join(","));
console.log("chains: " + (Object.getPrototypeOf(Iterator) === Function.prototype) + " " + (Object.getPrototypeOf(Iterator.prototype) === Object.prototype));
const mapDesc = Object.getOwnPropertyDescriptor(Iterator.prototype, "map");
console.log("map desc: " + mapDesc.writable + " " + mapDesc.enumerable + " " + mapDesc.configurable + " " + mapDesc.value.name + " " + mapDesc.value.length);
const protoDesc = Object.getOwnPropertyDescriptor(Iterator, "prototype");
console.log("prototype desc: " + protoDesc.writable + " " + protoDesc.enumerable + " " + protoDesc.configurable);
const ctorDesc = Object.getOwnPropertyDescriptor(Iterator.prototype, "constructor");
console.log("ctor accessor: " + typeof ctorDesc.get + " " + typeof ctorDesc.set + " " + ctorDesc.enumerable + " " + ctorDesc.configurable + " " + ctorDesc.get.name + " " + ctorDesc.set.name + " " + (ctorDesc.get() === Iterator));
const tagDesc = Object.getOwnPropertyDescriptor(Iterator.prototype, Symbol.toStringTag);
console.log("tag accessor: " + typeof tagDesc.get + " " + typeof tagDesc.set + " " + tagDesc.configurable + " " + tagDesc.get());
console.log("@@iterator: " + Iterator.prototype[Symbol.iterator].name + " " + Iterator.prototype[Symbol.iterator].length);
const selfIt = [].values();
console.log("@@iterator self: " + (Iterator.prototype[Symbol.iterator].call(selfIt) === selfIt));
console.log("from: " + Iterator.from.name + " " + Iterator.from.length);
console.log("method identity: " + ([].values().map === Iterator.prototype.map) + " " + ([].values().take === Iterator.prototype.take));

// helper 原型形态（§27.1.2.1）
const helperProto = Object.getPrototypeOf([1].values().map((x) => x));
console.log("helper proto: " + Object.getOwnPropertyNames(helperProto).join(",") + " | " + Object.getOwnPropertySymbols(helperProto).map(String).join(","));
console.log("helper chain: " + (Object.getPrototypeOf(helperProto) === Iterator.prototype) + " " + helperProto[Symbol.toStringTag]);
console.log("helper toString: " + Object.prototype.toString.call([1].values().map((x) => x)));
const helperTagDesc = Object.getOwnPropertyDescriptor(helperProto, Symbol.toStringTag);
console.log("helper tag desc: " + helperTagDesc.value + " " + helperTagDesc.writable + " " + helperTagDesc.enumerable + " " + helperTagDesc.configurable);
const helperNextDesc = Object.getOwnPropertyDescriptor(helperProto, "next");
console.log("helper next desc: " + helperNextDesc.writable + " " + helperNextDesc.configurable + " " + helperNextDesc.value.name + " " + helperNextDesc.value.length);

// wrapper 原型形态（§27.1.3.2.2）
const wrapper = Iterator.from({ next() { return { done: true }; } });
const wrapProto = Object.getPrototypeOf(wrapper);
console.log("wrap proto: " + Object.getOwnPropertyNames(wrapProto).join(",") + " " + Object.getOwnPropertySymbols(wrapProto).length + " " + (Object.getPrototypeOf(wrapProto) === Iterator.prototype));

// ── 基本行为 ──
console.log("map: " + [1, 2, 3].values().map((x) => x * 2).toArray().join(","));
console.log("filter: " + [1, 2, 3, 4].values().filter((x) => x % 2 === 0).toArray().join(","));
console.log("take: " + [1, 2, 3, 4].values().take(2).toArray().join(","));
console.log("drop: " + [1, 2, 3, 4].values().drop(2).toArray().join(","));
console.log("flatMap: " + [1, 2].values().flatMap((x) => [x, x * 10]).toArray().join(","));
console.log("reduce: " + [1, 2, 3].values().reduce((a, b) => a + b) + " " + [1, 2, 3].values().reduce((a, b) => a + b, 10));
const toArrayResult = [1, 2].values().toArray();
console.log("toArray: " + Array.isArray(toArrayResult) + " " + toArrayResult.join(","));
const forEachSeen = [];
[10, 20].values().forEach((v, i) => forEachSeen.push(v + "@" + i));
console.log("forEach: " + forEachSeen.join(",") + " " + ([1].values().forEach(() => {}) === undefined));
console.log("some: " + [1, 2, 3].values().some((x) => x > 2) + " " + [1, 2].values().some((x) => x > 5));
console.log("every: " + [1, 2].values().every((x) => x > 0) + " " + [1, 2].values().every((x) => x > 1));
console.log("find: " + [1, 2, 3].values().find((x) => x > 1) + " " + [1, 2].values().find((x) => x > 5));
console.log("chain: " + [1, 2, 3, 4, 5, 6].values().filter((x) => x % 2 === 0).map((x) => x * 10).take(2).toArray().join(","));
console.log("chain2: " + [1, 2, 3, 4, 5].values().drop(1).take(3).map((x) => x * x).toArray().join(","));

// take / drop 的 ToNumber / ToIntegerOrInfinity（§27.1.4.11 / §27.1.4.5）
console.log("take limits: " + [1, 2].values().take(Infinity).toArray().join(",") + " | " + [1, 2, 3].values().take(1.9).toArray().join(",") + " | " + [1, 2, 3].values().take("2").toArray().join(","));
console.log("drop limits: " + [1, 2].values().drop(-0).toArray().join(",") + " | " + [1, 2, 3, 4].values().drop(1.5).toArray().join(",") + " | " + [1, 2].values().drop(Infinity).toArray().length);

// counter 语义：map / filter 各自独立计数
const argsSeen = [];
[10, 11, 12].values().map((v, i) => { argsSeen.push(v + "@" + i); return v; }).filter((v, i) => { argsSeen.push("f" + v + "@" + i); return true; }).toArray();
console.log("counters: " + argsSeen.join("|"));
const reduceSeen = [];
const reduceTotal = [1, 2, 3].values().reduce((acc, v, i) => { reduceSeen.push(acc + ":" + v + ":" + i); return acc + v; }, 100);
console.log("reduce args: " + reduceTotal + " " + reduceSeen.join("|"));

// ── 惰性与关闭（§27.1.2 / §7.4.11）──
function makeIter(name) {
  let i = 0;
  return {
    next() { i++; console.log(name + " next " + i); return { value: i, done: i > 5 }; },
    return(v) { console.log(name + " return called"); return { value: v, done: true }; },
  };
}
const lazyHelper = Iterator.from(makeIter("A")).map((x) => x * 2);
console.log("created, no next yet");
const lazyFirst = lazyHelper.next();
console.log("A first: " + lazyFirst.value + " " + lazyFirst.done);
console.log("B take: " + JSON.stringify(Iterator.from(makeIter("B")).take(2).toArray()));
console.log("C find: " + Iterator.from(makeIter("C")).find((x) => x >= 2));
const returnHelper = Iterator.from(makeIter("D")).map((x) => x);
returnHelper.next();
console.log("D return: " + JSON.stringify(returnHelper.return(99)));
console.log("D next after return: " + JSON.stringify(returnHelper.next()));
const unstartedHelper = Iterator.from(makeIter("E")).map((x) => x);
console.log("E return: " + JSON.stringify(unstartedHelper.return()));
const throwingHelper = Iterator.from(makeIter("F")).map((x) => { throw new Error("boom"); });
try { throwingHelper.next(); } catch (e) { console.log("F caught: " + e.message); }
console.log("F next after throw: " + JSON.stringify(throwingHelper.next()));
let take0Closed = false;
const take0 = Iterator.from({ next() { return { value: 1, done: false }; }, return() { take0Closed = true; return { done: true }; } }).take(0);
console.log("take(0): " + JSON.stringify(take0.next()) + " " + take0Closed);
let doneCalls = 0;
const doneHelper = Iterator.from({ next() { doneCalls++; return { done: true }; } }).map((x) => x);
doneHelper.next();
doneHelper.next();
console.log("done next calls: " + doneCalls);
let lazySteps = 0;
const lazyChain = Iterator.from({ next() { lazySteps++; return { value: lazySteps, done: lazySteps > 4 }; } }).map((x) => x).filter((x) => x % 2 === 0);
console.log("steps before: " + lazySteps);
console.log("lazy step: " + lazyChain.next().value + " steps " + lazySteps);

// ── 家族覆盖：生成器 / 字符串 / Map / Set / for-of / 展开 ──
function* gen() { yield 1; yield 2; yield 3; }
console.log("gen map: " + gen().map((x) => x * 3).toArray().join(","));
const genInstance = gen();
console.log("from(gen): " + (Iterator.from(genInstance) === genInstance));
console.log("string iter: " + "abc"[Symbol.iterator]().map((c) => c.toUpperCase()).toArray().join(","));
const mapObj = new Map([["a", 1], ["b", 2]]);
console.log("map entries: " + mapObj.entries().map(([k, v]) => k + v).toArray().join(",") + " " + mapObj.keys().toArray().join(","));
console.log("set filter: " + new Set([1, 2, 3]).values().filter((x) => x > 1).toArray().join(","));
const forOfSeen = [];
for (const x of [1, 2, 3].values().map((v) => v * 2)) forOfSeen.push(x);
console.log("for-of: " + forOfSeen.join(","));
console.log("spread: " + [...[5, 6].values().map((v) => v + 1)].join(","));
function* innerGen(x) { yield x; yield x * 100; }
console.log("flatMap gen: " + [1, 2].values().flatMap((x) => innerGen(x)).toArray().join(","));
console.log("flatMap iter: " + [[1, 2], [3]].values().flatMap((x) => x.values()).toArray().join(","));
console.log("flatMap string wrap: " + [..."ab"[Symbol.iterator]().flatMap((c) => [c, c.toUpperCase()])].join(","));
function* genFinally() { try { yield 1; yield 2; } finally { console.log("gen finally"); } }
const genHelper = genFinally().map((x) => x);
genHelper.next();
console.log("gen return: " + JSON.stringify(genHelper.return()));

// ── Iterator.from（§27.1.3.2.1）──
const arrayIt = [4, 5].values();
console.log("from(arrayIt): " + (Iterator.from(arrayIt) === arrayIt));
console.log("from(array): " + Iterator.from([7, 8]).toArray().join(","));
console.log("from(string): " + [...Iterator.from("ab")].join(","));
console.log("wrap helpers: " + Iterator.from({ next() { return { value: 7, done: false }; } }).take(2).toArray().join(","));
const rawWrap = Iterator.from({ next() { return 42; } });
console.log("wrap raw next: " + rawWrap.next());
console.log("wrap no return: " + JSON.stringify(rawWrap.return(7)));
const fwdWrap = Iterator.from({ next() { return { done: false, value: 1 }; }, return(v) { return { ok: v }; } });
console.log("wrap fwd return: " + JSON.stringify(fwdWrap.return(9)) + " " + fwdWrap.return.length + " " + fwdWrap.next.length);

// ── SetterThatIgnoresPrototypeProperties 与子类化 ──
const subObject = Object.create(Iterator.prototype);
subObject.constructor = 9;
console.log("sub ctor: " + subObject.constructor + " " + JSON.stringify(Object.getOwnPropertyDescriptor(subObject, "constructor")));
subObject[Symbol.toStringTag] = "custom";
console.log("sub tag: " + subObject[Symbol.toStringTag]);
class MyIter extends Iterator {
  next() { return { done: true, value: undefined }; }
}
const myIter = new MyIter();
console.log("subclass: " + (myIter instanceof Iterator) + " " + (myIter instanceof MyIter) + " " + typeof myIter.map + " " + (Iterator.from(myIter) === myIter) + " " + JSON.stringify(myIter.toArray()));

// ── 错误路径（文案对齐 V8）──
const errorProbes = [
  ["map noncallable", () => [1].values().map(1)],
  ["flatMap noncallable", () => [1].values().flatMap(null)],
  ["reduce noncallable", () => [1].values().reduce(5)],
  ["find noncallable", () => [1].values().find(5)],
  ["toArray this=str", () => Iterator.prototype.toArray.call("s")],
  ["take this=null", () => Iterator.prototype.take.call(null, 1)],
  ["map this=1", () => Iterator.prototype.map.call(1, (x) => x)],
  ["take()", () => [1].values().take()],
  ["take(-1)", () => [1].values().take(-1)],
  ["take(NaN)", () => [1].values().take(NaN)],
  ["reduce empty", () => [].values().reduce((a, b) => a + b)],
  ["new Iterator", () => new Iterator()],
  ["Iterator()", () => Iterator()],
  ["from(1)", () => Iterator.from(1)],
  ["from(null)", () => Iterator.from(null)],
  ["from bad @@iterator", () => Iterator.from({ [Symbol.iterator]: () => 5 })],
  ["set proto ctor", () => { Iterator.prototype.constructor = 5; }],
  ["set proto tag", () => { Iterator.prototype[Symbol.toStringTag] = "x"; }],
  ["helper next bad this", () => [1].values().map((x) => x).next.call({})],
  ["helper return bad this", () => [1].values().map((x) => x).return.call({})],
  ["wrap next bad this", () => Iterator.from({ next() { return { done: true }; } }).next.call({})],
  ["flatMap primitive", () => [1].values().flatMap((x) => "str").next()],
  ["bad result", () => Iterator.from({ next() { return 42; } }).map((x) => x).next()],
];
for (const [name, probe] of errorProbes) {
  try { probe(); console.log(name + " | no throw"); } catch (e) { console.log(name + " | " + e.constructor.name + " | " + e.message); }
}
const noncallableNext = Iterator.prototype.map.call({ next: 5 }, (x) => x);
console.log("map on {next:5} created");
try { noncallableNext.next(); } catch (e) { console.log("step noncallable | " + e.constructor.name + " | " + e.message); }
const reentrant = Iterator.from({ next() { return reentrantHelper.next(); } });
const reentrantHelper = reentrant.map((x) => x);
try { reentrantHelper.next(); } catch (e) { console.log("reentrancy | " + e.constructor.name + " | " + e.message); }
try { Iterator.prototype.take.call({ get next() { throw new Error("getter boom"); } }, 1); } catch (e) { console.log("next getter | " + e.message); }
console.log("drop(-0.5) ok: " + [1, 2].values().drop(-0.5).toArray().join(","));

// ── 覆盖 / 删除对实例读取的一致性 ──
const originalMap = Iterator.prototype.map;
Iterator.prototype.map = function () { return "patched:" + this.next().value; };
console.log("patched: " + [9].values().map((x) => x));
delete Iterator.prototype.map;
console.log("deleted: " + typeof [].values().map + " " + typeof Iterator.prototype.map);
Iterator.prototype.map = originalMap;
console.log("restored: " + [3].values().map((x) => x * 2).toArray().join(","));
