// Map from array of [key, value] pairs
var m = new Map([["a", 1], ["b", 2]]);
console.log(m.size());
console.log(m.get("a"));
console.log(m.get("b"));
console.log(m.has("a"));
console.log(m.has("c"));

// Duplicate keys: last wins
var m2 = new Map([["x", 1], ["x", 2]]);
console.log(m2.size());
console.log(m2.get("x"));

// Empty array
var m3 = new Map([]);
console.log(m3.size());
