// ClassDefinitionEvaluation 求值序：全部成员计算键（含字段）在类定义期按源
// 顺序求值一次；静态元素（静态字段初始化器 / static block）在键求值完毕后按
// 源顺序执行；实例字段初始化器在构造期复用已求值的键，不重新求值键表达式。

// 键求值一次 + 全序（对齐 Node：k1,k2,k3,k4,v2,sb,v4,|,v1）
const log = [];
const k = (i) => {
  log.push("k" + i);
  return "p" + i;
};
const v = (i) => {
  log.push("v" + i);
  return i;
};
class C {
  [k(1)] = v(1);
  static [k(2)] = v(2);
  [k(3)]() {
    return "m";
  }
  static {
    log.push("sb");
  }
  static [k(4)] = v(4);
}
log.push("|");
new C();
console.log(log.join(","));

// 多次构造不重复求值键
let count = 0;
const onceKey = () => {
  count++;
  return "one";
};
class Once {
  [onceKey()] = count;
}
new Once();
new Once();
console.log(count, new Once().one);

// ToPropertyKey 再入（对象键 toString）只在定义期调用一次
let toStringCalls = 0;
const keyObj = {
  toString() {
    toStringCalls++;
    return "obj" + toStringCalls;
  },
};
class Reentry {
  [keyObj] = "v";
}
new Reentry();
console.log(toStringCalls, new Reentry().obj1);

// 循环内的类表达式：每次迭代的键与初始化器彼此隔离
const ctors = [];
for (let i = 0; i < 3; i++) {
  ctors.push(
    class {
      [`it${i}`] = i;
    },
  );
}
console.log(new ctors[0]().it0, new ctors[1]().it1, new ctors[2]().it2);

// DefineField：字段定义是自有数据属性，原型链 setter 不触发
let setterCalled = false;
class WithSetter {
  set x(_) {
    setterCalled = true;
  }
}
class Shadow extends WithSetter {
  x = 1;
}
const sh = new Shadow();
console.log(setterCalled, sh.x);

// 静态私有方法在 static block 执行前已绑定
class SP {
  static #m() {
    return 41;
  }
  static {
    console.log(this.#m() + 1);
  }
}

// 键表达式抛异常：类定义中止，可被捕获
let caught = "";
try {
  class Boom {
    [(() => {
      throw new Error("keyboom");
    })()] = 1;
  }
} catch (e) {
  caught = e.message;
}
console.log(caught);

// ToPropertyKey 转换抛异常（Symbol.toPrimitive 抛 TypeError）同样传播
let caught2 = "";
const badKey = {
  [Symbol.toPrimitive]() {
    throw new TypeError("badkey");
  },
};
try {
  class Boom2 {
    static [badKey] = 1;
  }
} catch (e) {
  caught2 = e.message;
}
console.log(caught2);
