// Computed property access on plain objects
let obj = {};
let key = "name";
obj[key] = "wjsm";
console.log(obj[key]);
console.log(obj["name"]);
// Nested compound assignment
let count = 1;
obj["counter"] = count;
obj["counter"] = obj["counter"] + 1;
console.log(obj["counter"]);
