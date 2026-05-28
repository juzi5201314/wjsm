// Comprehensive Reflect.* test covering working methods
console.log("=== Reflect.has ===");
var obj3 = { name: "test" };
console.log("has name:", Reflect.has(obj3, "name"));
console.log("has age:", Reflect.has(obj3, "age"));

console.log("\n=== Reflect.deleteProperty ===");
var obj4 = { a: 1, b: 2 };
console.log("delete a:", Reflect.deleteProperty(obj4, "a"));
console.log("a after delete:", obj4.a);
console.log("has a:", Reflect.has(obj4, "a"));

console.log("\n=== Reflect.apply ===");
function sum(a, b) { return a + b; }
console.log("apply sum:", Reflect.apply(sum, null, [3, 4]));

console.log("\n=== Reflect.construct ===");
function Point(x, y) { this.x = x; this.y = y; }
var pt = Reflect.construct(Point, [10, 20]);
console.log("pt.x:", pt.x);
console.log("pt.y:", pt.y);

console.log("\n=== Reflect.setPrototypeOf ===");
var obj6 = {};
var customProto = { inherited: true };
console.log("set proto:", Reflect.setPrototypeOf(obj6, customProto));
console.log("inherited:", obj6.inherited);

console.log("\n=== Reflect.isExtensible ===");
var obj7 = {};
console.log("extensible:", Reflect.isExtensible(obj7));

console.log("\n=== Reflect.preventExtensions ===");
var obj8 = { a: 1 };
console.log("prevent:", Reflect.preventExtensions(obj8));
console.log("extensible after:", Reflect.isExtensible(obj8));

console.log("\n=== Reflect.getOwnPropertyDescriptor ===");
var obj9 = { x: 42 };
var desc = Reflect.getOwnPropertyDescriptor(obj9, "x");
console.log("value:", desc.value);
console.log("writable:", desc.writable);
console.log("enumerable:", desc.enumerable);
console.log("configurable:", desc.configurable);

console.log("\n=== Reflect.defineProperty ===");
var obj10 = {};
console.log("define:", Reflect.defineProperty(obj10, "prop", {
  value: 99,
  writable: true,
  enumerable: true,
  configurable: true
}));
console.log("prop value:", obj10.prop);
