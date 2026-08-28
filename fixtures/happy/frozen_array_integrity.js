"use strict";
// 数组完整性级别：freeze/seal/preventExtensions 后的元素写、扩容、length
// 变更与谓词结果须与规范一致。
const frozen = [1, 2, 3];
Object.freeze(frozen);
try {
  frozen[0] = 9;
} catch (error) {
  console.log("elem", error.constructor.name);
}
try {
  frozen.push(4);
} catch (error) {
  console.log("push", error.constructor.name);
}
try {
  frozen.length = 1;
} catch (error) {
  console.log("length", error.constructor.name);
}
console.log("frozen state", frozen.length, frozen[0], frozen[2]);
console.log(
  "frozen flags",
  Object.isFrozen(frozen),
  Object.isSealed(frozen),
  Object.isExtensible(frozen),
);

const sealed = [1, 2];
Object.seal(sealed);
sealed[0] = 10;
console.log("seal write", sealed[0]);
try {
  sealed[2] = 3;
} catch (error) {
  console.log("seal add", error.constructor.name);
}
console.log("seal delete", Reflect.deleteProperty(sealed, "0"), sealed[0]);
console.log(
  "seal flags",
  Object.isSealed(sealed),
  Object.isFrozen(sealed),
  Object.isExtensible(sealed),
);

const capped = [1];
Object.preventExtensions(capped);
capped[0] = 5;
console.log("pe write", capped[0]);
try {
  capped[1] = 2;
} catch (error) {
  console.log("pe add", error.constructor.name);
}
capped.length = 0;
console.log("pe shrink", capped.length);
// TestIntegrityLevel：length 仍可写 → isFrozen 为 false。V8/Node 此处
// 偏离规范返回 true（length 仍可增长却报告 frozen），按规范口径断言。
console.log(
  "pe flags",
  Object.isExtensible(capped),
  Object.isSealed(capped),
  Object.isFrozen(capped),
);

// defineProperty 收紧下标后：不可写元素拒写、不可配置元素阻断 length 收缩。
const defined = [1, 2, 3];
Object.defineProperty(defined, 1, { writable: false });
try {
  defined[1] = 9;
} catch (error) {
  console.log("define nw", error.constructor.name);
}
Object.defineProperty(defined, 1, { configurable: false });
try {
  defined.length = 0;
} catch (error) {
  console.log("define shrink", error.constructor.name);
}
console.log("define state", defined.length, defined[0] === undefined, defined[1]);
