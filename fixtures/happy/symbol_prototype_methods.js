// ECMAScript §20.4.3 Symbol.prototype
const s = Symbol("x");
const u = Symbol();
console.log(s.toString());
console.log(u.toString());
console.log(s.valueOf() === s);
console.log(s.description);
console.log(u.description);
console.log(String(s));
console.log(String(u));
console.log(Symbol.prototype[Symbol.toStringTag]);