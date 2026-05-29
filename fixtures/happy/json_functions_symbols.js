// Functions and symbols are omitted from objects, symbol becomes null in arrays.
const obj = { a: 1, fn: function(){}, sym: Symbol("x") };
const arr = [1, function(){}, Symbol("y")];
console.log(JSON.stringify(obj));
console.log(JSON.stringify(arr));
