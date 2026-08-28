"use strict";
// callable 完整性级别：freeze/seal/preventExtensions 后的属性写、新增、
// defineProperty 与谓词结果须与规范一致。
function target(a, b) {}
target.custom = 1;
Object.freeze(target);
try {
  target.custom = 2;
} catch (error) {
  console.log("custom", error.constructor.name);
}
try {
  target.added = 1;
} catch (error) {
  console.log("added", error.constructor.name);
}
try {
  target.prototype = {};
} catch (error) {
  console.log("prototype", error.constructor.name);
}
try {
  Object.defineProperty(target, "custom", { value: 9 });
} catch (error) {
  console.log("define", error.constructor.name);
}
console.log("frozen state", target.custom, target.length, target.name);
console.log(
  "frozen flags",
  Object.isFrozen(target),
  Object.isSealed(target),
  Object.isExtensible(target),
);

const capped = function named() {};
Object.preventExtensions(capped);
try {
  capped.added = 1;
} catch (error) {
  console.log("pe added", error.constructor.name);
}
console.log(
  "pe flags",
  Object.isExtensible(capped),
  Object.isFrozen(capped),
  Object.isSealed(capped),
);

// seal 后既有属性仍可写，删除被拒。
const sealed = function inner() {};
sealed.data = 1;
Object.seal(sealed);
sealed.data = 2;
console.log("seal write", sealed.data);
console.log("seal delete", Reflect.deleteProperty(sealed, "data"), sealed.data);
console.log("seal flags", Object.isSealed(sealed), Object.isExtensible(sealed));
