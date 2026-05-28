var source = { x: 1, y: 2 };
var merged = { x: 0, ...source, y: 3 };
console.log(merged.x);
console.log(merged.y);
