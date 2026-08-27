// callable 接收者沿 callable_prototypes 链的属性完整语义：
// OrdinaryGet / OrdinarySet / HasProperty 对函数对象自有与继承数据属性、
// 访问器属性、显式 null 原型与非 callable 原型的行为需与规范一致。

// 子类构造器继承基类静态数据属性（[[Get]] 沿链查找）。
class Base {}
Base.kind = "k";
class D extends Base {}
console.log("get:", D.kind);
console.log("in:", "kind" in D);
console.log("reflect-get:", Reflect.get(D, "kind"));
console.log("reflect-has:", Reflect.has(D, "kind"));
console.log("own:", D.hasOwnProperty("kind"));

// 多级链：孙类沿两级构造器继承。
class DD extends D {}
console.log("grand:", DD.kind);

// 自有属性遮蔽祖先，不影响基类；delete 自有后继承值重新可见。
D.kind = "own";
console.log("shadow:", D.kind, Base.kind, D.hasOwnProperty("kind"));
delete D.kind;
console.log("unshadow:", D.kind, D.hasOwnProperty("kind"));

// 继承的不可写数据属性拒绝写入：不建自有属性，值保持基类的。
class RoBase {}
Object.defineProperty(RoBase, "ro", { value: 1, writable: false });
class RoD extends RoBase {}
RoD.ro = 2;
console.log("ro:", RoD.ro, Object.getOwnPropertyDescriptor(RoD, "ro") === undefined);

// 继承访问器：getter/setter 的 this 为实际接收者（子类构造器）。
class AccBase {}
Object.defineProperty(AccBase, "acc", {
  get() {
    return "get/" + (this === AccD);
  },
  set(v) {
    this.stored = "set/" + v;
  },
});
class AccD extends AccBase {}
console.log("acc-get:", AccD.acc);
AccD.acc = 7;
console.log("acc-set:", AccD.stored, AccD.hasOwnProperty("stored"));

// 仅 getter 的继承访问器拒绝写入。
class GetOnlyBase {}
Object.defineProperty(GetOnlyBase, "g", {
  get() {
    return "only";
  },
});
class GetOnlyD extends GetOnlyBase {}
GetOnlyD.g = 9;
console.log("get-only:", GetOnlyD.g, Object.getOwnPropertyDescriptor(GetOnlyD, "g") === undefined);

// 函数原型改为普通对象：对象属性可见，Function.prototype 内建不再在链上。
function viaObject() {}
Object.setPrototypeOf(viaObject, { fromObject: 42 });
console.log("obj-proto:", viaObject.fromObject, "fromObject" in viaObject, viaObject.call);

// 显式 null 原型：链终止，call/apply 均不可见。
function nullProto() {}
Object.setPrototypeOf(nullProto, null);
console.log(
  "null-proto:",
  nullProto.call,
  "call" in nullProto,
  Object.getPrototypeOf(nullProto),
);

// extends 内建构造器：静态成员沿链解析到同一内建。
class DA extends Array {}
console.log("builtin-static:", DA.from === Array.from, DA.isArray === Array.isArray);

// Reflect.set 直接以函数为目标：正常建自有属性并返回 true。
function reflectTarget() {}
console.log("reflect-set:", Reflect.set(reflectTarget, "z", 3), reflectTarget.z);

// 惰性元数据的可写性对赋值可见：name 不可写（静默拒绝），prototype 可写。
function meta() {}
meta.name = "renamed";
console.log("meta-name:", meta.name);
meta.prototype = "replaced";
console.log("meta-prototype:", meta.prototype);
