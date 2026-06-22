// Object.getOwnPropertyDescriptor with Symbol keys (#185)
const sym = Symbol("tag");
const o = {};
o[sym] = 42;
const d = Object.getOwnPropertyDescriptor(o, sym);
console.log(d && d.value === 42, d && d.enumerable);