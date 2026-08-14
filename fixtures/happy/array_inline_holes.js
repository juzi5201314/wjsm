let a = [1, , 3];
let b = a.map(x => x * 2);
console.log(b.length, 0 in b, 1 in b, 2 in b, b[2]);
let c = a.filter(x => true);
console.log(c.length, c[0], c[1]);
let d = a.reduce((acc, x) => acc + (x || 0), 0);
console.log(d);
