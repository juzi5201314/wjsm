const b = [3, 4];
b[Symbol.isConcatSpreadable] = false;
console.log("in", Symbol.isConcatSpreadable in b);
console.log(JSON.stringify([1,2].concat(b)));