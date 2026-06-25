const b = [3, 4];
Object.defineProperty(b, Symbol.isConcatSpreadable, { value: false, configurable: true, writable: true, enumerable: true });
console.log(JSON.stringify([1,2].concat(b)));