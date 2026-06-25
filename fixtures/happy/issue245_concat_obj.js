const a = [1, 2];
const b = { 0: 3, 1: 4, length: 2 };
b[Symbol.isConcatSpreadable] = false;
console.log(JSON.stringify(a.concat(b)));