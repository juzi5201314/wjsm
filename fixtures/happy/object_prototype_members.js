// %Object.prototype% 自有属性表：constructor 随固有对象一次性安装，
// 属性名集合 / 顺序与描述符特性与 Node v22 逐字节一致（§20.1.3、§B.2.2）。
const o = {};
console.log(o.constructor === Object);
console.log(o.constructor === Object);
console.log(o.constructor === Object);
console.log("constructor" in {});

console.log(JSON.stringify(Object.getOwnPropertyNames(Object.prototype)));

const ctorDesc = Object.getOwnPropertyDescriptor(Object.prototype, "constructor");
console.log(ctorDesc.writable, ctorDesc.enumerable, ctorDesc.configurable, ctorDesc.value === Object);
for (const name of ["hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable", "toString", "valueOf", "toLocaleString"]) {
  const desc = Object.getOwnPropertyDescriptor(Object.prototype, name);
  console.log(name, desc.writable, desc.enumerable, desc.configurable, typeof desc.value, desc.value.name, desc.value.length);
}

const protoDesc = Object.getOwnPropertyDescriptor(Object.prototype, "__proto__");
console.log(typeof protoDesc.get, typeof protoDesc.set, protoDesc.enumerable, protoDesc.configurable);
console.log("value" in protoDesc, "writable" in protoDesc);
console.log(protoDesc.get.name, protoDesc.get.length, protoDesc.set.name, protoDesc.set.length);

console.log(({}).__proto__ === Object.prototype);
console.log([].__proto__ === Array.prototype);

console.log(Object.prototype.isPrototypeOf({}));
console.log(Object.prototype.isPrototypeOf([]));
console.log(Array.prototype.isPrototypeOf([]));
console.log(Array.prototype.isPrototypeOf({}));
console.log(Object.prototype.isPrototypeOf.call(null, "not object"));
console.log(({}).isPrototypeOf(Object.prototype));

console.log(({ a: 1 }).toLocaleString());
const custom = { toString() { return "custom-tag"; } };
console.log(custom.toLocaleString());
