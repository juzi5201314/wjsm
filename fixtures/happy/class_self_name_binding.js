// 类自身名字绑定（CreateImmutableBinding(classBinding, true)）：类求值期
// （计算键）读为 TDZ ReferenceError；静态元素求值前 InitializeBinding
// （步骤 29）——静态字段与 static block 读到本次求值的类对象；初始化后写
// 在写点抛运行时 TypeError；命名类表达式的名字对体外不可见；类声明的外围
// 绑定保持可变，方法体经内层不可变绑定仍读到原类对象。
try {
  class G { [(() => G)()]() {} }
} catch (e) { console.log(e.constructor.name, e.message); }

class B { static s = B; static t = typeof B; }
console.log(B.s === B, B.t);

let cap;
class D { static { cap = D; } }
console.log(cap === D);

const K = class C2 { tag() { return C2; } };
console.log(typeof globalThis.C2, new K().tag() === K);

class A { m() { A = 1; } }
try { new A().m(); } catch (e) { console.log(e.constructor.name, e.message); }
class I { m() { I += 1; } }
try { new I().m(); } catch (e) { console.log(e.constructor.name, e.message); }
class J { m() { J++; } }
try { new J().m(); } catch (e) { console.log(e.constructor.name, e.message); }
class L { m() { [L] = [1]; } }
try { new L().m(); } catch (e) { console.log(e.constructor.name, e.message); }
class O { m() { O &&= 0; } }
try { new O().m(); } catch (e) { console.log(e.constructor.name, e.message); }

class E { tag() { return E; } }
const E0 = E;
E = 5;
console.log(E, E0.prototype.tag.call(0) === E0);
