// Test Reflect.ownKeys returns proper array with string elements
const obj = { a: 1, b: 2, c: 3 };
const keys = Reflect.ownKeys(obj);

console.log(keys.length);
console.log(keys[0]);
console.log(keys[1]);
console.log(keys[2]);

// Test with empty object
const empty = {};
const emptyKeys = Reflect.ownKeys(empty);
console.log(emptyKeys.length);
