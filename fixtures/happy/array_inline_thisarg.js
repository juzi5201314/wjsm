let a = [1, 2, 3];
let obj = { k: 10 };
let b = a.map(function(x) { return this.k + x; }, obj);
console.log(b[0], b[1], b[2]);
let c = a.filter(function(x) { return this.k <= x; }, obj);
console.log(c.length, c[0], c[1]);
