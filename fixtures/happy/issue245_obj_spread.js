const obj = { length: 2, [Symbol.isConcatSpreadable]: true };
console.log(obj[Symbol.isConcatSpreadable]);
console.log(JSON.stringify([1].concat(obj)));