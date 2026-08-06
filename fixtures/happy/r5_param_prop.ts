// TypeScript 参数属性：`constructor(private x: number)` 既声明形参又声明实例字段。
// 曾整个 TsParamProp 被丢弃，导致形参与字段都不存在（`this.x` 读到 undefined）。

// 1. 基础：private / public 修饰符都产生实例字段
class P {
  constructor(private x: number, public y: string) {}
  read() {
    return this.x;
  }
}
const p = new P(42, "hi");
console.log(p.read(), p.y);

// 2. 带默认值的参数属性
class A {
  constructor(
    protected a: number,
    private b = 7,
  ) {}
  sum() {
    return this.a + this.b;
  }
}
console.log(new A(3).sum(), new A(3, 10).sum());

// 3. 派生类：`this` 在 super() 之后才存在，字段赋值须推迟到 super() 之后
class B extends A {
  constructor(
    public tag: string,
    n: number,
  ) {
    super(n);
  }
  info() {
    return this.tag + ":" + this.sum();
  }
}
const b = new B("t", 3);
console.log(b.info(), b.tag);

// 4. 字段声明顺序 = 参数属性先于字段初始化器（故初始化器可读到参数属性）
class C {
  d = this.c * 2;
  constructor(private c: number) {}
  read() {
    return this.d;
  }
}
console.log(new C(5).read());

// 5. 参数属性与 rest 形参混用
class D {
  y = 0;
  constructor(
    readonly x: number,
    ...rest: number[]
  ) {
    this.y = rest.length;
  }
}
const d = new D(1, 2, 3);
console.log(d.x, d.y);

// 6. 枚举顺序：参数属性按声明序排在字段初始化器之前
console.log(JSON.stringify(Object.keys(b)));
console.log(JSON.stringify(Object.keys(new C(1))));

// 7. 参数属性可被构造器体重新赋值
class E {
  constructor(public v: number) {
    this.v = v + 100;
  }
}
console.log(new E(1).v);
