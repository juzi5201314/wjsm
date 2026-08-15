var map = new Map();
var set = new Set();
var weakMap = new WeakMap();
var weakSet = new WeakSet();
var key = {};

console.log("Map self:", map instanceof Map, typeof (map instanceof Map));
console.log("Set self:", set instanceof Set, typeof (set instanceof Set));
console.log("WeakMap self:", weakMap instanceof WeakMap, typeof (weakMap instanceof WeakMap));
console.log("WeakSet self:", weakSet instanceof WeakSet, typeof (weakSet instanceof WeakSet));

console.log("Map vs Set:", map instanceof Set, typeof (map instanceof Set));
console.log("Map vs WeakMap:", map instanceof WeakMap, typeof (map instanceof WeakMap));
console.log("Map vs WeakSet:", map instanceof WeakSet, typeof (map instanceof WeakSet));
console.log("Set vs Map:", set instanceof Map, typeof (set instanceof Map));
console.log("Set vs WeakMap:", set instanceof WeakMap, typeof (set instanceof WeakMap));
console.log("Set vs WeakSet:", set instanceof WeakSet, typeof (set instanceof WeakSet));
console.log("WeakMap vs Map:", weakMap instanceof Map, typeof (weakMap instanceof Map));
console.log("WeakMap vs Set:", weakMap instanceof Set, typeof (weakMap instanceof Set));
console.log("WeakMap vs WeakSet:", weakMap instanceof WeakSet, typeof (weakMap instanceof WeakSet));
console.log("WeakSet vs Map:", weakSet instanceof Map, typeof (weakSet instanceof Map));
console.log("WeakSet vs Set:", weakSet instanceof Set, typeof (weakSet instanceof Set));
console.log("WeakSet vs WeakMap:", weakSet instanceof WeakMap, typeof (weakSet instanceof WeakMap));

console.log("Map prototype:", Object.getPrototypeOf(map) === Map.prototype, Map.prototype.constructor === Map);
console.log("Set prototype:", Object.getPrototypeOf(set) === Set.prototype, Set.prototype.constructor === Set);
console.log("WeakMap prototype:", Object.getPrototypeOf(weakMap) === WeakMap.prototype, WeakMap.prototype.constructor === WeakMap);
console.log("WeakSet prototype:", Object.getPrototypeOf(weakSet) === WeakSet.prototype, WeakSet.prototype.constructor === WeakSet);

console.log("Prototype keys:", Object.keys(Map.prototype).length, Object.keys(Set.prototype).length, Object.keys(WeakMap.prototype).length, Object.keys(WeakSet.prototype).length);
var setConstructorDescriptor = Object.getOwnPropertyDescriptor(Set.prototype, "constructor");
var weakMapConstructorDescriptor = Object.getOwnPropertyDescriptor(WeakMap.prototype, "constructor");
var weakSetConstructorDescriptor = Object.getOwnPropertyDescriptor(WeakSet.prototype, "constructor");
var setMethodDescriptor = Object.getOwnPropertyDescriptor(Set.prototype, "add");
var weakMapMethodDescriptor = Object.getOwnPropertyDescriptor(WeakMap.prototype, "set");
var weakSetMethodDescriptor = Object.getOwnPropertyDescriptor(WeakSet.prototype, "add");
console.log("Set constructor descriptor:", setConstructorDescriptor.enumerable, setConstructorDescriptor.writable, setConstructorDescriptor.configurable);
console.log("Set method descriptor:", setMethodDescriptor.enumerable, setMethodDescriptor.writable, setMethodDescriptor.configurable);
console.log("WeakMap constructor descriptor:", weakMapConstructorDescriptor.enumerable, weakMapConstructorDescriptor.writable, weakMapConstructorDescriptor.configurable);
console.log("WeakMap method descriptor:", weakMapMethodDescriptor.enumerable, weakMapMethodDescriptor.writable, weakMapMethodDescriptor.configurable);
console.log("WeakSet constructor descriptor:", weakSetConstructorDescriptor.enumerable, weakSetConstructorDescriptor.writable, weakSetConstructorDescriptor.configurable);
console.log("WeakSet method descriptor:", weakSetMethodDescriptor.enumerable, weakSetMethodDescriptor.writable, weakSetMethodDescriptor.configurable);
var constructorDescriptor = Object.getOwnPropertyDescriptor(Map.prototype, "constructor");
var methodDescriptor = Object.getOwnPropertyDescriptor(Map.prototype, "set");
console.log("Map constructor descriptor:", constructorDescriptor.enumerable, constructorDescriptor.writable, constructorDescriptor.configurable);
console.log("Map method descriptor:", methodDescriptor.enumerable, methodDescriptor.writable, methodDescriptor.configurable);

map.set("answer", 42);
set.add(7);
weakMap.set(key, 9);
weakSet.add(key);
console.log("Map method:", map.get("answer"));
console.log("Set method:", set.has(7));
console.log("WeakMap method:", weakMap.get(key));
console.log("WeakSet method:", weakSet.has(key));
