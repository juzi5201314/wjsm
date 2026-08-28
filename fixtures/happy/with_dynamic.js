// with 动态分派：方法调用 this 绑定、@@unscopables（含 Array.prototype）、
// Proxy trap、原语 ToObject 装箱、null/undefined TypeError、未绑定名回退。
const obj = {
  name: "obj",
  greet() { return "hello from " + this.name; },
};
with (obj) {
  console.log(greet());
}

const arr = [1, 2, 3];
with (arr) {
  console.log(typeof keys, typeof values, length);
}

const un = { hidden: 1, visible: 2, [Symbol.unscopables]: { hidden: true } };
let hidden = "outer-hidden";
with (un) {
  console.log(hidden, visible);
}

const log = [];
const p = new Proxy({ q: 1 }, {
  has(t, k) { log.push("has:" + String(k)); return k in t; },
  get(t, k) { if (typeof k === "string") log.push("get:" + k); return t[k]; },
  set(t, k, v) { log.push("set:" + String(k)); t[k] = v; return true; },
});
with (p) {
  q = 41;
}
console.log(p.q, log.join(","));

with ("abc") {
  console.log(length, toUpperCase());
}
with (42) {
  console.log(toFixed(1));
}

try {
  with (null) {}
} catch (e) {
  console.log(e instanceof TypeError);
}
try {
  with (undefined) {}
} catch (e) {
  console.log(e instanceof TypeError);
}

with ({}) {
  console.log(typeof missing);
}
with ({}) {
  implicitGlobal = 7;
}
console.log(globalThis.implicitGlobal);
with ({}) {
  console.log(implicitGlobal);
  console.log(delete implicitGlobal, typeof implicitGlobal);
}
try {
  with ({}) {
    console.log(notDefined);
  }
} catch (e) {
  console.log(e instanceof ReferenceError, e.message);
}
