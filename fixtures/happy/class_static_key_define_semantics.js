// 静态成员计算键 "prototype" 守卫与 DefineField 的 CreateDataPropertyOrThrow
// 语义：键求值后立即抛 TypeError（初始化器与后续键不再求值）；自有访问器被
// 数据属性整槽替换；不可配置属性/不可扩展对象按规范拒绝。

// 守卫矩阵：仅静态成员的计算键 "prototype" 抛 TypeError；
// "constructor" 计算键、实例 "prototype"、Symbol 静态键全部放行。
const t = (label, fn) => {
  try {
    fn();
    console.log(label, "ok");
  } catch (e) {
    console.log(label, e.constructor.name);
  }
};
const kc = "constructor";
const kp = "prototype";
t("inst-field-ctor", () => {
  class C {
    [kc] = 1;
  }
});
t("static-field-ctor", () => {
  class C {
    static [kc] = 1;
  }
});
t("static-field-proto", () => {
  class C {
    static [kp] = 1;
  }
});
t("static-method-proto", () => {
  class C {
    static [kp]() {}
  }
});
t("static-getter-proto", () => {
  class C {
    static get [kp]() {
      return 1;
    }
  }
});
t("inst-field-proto", () => {
  class C {
    [kp] = 1;
  }
});
t("static-field-symbol", () => {
  class C {
    static [Symbol.iterator] = 1;
  }
});

// 守卫在键求值后立即触发：后续键与全部初始化器不再求值
const log = [];
try {
  class Eager {
    static [(log.push("k1"), "a")] = (log.push("v1"), 1);
    static [(log.push("k2"), "prototype")] = (log.push("v2"), 2);
    static [(log.push("k3"), "b")] = (log.push("v3"), 3);
  }
} catch (e) {
  log.push("caught");
}
console.log(log.join(","));

// 静态字段以数据属性整槽覆盖先前同名静态访问器（CreateDataPropertyOrThrow）
class SG {
  static get x() {
    return 1;
  }
  static x = 2;
}
const sgd = Object.getOwnPropertyDescriptor(SG, "x");
console.log(SG.x, sgd.value, sgd.writable, sgd.enumerable, sgd.configurable, typeof sgd.get);

// 基类构造器定义的自有可配置访问器被派生类字段整槽替换为数据属性
class AB {
  constructor() {
    Object.defineProperty(this, "x", {
      get() {
        return 1;
      },
      configurable: true,
    });
  }
}
class AD extends AB {
  x = 2;
}
const ad = new AD();
const add = Object.getOwnPropertyDescriptor(ad, "x");
console.log(ad.x, add.value, add.writable, typeof add.get);

// 不可配置自有属性与字段冲突：TypeError
class NB {
  constructor() {
    Object.defineProperty(this, "y", {
      value: 1,
      writable: false,
      configurable: false,
    });
  }
}
class ND extends NB {
  y = 2;
}
try {
  new ND();
} catch (e) {
  console.log("nonconfigurable:", e.constructor.name);
}

// 冻结实例上的字段定义：TypeError
class FB {
  constructor() {
    Object.freeze(this);
  }
}
class FD extends FB {
  z = 1;
}
try {
  new FD();
} catch (e) {
  console.log("frozen:", e.constructor.name);
}

// 静态字段可合法覆盖函数固有的 name / length（可配置属性）
class NL {
  static name = "renamed";
  static length = 5;
}
console.log(NL.name, NL.length);
