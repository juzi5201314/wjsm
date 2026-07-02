// Array.of (ES2015 §23.1.2.3)

console.log(JSON.stringify(Array.of(1, 2, 3)));
console.log(JSON.stringify(Array.of(7)));
console.log(JSON.stringify(Array.of()));

// 不同类型
console.log(JSON.stringify(Array.of("a", "b", "c")));
console.log(JSON.stringify(Array.of(1, "two", true, null, undefined)));
