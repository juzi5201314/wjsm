var m = new Map();
m.set("a", 1);
m.set("b", 2);
var entries = Array.from(m.entries());
console.log(entries.length);
console.log(entries[0][0]);
console.log(entries[0][1]);
console.log(entries[1][0]);
console.log(entries[1][1]);

var s = new Set();
s.add(3);
s.add(4);
var setEntries = Array.from(s.entries());
console.log(setEntries.length);
console.log(setEntries[0][0]);
console.log(setEntries[0][1]);
console.log(setEntries[1][0]);
console.log(setEntries[1][1]);
