// §13.5.1.2 步骤 5.b：delete 的 SuperReference 恒抛 ReferenceError
// （V8 口径「Unsupported reference to 'super'」）。SuperProperty 求值先求
// 计算键并 GetValue（§13.3.7.1），键副作用与键异常先于本错误（Node 同序）。

// 类方法（类体恒为严格代码，delete super.x 不是 early error）。
class A {
  m() {
    try {
      delete super.x;
    } catch (error) {
      console.log("method:", error.constructor.name, error.message);
    }
  }
}
new A().m();

// 计算键：键表达式先求值（副作用可见），之后才抛 ReferenceError。
class B {
  m() {
    try {
      delete super[(console.log("key evaluated"), "k")];
    } catch (error) {
      console.log("computed:", error.constructor.name, error.message);
    }
  }
}
new B().m();

// 键求值抛出：键异常先于 super ReferenceError 传播。
class C {
  m() {
    try {
      delete super[
        (() => {
          throw new Error("key boom");
        })()
      ];
    } catch (error) {
      console.log("key-throw:", error.constructor.name, error.message);
    }
  }
}
new C().m();

// 对象字面量方法（sloppy 上下文中的 super 属性同样恒抛）。
const obj = {
  m() {
    try {
      delete super.x;
    } catch (error) {
      console.log("object-method:", error.constructor.name, error.message);
    }
  },
};
obj.m();

// 派生类构造器（super() 调用前）：V8 同样先报 super 引用错误。
class Base {}
class Derived extends Base {
  constructor() {
    try {
      delete super.p;
    } catch (error) {
      console.log("derived-ctor:", error.constructor.name, error.message);
    }
    super();
  }
}
new Derived();

// 静态方法中的 super 属性删除同样恒抛。
class S {
  static m() {
    try {
      delete super.s;
    } catch (error) {
      console.log("static:", error.constructor.name, error.message);
    }
  }
}
S.m();
