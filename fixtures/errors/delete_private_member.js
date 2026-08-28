// §13.5.1.1 早错误：delete 不得作用于私有成员引用（所有模式统一成立，
// 类体恒为严格代码）。V8 同口径文案「Private fields can not be deleted」。
class A {
  #x = 1;
  m() {
    delete this.#x;
  }
}
new A().m();
console.log("unreachable");
