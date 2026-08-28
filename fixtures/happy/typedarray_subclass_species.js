// TypedArray 创建过程遵守 newTarget 与 species（FIX-09）：
// §23.2.5.1 AllocateTypedArray 经 newTarget.prototype 建实例原型；
// §23.2.3.24 slice / §23.2.3.28 subarray 经 TypedArraySpeciesCreate 构造结果。

// 11 种构造器的子类实例：instanceof / 原型 / @@species 静态继承
{
  function mk(Base) {
    return class extends Base {};
  }
  const kinds = [
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
    Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array,
    BigUint64Array,
  ];
  for (const Base of kinds) {
    const Sub = mk(Base);
    const inst = new Sub(2);
    console.log(
      Base.name,
      inst instanceof Sub,
      inst instanceof Base,
      Object.getPrototypeOf(inst) === Sub.prototype,
      inst.length,
      Sub[Symbol.species] === Sub,
      Base[Symbol.species] === Base,
    );
  }
}

// 子类构造实参形态：values / buffer 视图（别名）/ SharedArrayBuffer 视图
{
  class U1 extends Uint8Array {}
  const uv = new U1([5, 6, 7]);
  console.log("values", uv instanceof U1, uv.join(","));
  const ab = new ArrayBuffer(8);
  const ub = new U1(ab, 2, 4);
  console.log("bufview", ub instanceof U1, ub.byteOffset, ub.length, ub.buffer === ab);
  ub[0] = 9;
  console.log("bufview alias", new Uint8Array(ab)[2]);
  const sab = new SharedArrayBuffer(8);
  const us = new U1(sab);
  console.log("sabview", us instanceof U1, us.length);
  const uss = us.subarray(2, 5);
  uss[0] = 11;
  console.log("sab subarray", uss instanceof U1, uss.length, us[2]);
}

// 自定义构造器体：super() 后 this 即 TypedArray 实例，自有字段保留；
// slice 经 Construct(子类, «count») 再跑一遍构造器体
{
  class V extends Uint8Array {
    constructor(n) {
      super(n);
      this.tag = "v";
    }
  }
  const v = new V(3);
  console.log("fields", v instanceof V, v.tag, v.length);
  const vs = v.slice(1);
  console.log("field slice", vs instanceof V, vs.tag, vs.length);
}

// Reflect.construct 的 newTarget
{
  class U2 extends Uint8Array {}
  const r1 = Reflect.construct(Uint8Array, [2], U2);
  console.log("rc explicit", r1 instanceof U2, r1 instanceof Uint8Array, r1.length);
  const r2 = Reflect.construct(U2, [2]);
  console.log("rc self", r2 instanceof U2);
  const r3 = Reflect.construct(U2, [2], Uint8Array);
  console.log("rc back", r3 instanceof U2, Object.getPrototypeOf(r3) === Uint8Array.prototype);
  function F() {}
  F.prototype = 5;
  const r4 = Reflect.construct(Uint8Array, [2], F);
  console.log("rc non-object proto", Object.getPrototypeOf(r4) === Uint8Array.prototype, r4.length);
}

// slice / subarray：子类 species、二级子类、别名保持
{
  class U3 extends Uint8Array {}
  const u = new U3([1, 2, 3, 4]);
  const s = u.slice(1, 3);
  console.log("slice", s instanceof U3, s.constructor === U3, s.join(","));
  const sub = u.subarray(1, 3);
  console.log("subarray", sub instanceof U3, sub.length, sub.byteOffset, sub.buffer === u.buffer);
  sub[0] = 42;
  console.log("subarray alias", u[1]);
  class B1 extends Uint8Array {}
  class B2 extends B1 {}
  const b2 = new B2(3);
  console.log("two-level", b2.slice(1) instanceof B2, b2.slice(1) instanceof B1, b2.subarray(0, 2) instanceof B2);
  class BU extends BigUint64Array {}
  const bu = new BU(2);
  bu[0] = 7n;
  const bus = bu.slice();
  console.log("bigint", bus instanceof BU, bus[0] === 7n, bu.subarray(1) instanceof BU);
}

