// %TypedArray% intrinsic 构造器/原型对（§23.2.1–23.2.3）：11 种具体构造器的
// 静态 [[Prototype]] 是 %TypedArray% 抽象构造器（from / of / @@species 沿
// 静态原型链继承），共享原型自有 constructor 指回抽象构造器并携带
// @@toStringTag 访问器 getter，Object.prototype.toString 得出 [object <kind>]。

const kinds = [
  Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
  Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array,
  BigUint64Array,
];
const TA = Object.getPrototypeOf(Uint8Array);
const shared = Object.getPrototypeOf(Uint8Array.prototype);

// 构造器/原型对身份
console.log("pair: " + (TA.prototype === shared) + " " + (shared.constructor === TA) + " " + shared.hasOwnProperty("constructor"));
console.log("ctor chain: " + (TA === Function.prototype) + " " + (Object.getPrototypeOf(TA) === Function.prototype) + " " + kinds.every((Ctor) => Object.getPrototypeOf(Ctor) === TA));
console.log("proto chain: " + (Object.getPrototypeOf(shared) === Object.prototype) + " " + (shared === Object.prototype) + " " + kinds.every((Ctor) => Object.getPrototypeOf(Ctor.prototype) === shared));

// 抽象构造器形态（§23.2.2）
console.log("shape: " + TA.name + " " + TA.length + " " + String(TA));
console.log("own: " + Object.getOwnPropertyNames(TA).sort().join(","));
const dp = Object.getOwnPropertyDescriptor(TA, "prototype");
console.log("prototype desc: " + (dp.value === shared) + " " + dp.writable + " " + dp.enumerable + " " + dp.configurable);
const df = Object.getOwnPropertyDescriptor(TA, "from");
console.log("from desc: " + df.writable + " " + df.enumerable + " " + df.configurable + " " + df.value.name + " " + df.value.length + " " + String(df.value));
const doff = Object.getOwnPropertyDescriptor(TA, "of");
console.log("of desc: " + doff.writable + " " + doff.enumerable + " " + doff.configurable + " " + doff.value.name + " " + doff.value.length);
const dsp = Object.getOwnPropertyDescriptor(TA, Symbol.species);
console.log("species desc: " + dsp.get.name + " " + dsp.enumerable + " " + dsp.configurable + " " + dsp.set);
console.log("species get: " + (TA[Symbol.species] === TA) + " " + (Uint8Array[Symbol.species] === Uint8Array));

// 共享原型的 constructor 与 @@toStringTag（§23.2.3.4 / §23.2.3.38）
const dc = Object.getOwnPropertyDescriptor(shared, "constructor");
console.log("constructor desc: " + (dc.value === TA) + " " + dc.writable + " " + dc.enumerable + " " + dc.configurable);
const dt = Object.getOwnPropertyDescriptor(shared, Symbol.toStringTag);
console.log("tag desc: " + dt.get.name + " " + dt.get.length + " " + dt.enumerable + " " + dt.configurable + " " + dt.set + " " + String(dt.get));
console.log("tag get: " + dt.get.call(new Uint16Array(0)) + " " + dt.get.call({}) + " " + dt.get.call(null) + " " + dt.get.call(7) + " " + dt.get.call(shared));
console.log("tags: " + kinds.map((Ctor) => Object.prototype.toString.call(new Ctor(0))).join(","));
console.log("proto tags: " + Object.prototype.toString.call(shared) + " " + Object.prototype.toString.call(Uint8Array.prototype));
console.log("instance tag: " + new Uint8Array(0)[Symbol.toStringTag] + " " + shared[Symbol.toStringTag]);

// 抽象构造器不可直接 Call / Construct（§23.2.1）
try {
  TA();
} catch (error) {
  console.log("call: " + error.constructor.name + " " + error.message);
}
try {
  new TA();
} catch (error) {
  console.log("new: " + (Object.getPrototypeOf(error) === TypeError.prototype) + " " + error.message);
}
try {
  Reflect.construct(TA, []);
} catch (error) {
  console.log("reflect: " + error.message);
}
class Abstract extends TA {}
try {
  new Abstract();
} catch (error) {
  console.log("extends: " + error.message);
}

// 静态成员经原型链继承（§23.2.6：具体构造器无自有 from / of / @@species）
console.log("inherit: " + kinds.every((Ctor) => Ctor.from === TA.from && Ctor.of === TA.of && Ctor[Symbol.species] === Ctor));
console.log("own statics: " + Uint8Array.hasOwnProperty("from") + " " + Uint8Array.hasOwnProperty("of") + " " + Object.getOwnPropertySymbols(Uint8Array).length);
class Sub extends Uint8Array {}
console.log("sub statics: " + (Sub.from === TA.from) + " " + (Sub.of === TA.of) + " " + (Sub[Symbol.species] === Sub));

