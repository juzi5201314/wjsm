// 统一对象协议：callable 与基元不再合成 %Object.prototype% 继承成员，
// 读取未命中后沿真实堆原型链上行（callable 链尾 / 基元包装对象的
// [[Prototype]]），删除 %Object.prototype% 自有属性对二者立即可见，
// 与 Node v22 逐字节一致（§10.1.8、§7.1.18、§20.1.3）。

// callable 经链尾 %Object.prototype% 命中真实自有属性。
function f() {}
console.log(f.hasOwnProperty("name"), f.propertyIsEnumerable("name"), f.valueOf() === f);

// 删除后 callable 的继承成员自然缺失（合成路径已移除）。
const saved = Object.prototype.hasOwnProperty;
delete Object.prototype.hasOwnProperty;
console.log(typeof f.hasOwnProperty);
Object.prototype.hasOwnProperty = saved;

// 基元读取未命中合成后进入真实堆原型链。
console.log((1).hasOwnProperty("x"), true.hasOwnProperty("x"), "s".hasOwnProperty(0));
Object.prototype.customTag = 7;
console.log((1).customTag, true.customTag, "s".customTag, 1n .customTag, Symbol().customTag);
delete Object.prototype.customTag;

// __proto__ getter 对基元：ToObject 包装对象的 [[Prototype]] 即 %X.prototype%。
console.log((42).__proto__ === Number.prototype, "s".__proto__ === String.prototype);
console.log(true.__proto__ === Boolean.prototype, false.__proto__ === Boolean.prototype);
console.log(Symbol().__proto__ === Symbol.prototype, 1n .__proto__ === BigInt.prototype);

// %Boolean.prototype% / %Symbol.prototype% 的 constructor 与方法为真实属性。
console.log(Boolean.prototype.constructor === Boolean, Symbol.prototype.constructor === Symbol);
console.log(true.toString(), false.valueOf(), Boolean.prototype.toString.call(true));
console.log(Object.getPrototypeOf(Boolean.prototype) === Object.prototype);

// 基元 this 的 propertyIsEnumerable：字符串索引可枚举，继承方法不算自有。
console.log((5).propertyIsEnumerable("toString"), "abc".propertyIsEnumerable(1), "abc".propertyIsEnumerable(9));

// isPrototypeOf 对基元 V 短路 false（临时包装对象不在任何链上）。
console.log(Number.prototype.isPrototypeOf(5), Object.prototype.isPrototypeOf(5));

// hasOwnProperty（ToPropertyKey 先行）与 Object.hasOwn（ToObject 先行）
// 的副作用顺序差：前者先触发键副作用再抛，后者直接抛。
const sideEffect = { toString() { console.log("key-effect"); return "k"; } };
try { Object.prototype.hasOwnProperty.call(null, sideEffect); } catch (e) { console.log("hop:", e.constructor.name); }
try { Object.hasOwn(null, sideEffect); } catch (e) { console.log("hasOwn:", e.constructor.name); }
