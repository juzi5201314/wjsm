// 方法体的严格性决定 arguments 形态：对象字面量方法/访问器沿用外层松散模式
// （§10.2.11 步骤 22.a 判定为 mapped），类体恒严格（§15.7.1）故为 unmapped。

function calleeKind(args) {
  try {
    args.callee;
    return "no-throw";
  } catch (error) {
    return error.constructor.name;
  }
}

// 简写方法：双向同步 + callee 为数据属性。
const shorthand = {
  m(a) {
    a = 2;
    const afterParamWrite = arguments[0];
    arguments[0] = 3;
    return [afterParamWrite, a, calleeKind(arguments)].join(",");
  },
};
console.log("shorthand", shorthand.m(1));

// 函数属性与简写方法同形态。
const property = {
  m: function (a) {
    arguments[0] = 3;
    return [a, calleeKind(arguments)].join(",");
  },
};
console.log("property", property.m(1));

// setter 的单形参同样进 [[ParameterMap]]。
const accessor = {
  got: 0,
  set s(a) {
    arguments[0] = 3;
    this.got = a;
  },
  get g() {
    return [arguments.length, calleeKind(arguments)].join(",");
  },
};
accessor.s = 1;
console.log("setter", accessor.got);
console.log("getter", accessor.g);

// 计算键方法、generator 方法、async 方法都走同一判定。
const key = "computed";
const others = {
  [key](a) {
    a = 2;
    return arguments[0];
  },
  *gen(a) {
    a = 2;
    yield arguments[0];
  },
  async asyncMethod(a) {
    await null;
    a = 2;
    return arguments[0];
  },
};
console.log("computed", others.computed(1));
console.log("generator", [...others.gen(1)].join(","));
others.asyncMethod(1).then((value) => console.log("async", value));

// 非简单形参列表的对象方法回到 unmapped。
const nonSimple = {
  m(a = 0) {
    a = 2;
    return [arguments[0], calleeKind(arguments)].join(",");
  },
};
console.log("non-simple", nonSimple.m(1));

// 类体恒严格：方法、访问器、构造器都是 unmapped。
class C {
  constructor(a) {
    a = 2;
    this.result = [arguments[0], calleeKind(arguments)].join(",");
  }

  m(a) {
    a = 2;
    return [arguments[0], calleeKind(arguments)].join(",");
  }

  static s(a) {
    a = 2;
    return [arguments[0], calleeKind(arguments)].join(",");
  }
}
console.log("class-ctor", new C(1).result);
console.log("class-method", new C(0).m(1));
console.log("class-static", C.s(1));
