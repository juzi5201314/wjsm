function F() {}
F.prototype = 42;
let f = new F();
f.x = 10;
console.log(f.x);
