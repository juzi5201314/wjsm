// 箭头函数中的 super()：BindThisValue 经 GetThisEnvironment 沿环境链
// 定位构造器帧的 this 绑定；构造器帧与箭头帧观察到同一重绑结果。
// 同时覆盖词法 this 捕获的槽命名一致性（方法内直接 this 访问与箭头捕获
// 共存、嵌套箭头逐层向外传播捕获）。
class ObjectBase {
  constructor() {
    return { marker: 7 };
  }
}
class PlainBase {
  constructor() {
    this.b = 2;
  }
}

// 方法内直接 this 访问 + 箭头捕获 this 共存。
class MixedThis {
  constructor() {
    this.x = 42;
    const read = () => this.x;
    console.log("mixed:", read(), this.x);
  }
}
new MixedThis();

// 嵌套箭头逐层捕获词法 this。
class NestedArrows {
  measure() {
    this.v = 5;
    const outer = () => {
      const inner = () => this.v;
      return inner();
    };
    return outer();
  }
}
console.log("nested:", new NestedArrows().measure());

// 普通函数内嵌套箭头的词法 this。
function nestedInFunction() {
  const outer = () => {
    const inner = () => this.w;
    return inner();
  };
  return outer();
}
console.log("function:", nestedInFunction.call({ w: 3 }));

// 箭头内 super()：基类不返回对象，this 仍是分配的实例。
class ArrowSuperPlain extends PlainBase {
  constructor() {
    (() => super())();
    console.log("arrow-plain:", this.b);
  }
}
new ArrowSuperPlain();

// 箭头内 super()：基类返回对象，构造器帧观察到重绑后的 this。
class ArrowSuperObject extends ObjectBase {
  constructor() {
    (() => super())();
    console.log("arrow-object:", this.marker);
  }
}
console.log("arrow-object result:", new ArrowSuperObject().marker);

// super() 之后创建的箭头读取重绑后的 this。
class ArrowAfterSuper extends ObjectBase {
  constructor() {
    super();
    const read = () => this.marker;
    console.log("arrow-after:", read());
  }
}
new ArrowAfterSuper();

// 箭头 super() 站点同样发射 InitializeInstanceElements（SuperCall 步骤 11）：
// 字段初始化在箭头帧、重绑后的 this 上执行。
class ArrowSuperFields extends ObjectBase {
  #secret = 10;
  field = 5;
  read = () => this.marker;
  constructor() {
    (() => super())();
    console.log(
      "arrow-fields:",
      this.field,
      this.marker,
      this.#secret,
      this.read()
    );
  }
}
new ArrowSuperFields();

// 嵌套箭头 super()：初始化上下文逐层克隆进入。
class NestedArrowSuper extends ObjectBase {
  n = 4;
  constructor() {
    (() => (() => super())())();
    console.log("nested-arrow-super:", this.n, this.marker);
  }
}
new NestedArrowSuper();

// 字段初始化器引用外层绑定：箭头帧发射时沿捕获链解析。
const outerBinding = 6;
class ArrowSuperOuterRef extends ObjectBase {
  o = outerBinding;
  constructor() {
    (() => super())();
    console.log("arrow-outer-ref:", this.o, this.marker);
  }
}
new ArrowSuperOuterRef();
