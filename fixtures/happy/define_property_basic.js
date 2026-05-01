var obj = {a: 1, b: 2};
Object.defineProperty(obj, "c", {value: 3, writable: false, enumerable: true, configurable: false});
console.log(obj.c);
console.log(delete obj.c);
Object.defineProperty(obj, "d", {value: 4});
console.log(obj.d);
