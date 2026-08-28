// 函数级 "use strict" 与类体恒严格：各站点写失败必须抛 TypeError，
// 外层 sloppy 不受内层 directive 影响。
const frozen = Object.freeze({ n: 1 });

function fnDecl() {
  "use strict";
  try {
    frozen.n = 2;
    return "no throw";
  } catch (error) {
    return "fn-decl " + error.constructor.name;
  }
}
console.log(fnDecl());

const fnExpr = function () {
  "use strict";
  try {
    frozen.n = 2;
    return "no throw";
  } catch (error) {
    return "fn-expr " + error.constructor.name;
  }
};
console.log(fnExpr());

const arrow = () => {
  "use strict";
  try {
    frozen.n = 2;
    return "no throw";
  } catch (error) {
    return "arrow " + error.constructor.name;
  }
};
console.log(arrow());

// 严格函数内的嵌套 sloppy 函数继承严格性（只增不减）。
function outerStrict() {
  "use strict";
  function inner() {
    try {
      frozen.n = 2;
      return "no throw";
    } catch (error) {
      return "nested " + error.constructor.name;
    }
  }
  return inner();
}
console.log(outerStrict());

// 类体（方法 / 构造器 / 静态块 / 字段初始化器）恒为严格模式。
class Klass {
  static staticField = (() => {
    try {
      frozen.n = 2;
      return "no throw";
    } catch (error) {
      return "static-field " + error.constructor.name;
    }
  })();

  static {
    try {
      frozen.n = 2;
      console.log("no throw");
    } catch (error) {
      console.log("static-block " + error.constructor.name);
    }
  }

  constructor() {
    try {
      frozen.n = 2;
      this.report = "no throw";
    } catch (error) {
      this.report = "ctor " + error.constructor.name;
    }
  }

  method() {
    try {
      frozen.n = 2;
      return "no throw";
    } catch (error) {
      return "method " + error.constructor.name;
    }
  }
}
console.log(Klass.staticField);
const instance = new Klass();
console.log(instance.report);
console.log(instance.method());

// 对象字面量方法/访问器可携带自身 directive。
const literal = {
  strictMethod() {
    "use strict";
    try {
      frozen.n = 2;
      return "no throw";
    } catch (error) {
      return "obj-method " + error.constructor.name;
    }
  },
  sloppyMethod() {
    frozen.n = 2;
    return "obj-sloppy silent " + frozen.n;
  },
  get accessor() {
    "use strict";
    try {
      frozen.n = 2;
      return "no throw";
    } catch (error) {
      return "obj-getter " + error.constructor.name;
    }
  },
};
console.log(literal.strictMethod());
console.log(literal.sloppyMethod());
console.log(literal.accessor);

// 顶层保持 sloppy：静默失败，值不变。
frozen.n = 2;
console.log("top-level silent", frozen.n);
