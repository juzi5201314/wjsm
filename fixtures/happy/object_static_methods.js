// Test all new Object.* static methods on plain objects
console.log("=== Object.keys ===");
const obj1 = { a: 1, b: 2, c: 3 };
console.log("keys:", JSON.stringify(Object.keys(obj1)));

console.log("\n=== Object.values ===");
console.log("values:", JSON.stringify(Object.values(obj1)));

console.log("\n=== Object.entries ===");
console.log("entries:", JSON.stringify(Object.entries(obj1)));

console.log("\n=== Object.assign ===");
const target = { a: 1, b: 2 };
const source1 = { b: 4, c: 5 };
const source2 = { c: 6, d: 7 };
const result = Object.assign(target, source1, source2);
console.log("result:", JSON.stringify(result));
console.log("same ref:", result === target);

console.log("\n=== Object.create ===");
const proto = { greet() { return "hello"; } };
const obj2 = Object.create(proto);
console.log("proto method:", obj2.greet());
console.log("has own:", Object.getOwnPropertyNames(obj2).length);

const nullProto = Object.create(null);
console.log("null proto keys:", Object.keys(nullProto).length);

console.log("\n=== Object.is ===");
console.log("is(NaN, NaN):", Object.is(NaN, NaN));
console.log("is(0, -0):", Object.is(0, -0));
console.log("is(0, 0):", Object.is(0, 0));
console.log("is(1, 1):", Object.is(1, 1));
console.log("is('a', 'a'):", Object.is("a", "a"));
console.log("is(null, null):", Object.is(null, null));
console.log("is(undefined, undefined):", Object.is(undefined, undefined));
console.log("is(true, true):", Object.is(true, true));
console.log("is(1, '1'):", Object.is(1, "1"));

console.log("\n=== Object.getPrototypeOf ===");
const obj3 = {};
console.log("proto is Object.prototype:", Object.getPrototypeOf(obj3) === Object.prototype);
const arr = [];
console.log("arr proto is Array.prototype:", Object.getPrototypeOf(arr) === Array.prototype);

console.log("\n=== Object.setPrototypeOf ===");
const obj4 = {};
const newProto = { inherited: true };
Object.setPrototypeOf(obj4, newProto);
console.log("inherited:", obj4.inherited);
console.log("proto check:", Object.getPrototypeOf(obj4) === newProto);

console.log("\n=== Object.getOwnPropertyNames ===");
const obj5 = { a: 1, b: 2 };
Object.defineProperty(obj5, "c", { value: 3, enumerable: false });
console.log("own names:", JSON.stringify(Object.getOwnPropertyNames(obj5)));

console.log("\n=== Object.isExtensible ===");
const obj6 = {};
console.log("extensible:", Object.isExtensible(obj6));

console.log("\n=== Object.preventExtensions ===");
const obj7 = { x: 1, y: 2 };
Object.preventExtensions(obj7);
console.log("after prevent, extensible:", Object.isExtensible(obj7));
obj7.z = 3;
console.log("z after add attempt:", obj7.z);
