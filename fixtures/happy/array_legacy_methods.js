// Issue #34: legacy host imports arr_concat/slice/flat/fill (also exercised via prototype)
console.log([1, 2].concat([3, 4]).join(","));
console.log([1, 2, 3].slice(1).join(","));
console.log([1, 2, 3].slice(-1).join(","));
console.log(JSON.stringify([1, [2, [3]]].flat()));
console.log(JSON.stringify([1, [2, [3]]].flat(2)));
var a = [1, 2, 3];
a.fill(0, 1, 2);
console.log(a.join(","));
var b = [1, 2, 3];
b.fill(0);
console.log(b.join(","));