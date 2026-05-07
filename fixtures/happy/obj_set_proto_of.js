var obj = {};
var proto = {x: 99};
Object.setPrototypeOf(obj, proto);
console.log(obj.x);
