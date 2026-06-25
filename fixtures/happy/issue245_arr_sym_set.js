const b = [3, 4];
b[Symbol.isConcatSpreadable] = false;
console.log("get", b[Symbol.isConcatSpreadable]);
console.log("typeof", typeof b[Symbol.isConcatSpreadable]);