// from / of 行为（§23.2.2.1–23.2.2.2）
console.log("from array: " + Uint8Array.from([1, 2, 3]).join(",") + " " + Uint8Array.from("123").join(",") + " " + Uint8Array.from(new Set([1, 2])).join(","));
console.log("from arraylike: " + Uint8Array.from({ length: 3, 0: 7, 1: 8 }).join(",") + " " + Uint8Array.from({ length: 2.7, 0: 1, 1: 2, 2: 3 }).join(",") + " " + Uint8Array.from({ length: -1 }).length);
console.log("from map: " + Uint8Array.from([1, 2, 3], (v, k) => v * 10 + k).join(",") + " " + Uint8Array.from([1, 2], function () { return this.x; }, { x: 5 }).join(","));
console.log("from convert: " + Uint8Array.from([300, -1, 256]).join(",") + " " + Int8Array.of(200).join(",") + " " + Uint8Array.of(1, "2", 3.7).join(","));
console.log("empty: " + Uint8Array.from("").length + " " + Uint8Array.of().length);
console.log("bigint: " + BigInt64Array.from([1n, 2n]).join(",") + " " + (BigUint64Array.of(3n)[0] === 3n));
const iterObserved = [];
Uint8Array.from({ get [Symbol.iterator]() { iterObserved.push("get-iter"); return undefined; }, length: 1, 0: 6 });
console.log("iter observed: " + JSON.stringify(iterObserved));

// this 决定构造目标：显式 receiver / bind / 子类
const cross = TA.from.call(Int32Array, [5, 6]);
console.log("cross: " + (cross instanceof Int32Array) + " " + cross.join(","));
const bound = Uint8Array.from.bind(Int16Array)([9]);
console.log("bound: " + (bound instanceof Int16Array) + " " + bound.join(","));
const sf = Sub.from([1, 2]);
const so = Sub.of(9);
console.log("sub create: " + (sf instanceof Sub) + " " + sf.join(",") + " " + (so instanceof Sub) + " " + so.join(","));
const order = [];
class Obs extends Uint8Array {
  constructor(...a) {
    order.push("ctor:" + a.join("|"));
    super(...a);
  }
}
Obs.from([3, 4]);
Obs.of(5, 6);
console.log("ctor args: " + JSON.stringify(order));

// 错误路径（文案对齐 V8）
try {
  TA.from.call(undefined, []);
} catch (error) {
  console.log("this undefined: " + error.constructor.name + " " + error.message);
}
try {
  TA.from.call({}, []);
} catch (error) {
  console.log("this object: " + error.message);
}
try {
  TA.of.call(5, 1);
} catch (error) {
  console.log("this number: " + error.message);
}
try {
  Uint8Array.from([1], 5);
} catch (error) {
  console.log("map number: " + error.message);
}
try {
  Uint8Array.from([1], "x");
} catch (error) {
  console.log("map string: " + error.message);
}
try {
  Uint8Array.from([1], null);
} catch (error) {
  console.log("map null: " + error.message);
}
try {
  Uint8Array.from([1], {});
} catch (error) {
  console.log("map object: " + error.message);
}
try {
  Uint8Array.from(null);
} catch (error) {
  console.log("from null: " + error.message);
}
try {
  Uint8Array.from(undefined);
} catch (error) {
  console.log("from undefined: " + error.message);
}
try {
  Uint8Array.from({ [Symbol.iterator]: 5 });
} catch (error) {
  console.log("bad iterator: " + error.message);
}
try {
  Uint8Array.of.call(function () { return {}; }, 1);
} catch (error) {
  console.log("non-ta result: " + error.message);
}
try {
  Uint8Array.of.call(function () { return new Uint8Array(0); }, 1);
} catch (error) {
  console.log("short result: " + error.message);
}
try {
  Uint8Array.from.call(class extends Uint8Array { constructor() { super(0); } }, [1, 2]);
} catch (error) {
  console.log("short sub: " + error.message);
}
try {
  TA.from.call(Uint8Array, { [Symbol.iterator]() { return { next() { throw new RangeError("boom-iter"); } }; } });
} catch (error) {
  console.log("iter throw: " + error.constructor.name + " " + error.message);
}
try {
  Uint8Array.from({ get length() { throw new SyntaxError("boom-len"); } });
} catch (error) {
  console.log("len throw: " + error.constructor.name + " " + error.message);
}

// from / of 本身不是构造器（无 [[Construct]]）
try {
  Reflect.construct(TA.from, [[1]], Uint8Array);
} catch (error) {
  console.log("from construct: " + error.constructor.name);
}
console.log("method proto: " + (Object.getPrototypeOf(TA.from) === Function.prototype) + " " + (Object.getPrototypeOf(dt.get) === Function.prototype));
