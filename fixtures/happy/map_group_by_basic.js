const m = Map.groupBy([1, 2, 3, 4, 5], x => x % 2 === 0 ? "even" : "odd");
// Map.groupBy returns a Map with keys "even" and "odd", each an array
const keys = Array.from(m.keys());
keys.sort();
console.log(keys.length === 2 ? "PASS keys" : "FAIL keys=" + keys.length);
const even = m.get("even");
const odd = m.get("odd");
console.log(even && even.length === 2 ? "PASS even" : "FAIL even");
console.log(odd && odd.length === 3 ? "PASS odd" : "FAIL odd");
console.log(even && even[0] === 2 ? "PASS even[0]" : "FAIL");
console.log(even && even[1] === 4 ? "PASS even[1]" : "FAIL");
console.log(odd && odd[0] === 1 ? "PASS odd[0]" : "FAIL");
console.log(odd && odd[1] === 3 ? "PASS odd[1]" : "FAIL");
console.log(odd && odd[2] === 5 ? "PASS odd[2]" : "FAIL");
