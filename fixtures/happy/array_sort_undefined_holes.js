// Array.prototype.sort：undefined 置后，hole 置最后且保持为 hole（ES2024 §23.1.3.30）
var a = [3, undefined, 1, , 2];
a.sort();
console.log(a[0], a[1], a[2], a[3], a[4]);
console.log(0 in a, 1 in a, 2 in a, 3 in a, 4 in a);

var b = [10, , 5];
b.sort();
console.log(b[0], b[1], b[2], 0 in b, 1 in b, 2 in b);