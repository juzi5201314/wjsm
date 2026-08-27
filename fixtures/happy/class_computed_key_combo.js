// 类计算键组合：实例 + 静态 + 方法/访问器同键混合（IR 支配性回归用例）。
// 曾因类成员键/字段值 lowering 未做块线程化而触发
// "definition of value %N in bbX does not dominate use" IR 验证失败。

// 实例与静态字段共用同一标识符键
const k = "x";
class C1 {
  [k] = 1;
  static [k] = 2;
}
console.log(new C1().x, C1.x);

// 键为调用表达式（求值产生控制流/异常分叉）
function makeKey() {
  return "y";
}
class C2 {
  [makeKey()] = 3;
  static [makeKey()] = 4;
}
console.log(new C2().y, C2.y);

// 模板字面量键 + 方法计算键混合
let n = 0;
class C3 {
  [`f${n++}`] = 10;
  static [`f${n++}`] = 20;
  [`f${n++}`]() {
    return 30;
  }
}
const c3 = new C3();
console.log(c3.f0, C3.f1, c3.f2(), n);

// Symbol 键：实例字段 + 静态字段 + getter 计算键
const s = Symbol("k");
class C4 {
  [s] = 10;
  static [s] = 20;
  get ["g" + "x"]() {
    return this[s];
  }
}
const c4 = new C4();
console.log(c4[s], C4[s], c4.gx);

// getter/setter 计算键对
class C5 {
  #v = 0;
  get [("val")]() {
    return this.#v;
  }
  set [("val")](x) {
    this.#v = x;
  }
}
const c5 = new C5();
c5.val = 99;
console.log(c5.val);

// 派生类：计算字段在 super() 之后初始化
function dk() {
  return "d";
}
class Base {
  constructor() {
    this.a = 1;
  }
}
class Derived extends Base {
  [dk()] = 2;
  constructor() {
    super();
    this.b = 3;
  }
}
const d = new Derived();
console.log(d.a, d.d, d.b);

// 静态字段值为调用表达式（值求值控制流）
function val() {
  return 5;
}
class C6 {
  x = val();
  static y = val();
}
console.log(new C6().x, C6.y);
