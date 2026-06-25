// super 属性复合赋值与自增/自减（Reflect.get + Reflect.set + 当前 this 作为 receiver）
class Base {
  get count() { return this._count || 0; }
  set count(v) { this._count = v; }
}
class Derived extends Base {
  increment() { super.count++; }
  decrement() { super.count--; }
  add(n) { super.count += n; return super.count; }
  setVal(v) { super.count = v; }
}
const d = new Derived();
d.setVal(10);
console.log(d.count);
d.increment();
console.log(d.count);
console.log(d.add(5));
d.decrement();
console.log(d.count);