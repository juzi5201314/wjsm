// 内建函数与用户函数的 name/length 三特性均为 { writable: false,
// enumerable: false, configurable: true }（§10.2.9 / §10.2.10 / §17）：
// delete 后 own 层缺失且不复活，读取沿 %Function.prototype% 继承 ""/0，
// 赋值被继承只读数据属性拒绝，defineProperty 可恢复。输出与 Node 对拍。
function show(x) {
  console.log(JSON.stringify(x));
}
show(Object.getOwnPropertyDescriptor(Math.max, "name"));
show(Object.getOwnPropertyDescriptor(Math.max, "length"));

console.log(delete Math.max.name, delete Math.max.length);
console.log(Object.prototype.hasOwnProperty.call(Math.max, "name"));
show(Object.getOwnPropertyDescriptor(Math.max, "name"));
console.log(Math.max.name === "", Math.max.length === 0);
console.log("name" in Math.max, "length" in Math.max);
console.log(Object.getOwnPropertyNames(Math.max).join(","));

// 赋值被继承的只读 %Function.prototype%.name 拒绝（sloppy 静默失败）。
Math.max.name = "boom";
console.log(Math.max.name === "");

// defineProperty 恢复后描述符与读取一致。
Object.defineProperty(Math.max, "name", {
  value: "max",
  writable: false,
  enumerable: false,
  configurable: true,
});
show(Object.getOwnPropertyDescriptor(Math.max, "name"));
console.log(Math.max.name);

// 用户函数同一根因：删除后 own 缺失、继承 ""/0。
function foo(a, b) {}
console.log(delete foo.name, foo.name === "", Object.getOwnPropertyDescriptor(foo, "name") === undefined);
console.log(delete foo.length, foo.length === 0);

// 类静态链：删除子类 own name 后沿显式静态原型链取基类 name。
class B {}
class D extends B {}
console.log(delete D.name, D.name);

// test262 propertyHelper.verifyConfigurable 同型流程。
function verifyConfigurable(obj, key) {
  const desc = Object.getOwnPropertyDescriptor(obj, key);
  delete obj[key];
  const gone = !Object.prototype.hasOwnProperty.call(obj, key);
  Object.defineProperty(obj, key, desc);
  const restored = Object.getOwnPropertyDescriptor(obj, key);
  return gone && restored.value === desc.value && restored.configurable === true;
}
console.log(verifyConfigurable(JSON.parse, "name"), verifyConfigurable(JSON.parse, "length"));
console.log(verifyConfigurable([].values, "name"), verifyConfigurable("".charAt, "length"));
console.log(verifyConfigurable(Map, "name"), verifyConfigurable(Map, "length"));
console.log(verifyConfigurable(parseInt, "name"), verifyConfigurable(isNaN, "length"));
