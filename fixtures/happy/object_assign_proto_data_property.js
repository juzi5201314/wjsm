// Assignment should create an own data property even when the prototype has that key.
const proto = { value: "proto" };
const child = {};

Object.setPrototypeOf(child, proto);
child.value = "child";

console.log("child value:", child.value);
console.log("proto value:", proto.value);
console.log("child owns value:", Object.hasOwn(child, "value"));
console.log("proto owns value:", Object.hasOwn(proto, "value"));
