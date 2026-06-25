// Issue #245: Array.prototype.concat + Symbol.isConcatSpreadable
const a = [1, 2];
const b = [3, 4];
b[Symbol.isConcatSpreadable] = false;
console.log(JSON.stringify(a.concat(b)));

const obj = { length: 2, [Symbol.isConcatSpreadable]: true };
obj[0] = "x";
obj[1] = "y";
console.log(JSON.stringify(a.concat(obj)));

const item = [5, 6];
item[Symbol.isConcatSpreadable] = undefined;
console.log([].concat(item).length);