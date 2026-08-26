// Latin-1 SSO 属性键（≤6 码元）应零堆驻留。
const key = "\u00e9"; // é，单码元 Latin-1
const object = {};
object[key] = 42;
console.log(object[key], Object.keys(object)[0] === key);
