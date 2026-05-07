var proto = {x: 42};
var obj = Object.create(proto);
console.log(obj.x);
console.log(Object.getPrototypeOf(obj) === proto);
