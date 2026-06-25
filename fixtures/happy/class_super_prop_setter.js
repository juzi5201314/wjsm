// super.property assignment invokes parent setter with this as receiver.
class Base {
  set foo(v) {
    this._foo = v;
  }
  get foo() {
    return this._foo;
  }
}
class Derived extends Base {
  setFoo(v) {
    super.foo = v;
  }
}
const d = new Derived();
d.setFoo(42);
console.log(d.foo);