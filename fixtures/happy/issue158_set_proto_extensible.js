// #158 Object.setPrototypeOf 对不可扩展对象改原型必须抛 TypeError，且原型保持不变
const o = {};
Object.preventExtensions(o);
let caught = "none";
try {
  Object.setPrototypeOf(o, { a: 1 });
} catch (e) {
  caught = e instanceof TypeError ? "TypeError" : e.name;
}
// 原型未被改写：o 不应继承 a
console.log(caught, o.a === undefined);
