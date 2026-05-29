// replacer as array whitelist — documents current behavior.
const data = { a: 1, b: 2, c: 3, d: 4 };
const result = JSON.stringify(data, ["a", "c"]);
console.log(result);
