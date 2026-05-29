// undefined handling: omitted in objects, becomes null in arrays.
const obj = { a: 1, b: undefined, c: 3 };
const arr = [1, undefined, 3];
console.log(JSON.stringify(obj));
console.log(JSON.stringify(arr));
