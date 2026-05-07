var obj = {};
var nullProtoObj = Object.create(null);
console.log(Object.getPrototypeOf(nullProtoObj) === null);