// species 构造先于元素读取；构造实参：slice «count»，subarray «buffer, byteOffset, newLength»
{
  const order = [];
  class Obs extends Uint8Array {
    constructor(...a) {
      order.push("ctor:" + a.map(x => (typeof x === "object" ? "buf" : String(x))).join("|"));
      super(...a);
    }
  }
  const ob = new Obs(new ArrayBuffer(8), 2, 4);
  order.length = 0;
  ob.slice(1, 3);
  console.log("slice args", JSON.stringify(order));
  order.length = 0;
  ob.subarray(1, 3);
  console.log("subarray args", JSON.stringify(order));
}

// species 显式指回内在构造器 / null / undefined → 缺省
{
  class Back extends Uint8Array {
    static get [Symbol.species]() {
      return Uint8Array;
    }
  }
  const b = new Back(3);
  console.log("back slice", Object.getPrototypeOf(b.slice()) === Uint8Array.prototype);
  console.log("back subarray", Object.getPrototypeOf(b.subarray()) === Uint8Array.prototype);
  class NullSp extends Uint8Array {
    static get [Symbol.species]() {
      return null;
    }
  }
  console.log("null species", Object.getPrototypeOf(new NullSp(2).slice()) === Uint8Array.prototype);
  const plain = new Uint8Array(2);
  plain.constructor = undefined;
  console.log("undef ctor", Object.getPrototypeOf(plain.slice()) === Uint8Array.prototype);
}

// 跨元素类型 species（同 content type）：值经转换复制
{
  const p = new Uint8Array([1, 44, 3]);
  p.constructor = { [Symbol.species]: Int32Array };
  const c = p.slice(0, 2);
  console.log("cross kind", c.constructor.name, c.join(","));
}

// species 协议的 TypeError / 异常传播（文案对齐 V8）
{
  class BadSp extends Uint8Array {
    static get [Symbol.species]() {
      return {};
    }
  }
  try {
    new BadSp(2).slice();
  } catch (e) {
    console.log("bad species", e.constructor.name, e.message);
  }
  const num = new Uint8Array(2);
  num.constructor = 5;
  try {
    num.slice();
  } catch (e) {
    console.log("non-object ctor slice", e.constructor.name, e.message);
  }
  try {
    num.subarray();
  } catch (e) {
    console.log("non-object ctor subarray", e.constructor.name, e.message);
  }
  const nonTa = new Uint8Array(2);
  nonTa.constructor = { [Symbol.species]: function () { return {}; } };
  try {
    nonTa.slice();
  } catch (e) {
    console.log("non-ta result", e.constructor.name, e.message);
  }
  const short = new Uint8Array(4);
  short.constructor = { [Symbol.species]: function () { return new Uint8Array(1); } };
  try {
    short.slice(0, 3);
  } catch (e) {
    console.log("short result", e.constructor.name, e.message);
  }
  const mixed = new Uint8Array(2);
  mixed.constructor = { [Symbol.species]: BigInt64Array };
  try {
    mixed.slice();
  } catch (e) {
    console.log("content type", e.constructor.name, e.message);
  }
  const ctorThrow = new Uint8Array(2);
  Object.defineProperty(ctorThrow, "constructor", {
    get() {
      throw new RangeError("boom-ctor");
    },
  });
  try {
    ctorThrow.slice();
  } catch (e) {
    console.log("ctor getter", e.constructor.name, e.message);
  }
  class ThrowSp extends Uint8Array {
    static get [Symbol.species]() {
      throw new SyntaxError("boom-sp");
    }
  }
  try {
    new ThrowSp(2).subarray();
  } catch (e) {
    console.log("species getter", e.constructor.name, e.message);
  }
}

// species 结果长于 count：多余元素保持初始值
{
  const long = new Uint8Array([9, 8, 7]);
  long.constructor = { [Symbol.species]: function (n) { return new Uint8Array(Number(n) + 2); } };
  console.log("long result", long.slice(0, 2).join(","));
}

// 缺省路径不受影响：内在构造器实例的原型与 species 快路径
{
  const plain = new Uint8Array([1, 2, 3]);
  console.log(
    "plain",
    Object.getPrototypeOf(plain) === Uint8Array.prototype,
    Object.getPrototypeOf(plain.slice(1)) === Uint8Array.prototype,
    Object.getPrototypeOf(plain.subarray(1)) === Uint8Array.prototype,
    plain.slice(1).join(","),
  );
}
