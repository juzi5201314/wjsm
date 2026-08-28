// arguments 对象的原型链（issue #398(b)）：[[Prototype]] 恒为 %Object.prototype%
// （§10.4.4 CreateMappedArgumentsObject 步骤 3 / CreateUnmappedArgumentsObject 步骤 2），
// 继承属性 constructor 经链解析到 %Object%，Annex B __proto__ 访问器与
// Object.getPrototypeOf 口径一致。

// mapped（sloppy 简单形参列表）：constructor 是链上继承属性而非自有属性。
function mapped(a) {
  return [
    Object.getPrototypeOf(arguments) === Object.prototype,
    arguments.__proto__ === Object.prototype,
    arguments.constructor === Object,
    arguments.constructor.prototype === Object.prototype,
    "constructor" in arguments,
    Object.prototype.hasOwnProperty.call(arguments, "constructor"),
  ].join(",");
}
console.log(mapped(1));

// unmapped（非简单形参列表：rest）：原型链行为与 mapped 一致。
function unmappedRest(...rest) {
  return [
    Object.getPrototypeOf(arguments) === Object.prototype,
    arguments.__proto__ === Object.prototype,
    arguments.constructor === Object,
  ].join(",");
}
console.log(unmappedRest(1, 2));

// unmapped（严格模式）：原型链行为与 mapped 一致。
function strictArgs(a) {
  "use strict";
  return [
    Object.getPrototypeOf(arguments) === Object.prototype,
    arguments.__proto__ === Object.prototype,
    arguments.constructor === Object,
  ].join(",");
}
console.log(strictArgs(1));

// __proto__ setter 可改写原型，constructor 随链变化，getPrototypeOf 同步。
function reproto(a) {
  const custom = { constructor: "custom" };
  arguments.__proto__ = custom;
  return [
    Object.getPrototypeOf(arguments) === custom,
    arguments.constructor,
  ].join(",");
}
console.log(reproto(1));

// 原型置 null 后链上属性消失，__proto__ 访问器（也在链上）同样不可达。
function nullProto(a) {
  Object.setPrototypeOf(arguments, null);
  return [
    Object.getPrototypeOf(arguments) === null,
    arguments.constructor === undefined,
    typeof arguments.__proto__,
  ].join(",");
}
console.log(nullProto(1));
