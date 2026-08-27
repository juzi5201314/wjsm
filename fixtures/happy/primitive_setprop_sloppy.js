// sloppy 模式下对基元接收者的属性/下标写入是静默 no-op（PutValue →
// OrdinarySetWithOwnDescriptor 步骤 3.d.iv：Receiver 非对象返回 false），
// 不得崩溃也不得改变基元值；null/undefined base 则无论模式一律 TypeError。
var s = "hello";
s.x = 1;
console.log("str-prop", s.x);
s[0] = "H";
console.log("str-elem", s[0], s);
s.length = 3;
console.log("str-length", s.length);
s[99] = 1;
console.log("str-oob", s[99]);

var n = 5;
n.x = 1;
console.log("num-prop", n.x);

var b = true;
b.x = 1;
console.log("bool-prop", b.x);

var sym = Symbol("k");
sym.x = 1;
console.log("sym-prop", sym.x);

var big = 7n;
big.x = 1;
console.log("bigint-prop", big.x);

// 复合赋值 / 更新表达式同样走 SetProp，结果仍为 no-op。
var t = "xy";
t.n = (t.n || 0) + 1;
console.log("compound", t.n);
t.c++;
console.log("update", t.c);

// 赋值表达式自身的值是 RHS，与接收者是否可写无关。
var assigned = (s.y = 42);
console.log("assign-value", assigned);

// null / undefined base：ToObject 抛 TypeError（与 strict 无关）。
try {
  null.x = 1;
} catch (error) {
  console.log("null-prop", error.name, "|", error.message);
}
try {
  undefined.x = 1;
} catch (error) {
  console.log("undef-prop", error.name, "|", error.message);
}
var u;
try {
  u[3] = 1;
} catch (error) {
  console.log("undef-elem", error.name, "|", error.message);
}

// Reflect.set 显式传基元 receiver：OrdinarySet 返回 false，不抛错。
console.log("reflect-prim-receiver", Reflect.set({}, "x", 1, "prim"));

// 对象/数组写入不受影响（回归护栏）。
var obj = { a: 1 };
obj.a = 2;
obj["b"] = 3;
var arr = [1, 2, 3];
arr[1] = 9;
console.log("object-control", obj.a, obj.b, arr[1]);
