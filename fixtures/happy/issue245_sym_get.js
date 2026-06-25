const b = [3, 4];
b[Symbol.isConcatSpreadable] = false;
console.log("after set");
const k = Symbol.isConcatSpreadable;
console.log("has", k in b);