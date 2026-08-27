// SuperCall（ES §13.3.7.1）步骤 6–11：父构造器返回对象时，派生构造器的
// this 绑定重绑为该对象（BindThisValue），随后字段初始化落在该对象上
//（InitializeInstanceElements）；[[Construct]] 步骤 13：派生构造器正常
// 完结返回当前 this 绑定。
class Base {
  constructor() {
    return { marker: 1 };
  }
}

// 显式 super()：super() 之后 this 即父构造器返回的对象。
class ExplicitCtor extends Base {
  constructor() {
    super();
    console.log("explicit in-ctor:", this.marker);
  }
}
console.log("explicit:", new ExplicitCtor().marker);

// 隐式缺省构造器同样重绑。
class ImplicitCtor extends Base {}
console.log("implicit:", new ImplicitCtor().marker);

// 字段初始化器在重绑后的 this 上求值并定义（含 this 引用与私有字段）。
class WithFields extends Base {
  #secret = 10;
  field = 5;
  self = this;
  constructor() {
    super();
  }
  readSecret() {
    return this.#secret;
  }
}
const withFields = new WithFields();
console.log(
  "fields:",
  withFields.field,
  withFields.marker,
  withFields.self === withFields,
  WithFields.prototype.readSecret.call(withFields)
);

// 返回的对象原型是对象字面量的原型：派生类原型方法不可达。
console.log("proto:", Object.getPrototypeOf(withFields) === Object.prototype);

// 父构造器返回非对象 → [[Construct]] 结果仍是派生实例。
class NonObjectBase {
  constructor() {
    return 42;
  }
  protoMethod() {
    return "proto";
  }
}
class KeepsInstance extends NonObjectBase {
  constructor() {
    super();
    this.own = 7;
  }
}
const kept = new KeepsInstance();
console.log("non-object:", kept.own, kept.protoMethod());

// super() 是表达式：结果即 SuperCall 的值，且与重绑后的 this 同一。
class ExprPosition extends Base {
  f = 9;
  constructor() {
    const r = super();
    console.log("expr:", r === this, this.f, this.marker);
  }
}
new ExprPosition();

// if/else 双 super() 站点：每个站点都执行字段初始化。
class BranchSuper extends Base {
  tag = "b";
  constructor(cond) {
    if (cond) {
      super();
    } else {
      super();
    }
    console.log("branch:", this.tag, this.marker);
  }
}
new BranchSuper(true);
new BranchSuper(false);

// return super()：字段先初始化，随后返回 super() 的结果对象。
class ReturnSuper extends Base {
  g = 3;
  constructor() {
    return super();
  }
}
const returned = new ReturnSuper();
console.log("return-super:", returned.g, returned.marker);

// 显式 return 对象覆盖绑定 this；return undefined 返回绑定 this。
class ReturnsObject extends Base {
  constructor() {
    super();
    return { own: "x" };
  }
}
const overridden = new ReturnsObject();
console.log("return-object:", overridden.own, overridden.marker);
class ReturnsUndefined extends Base {
  constructor() {
    super();
    return undefined;
  }
}
console.log("return-undefined:", new ReturnsUndefined().marker);

// 多级继承：中间构造器的 this 重绑沿链传递到最外层 new。
class MidLevel extends Base {
  mid = "m";
  constructor() {
    super();
  }
}
class TopLevel extends MidLevel {
  top = "t";
}
const chained = new TopLevel();
console.log("chain:", chained.marker, chained.mid, chained.top);

// super(...spread) 同样重绑。
class SpreadBase {
  constructor(a, b) {
    return { sum: a + b };
  }
}
class SpreadSuper extends SpreadBase {
  constructor() {
    super(...[3, 4]);
  }
}
console.log("spread:", new SpreadSuper().sum);

// Reflect.construct 走同一构造协议。
class ReflectTarget extends Base {
  r = 8;
  constructor() {
    super();
  }
}
const reflected = Reflect.construct(ReflectTarget, []);
console.log("reflect:", reflected.marker, reflected.r);

// 派生构造器 return 原语（非 undefined）→ [[Construct]] 步骤 13.b TypeError，
// 且不可被构造器体内的 try/catch 捕获、finally 先于其执行。
class ReturnsPrimitive extends Base {
  constructor() {
    try {
      super();
      return 5;
    } finally {
      console.log("primitive: finally ran");
    }
  }
}
try {
  new ReturnsPrimitive();
} catch (error) {
  console.log("primitive:", error instanceof TypeError);
}
