// ArraySpeciesCreate（§23.1.3.2）：map / filter / flatMap 结果构造器解析。

// Array[Symbol.species] 访问器本体（§23.1.2.5）
{
  const desc = Object.getOwnPropertyDescriptor(Array, Symbol.species);
  console.log("desc", typeof desc.get, desc.set, desc.enumerable, desc.configurable);
  console.log("getter", JSON.stringify(desc.get.name), desc.get.length);
  console.log("identity", Array[Symbol.species] === Array);
}

// 子类缺省继承：结果为子类实例
{
  class Sub extends Array {}
  const a = new Sub();
  a.push(1, 2, 3);
  console.log("sub map", a.map(x => x * 2) instanceof Sub);
  console.log("sub filter", a.filter(x => x > 1) instanceof Sub);
  console.log("sub flatMap", a.flatMap(x => [x]) instanceof Sub);
  console.log("sub species", Sub[Symbol.species] === Sub);
}

// 二级子类
{
  class L1 extends Array {}
  class L2 extends L1 {}
  const x = new L2();
  x.push(7);
  const r = x.map(v => v);
  console.log("two-level", r instanceof L2, r instanceof L1);
}

// 自定义自有 constructor 上的 species
{
  class Custom extends Array {}
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: Custom };
  console.log("custom map", a.map(x => x) instanceof Custom);
  console.log("custom filter", a.filter(x => x > 1) instanceof Custom);
  console.log("custom flatMap", a.flatMap(x => [x]) instanceof Custom);
}

// null / undefined species → 缺省 ArrayCreate
{
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: null };
  console.log("null species", Object.getPrototypeOf(a.map(x => x)) === Array.prototype);
  const b = [1, 2, 3];
  b.constructor = { [Symbol.species]: undefined };
  console.log("undef species", Object.getPrototypeOf(b.filter(x => x)) === Array.prototype);
}

// 子类显式指回 Array → 普通数组
{
  class Back extends Array {
    static get [Symbol.species]() {
      return Array;
    }
  }
  const s = new Back();
  s.push(1, 2);
  console.log("species=Array", Object.getPrototypeOf(s.map(x => x)) === Array.prototype);
}

// 非构造器 species → TypeError（V8 文案）
{
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: {} };
  try {
    a.map(x => x);
    console.log("non-ctor no-throw");
  } catch (e) {
    console.log("non-ctor", e.constructor.name, e.message);
  }
  const b = [1];
  b.constructor = 5;
  try {
    b.map(x => x);
    console.log("num-ctor no-throw");
  } catch (e) {
    console.log("num-ctor", e.constructor.name, e.message);
  }
}

// 抛错的 species getter / constructor getter：先于迭代传播
{
  const a = [1, 2, 3];
  a.constructor = {
    get [Symbol.species]() {
      throw new RangeError("boom-species");
    },
  };
  try {
    a.filter(x => x);
  } catch (e) {
    console.log("species-getter", e.constructor.name, e.message);
  }
  const b = [1, 2, 3];
  Object.defineProperty(b, "constructor", {
    get() {
      throw new Error("boom-ctor");
    },
  });
  try {
    b.flatMap(x => [x]);
  } catch (e) {
    console.log("ctor-getter", e.constructor.name, e.message);
  }
}

// 抛错的子类静态 species getter
{
  class Bad extends Array {
    static get [Symbol.species]() {
      throw new SyntaxError("bad-species");
    }
  }
  const b = new Bad();
  b.push(1);
  try {
    b.map(x => x);
  } catch (e) {
    console.log("bad-sub", e.constructor.name, e.message);
  }
}

// species 构造器实参：map «𝔽(len)»，filter / flatMap «+0𝔽»；构造先于回调
{
  const log = [];
  function C(n) {
    log.push("ctor:" + n);
  }
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: C };
  a.map(x => {
    log.push("cb:" + x);
    return x;
  });
  a.filter(x => true);
  a.flatMap(x => [x]);
  console.log("order", JSON.stringify(log));
}

// filter / flatMap 结果从 0 长度逐项增长
{
  class Grow extends Array {}
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: Grow };
  const f = a.filter(x => x !== 2);
  console.log("filter grow", f instanceof Grow, f.length, f[0], f[1]);
  const fm = a.flatMap(x => (x === 2 ? [] : [x, -x]));
  console.log("flatMap grow", fm instanceof Grow, fm.length, JSON.stringify(fm));
}

// 重定义 Array[Symbol.species] → 普通数组字面量的 map 也受影响；删除后回落缺省
{
  class Weird extends Array {}
  Object.defineProperty(Array, Symbol.species, {
    get() {
      return Weird;
    },
    configurable: true,
  });
  console.log("redefined", [1, 2].map(x => x) instanceof Weird);
  delete Array[Symbol.species];
  console.log("deleted", Array[Symbol.species], Object.getPrototypeOf([1].map(x => x)) === Array.prototype);
}

// 替换 Array.prototype.constructor → 自定义 species 可观察
{
  const orig = Array.prototype.constructor;
  function FakeCtor() {}
  FakeCtor[Symbol.species] = function C2() {};
  Array.prototype.constructor = FakeCtor;
  const r = [5, 6].filter(x => x > 4);
  console.log("proto-swap", Object.getPrototypeOf(r) !== Array.prototype, r.length, r[0], r[1]);
  Array.prototype.constructor = orig;
  console.log("restored", Object.getPrototypeOf([1].map(x => x)) === Array.prototype);
}

// 非数组接收者（array-like）：IsArray 为 false → 恒缺省，constructor 不读取
{
  let read = false;
  const like = {
    length: 2,
    0: "a",
    1: "b",
    get constructor() {
      read = true;
      return { [Symbol.species]: function X() {} };
    },
  };
  const r = Array.prototype.map.call(like, x => x + "!");
  console.log("array-like", Array.isArray(r), r[0], r[1], read);
}
