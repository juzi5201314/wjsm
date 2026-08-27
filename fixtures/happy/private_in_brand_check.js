// `#x in obj` 人体工学 brand 检查（ES §13.10.1）：
// 私有名作为 in 左操作数时查 receiver 的私有槽，字段/方法/访问器同一存储；
// receiver 非对象抛 TypeError，文案与 V8/Node 对齐（实例私有方法/访问器
// 显示类 brand 名，其余显示 `#名`）。
// 矩阵：实例/static × 字段/方法/getter/setter × 存在/不存在 × exotic 对象
// × 嵌套类遮蔽 × 子类继承 × RHS 求值顺序/异常传播 × 逻辑链 × generator/async。

class P {
  #f = 1;
  #m() {}
  get #g() {
    return 1;
  }
  set #s(v) {}
  static #sm() {}
  static #sf = 2;
  static tf(o) {
    return #f in o;
  }
  static tm(o) {
    return #m in o;
  }
  static tg(o) {
    return #g in o;
  }
  static ts(o) {
    return #s in o;
  }
  static tsm(o) {
    return #sm in o;
  }
  static tsf(o) {
    return #sf in o;
  }
}

const p = new P();
console.log("instance:", P.tf(p), P.tm(p), P.tg(p), P.ts(p));
console.log("static-on-ctor:", P.tsm(P), P.tsf(P), P.tf(P));
console.log("static-on-instance:", P.tsm(p), P.tsf(p));
console.log("plain-objects:", P.tf({}), P.tm([]), P.tg(function () {}));
console.log(
  "exotic:",
  P.tf(new Proxy({}, {})),
  P.tf(/re/),
  P.tf(Object.create(P.prototype)),
);

// receiver 非对象：TypeError，显示名按 V8 规则（字段 #f、实例方法/访问器
// 为类名 P、static 成员 #sm / #sf）。
for (const [label, fn] of [
  ["f", P.tf],
  ["m", P.tm],
  ["g", P.tg],
  ["s", P.ts],
  ["sm", P.tsm],
  ["sf", P.tsf],
]) {
  try {
    fn(1);
  } catch (e) {
    console.log(label, "=>", e.constructor.name, "|", e.message);
  }
}
for (const value of [null, undefined, "text", true, 42.5, 123n, NaN]) {
  try {
    P.tf(value);
  } catch (e) {
    console.log("primitive =>", e.message);
  }
}
try {
  P.tf(Symbol("k"));
} catch (e) {
  console.log("symbol =>", e.message);
}

// 嵌套类遮蔽：内层 #x 与外层 #x 是不同私有名。
class Outer {
  #x = "outer";
  static probe(o) {
    return #x in o;
  }
  static viaInner(o) {
    class Inner {
      #x = "inner";
      static probe(v) {
        return #x in v;
      }
    }
    return [Inner.probe(new Inner()), Inner.probe(o), Outer.probe(new Inner())];
  }
}
console.log("nested:", Outer.viaInner(new Outer()).join(","));

// 子类实例经基类构造获得基类 brand；基类实例没有子类 brand。
class Base {
  #b = 1;
  static hasB(o) {
    return #b in o;
  }
}
class Sub extends Base {
  #s = 2;
  static hasS(o) {
    return #s in o;
  }
}
const sub = new Sub();
console.log("subclass:", Base.hasB(sub), Sub.hasS(sub), Sub.hasS(new Base()));

// RHS 先于 brand 检查求值（求值顺序可观测）；RHS 抛异常按 ? GetValue 传播。
let order = [];
class S {
  #k = 1;
  static t() {
    return #k in (order.push("rhs"), new S());
  }
  static boom() {
    try {
      return (
        #k in
        (() => {
          throw new Error("rhs throw");
        })()
      );
    } catch (e) {
      return "caught:" + e.message;
    }
  }
}
console.log("order:", S.t(), order.join(","));
console.log("rhs-throw:", S.boom());

// 逻辑链短路组合。
class L {
  #a = 1;
  #b;
  static t(o) {
    return #a in o && #b in o;
  }
}
console.log("chain:", L.t(new L()), L.t({}));

// 方法体内 try/catch 本地捕获 brand TypeError（不依赖调用点外层捕获）。
class InMethod {
  #q = 1;
  static probe(o) {
    try {
      return #q in o;
    } catch (e) {
      return "local:" + e.message;
    }
  }
}
console.log("in-method:", InMethod.probe(new InMethod()), InMethod.probe(0));

// 类表达式的 brand 显示名：匿名为 'anonymous'，命名用源码名（非推断名）。
const AnonExpr = class {
  #m() {}
  static t(o) {
    try {
      return #m in o;
    } catch (e) {
      return e.message;
    }
  }
};
const NamedExpr = class Y {
  #m() {}
  static t(o) {
    try {
      return #m in o;
    } catch (e) {
      return e.message;
    }
  }
};
console.log("anon-expr:", AnonExpr.t(1));
console.log("named-expr:", NamedExpr.t(1));

// static 块内 brand 检查（this 为构造器）。
class SB {
  static #x = 1;
  static result;
  static {
    this.result = #x in this;
  }
}
console.log("static-block:", SB.result);

// generator / async 上下文：结果与异常路径。
class A {
  #v = 7;
  *gen(o) {
    yield (#v in o);
  }
  async check(o) {
    await Promise.resolve();
    return #v in o;
  }
  async viaThrow() {
    await Promise.resolve();
    return #v in
      (() => {
        throw new Error("rhs-async");
      })();
  }
  async viaPrimitive() {
    await Promise.resolve();
    return #v in 1;
  }
  async awaitRhs(p) {
    return #v in (await p);
  }
}
const a = new A();
console.log("gen:", a.gen(a).next().value, a.gen({}).next().value);
a.check(a)
  .then((r1) => a.check({}).then((r2) => console.log("async:", r1, r2)))
  .then(() => a.viaThrow())
  .then(
    () => console.log("viaThrow resolved"),
    (e) => console.log("viaThrow rejected:", e.message),
  )
  .then(() => a.viaPrimitive())
  .then(
    () => console.log("viaPrimitive resolved"),
    (e) => console.log("viaPrimitive rejected:", e.constructor.name, "|", e.message),
  )
  .then(() => a.awaitRhs(Promise.resolve(a)))
  .then((r1) =>
    a.awaitRhs(Promise.resolve({})).then((r2) => console.log("await-rhs:", r1, r2)),
  );
