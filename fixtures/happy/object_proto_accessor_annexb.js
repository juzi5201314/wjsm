// Annex B `__proto__` 访问器对与 __defineGetter__ 族（§B.2.2.1–B.2.2.5），
// 以及对象字面量 `__proto__:` 三态（§B.3.1）不回归。
const parent = { greet() { return "hi"; } };

// 字面量三态：对象 → 设为原型；null → 空原型；其余值 → 忽略。
const litObj = { __proto__: parent };
console.log(Object.getPrototypeOf(litObj) === parent, litObj.greet());
const litNull = { __proto__: null };
console.log(Object.getPrototypeOf(litNull), litNull.hasOwnProperty === undefined);
const litUndef = { __proto__: undefined };
console.log(Object.getPrototypeOf(litUndef) === Object.prototype);
const num = 42;
const litNum = { __proto__: num };
console.log(Object.getPrototypeOf(litNum) === Object.prototype);

// __proto__ 赋值与访问器：setter 对基元 proto 静默忽略。
const assigned = {};
assigned.__proto__ = parent;
console.log(Object.getPrototypeOf(assigned) === parent, assigned.greet());
assigned.__proto__ = null;
console.log(Object.getPrototypeOf(assigned));
const protoSetter = Object.getOwnPropertyDescriptor(Object.prototype, "__proto__").set;
const target = {};
console.log(protoSetter.call(target, "ignored"), Object.getPrototypeOf(target) === Object.prototype);
try {
  Object.prototype.isPrototypeOf.call(null, {});
} catch (error) {
  console.log(error.name, error.message);
}

// __defineGetter__ / __defineSetter__ / __lookupGetter__ / __lookupSetter__。
const host = {};
host.__defineGetter__("x", function () { return 42; });
console.log(host.x);
let stored = 0;
host.__defineSetter__("y", function (value) { stored = value * 2; });
host.y = 21;
console.log(stored);
const xDesc = Object.getOwnPropertyDescriptor(host, "x");
console.log(typeof xDesc.get, xDesc.set, xDesc.enumerable, xDesc.configurable);
console.log(typeof host.__lookupGetter__("x"), host.__lookupSetter__("x"));
console.log(host.__lookupGetter__("y"), typeof host.__lookupSetter__("y"));
console.log(host.__lookupGetter__("missing"));
const child = Object.create(host);
console.log(typeof child.__lookupGetter__("x"));
console.log(child.__lookupGetter__("toString"));
try {
  ({}).__defineGetter__("bad", 1);
} catch (error) {
  console.log(error.name, error.message);
}
try {
  ({}).__defineSetter__("bad", "nope");
} catch (error) {
  console.log(error.name, error.message);
}
