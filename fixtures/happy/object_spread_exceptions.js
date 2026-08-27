// 对象字面量的异常传播语义（ECMAScript PropertyDefinitionEvaluation /
// CopyDataProperties）：spread 源求值抛错、源自有属性 getter 抛错、
// 属性值求值抛错都必须传播，不得静默产生残缺对象。
function boom() {
  throw new Error("boom");
}

// spread 源调用抛异常 → 传播
try {
  const o = { ...boom() };
  console.log("FAIL spread source", Object.keys(o).length);
} catch (e) {
  console.log("spread source:", e.message);
}

// 源自有属性 getter 抛错 → CopyDataProperties 传播
const getterThrows = {
  get bad() {
    throw new Error("getter-throw");
  },
};
try {
  const o = { ...getterThrows };
  console.log("FAIL getter", Object.keys(o).length);
} catch (e) {
  console.log("getter:", e.message);
}

// getter 之前的属性照常复制后异常仍传播（残缺对象不可逃逸）
const partial = {
  a: 1,
  get bad() {
    throw new Error("partial-throw");
  },
  c: 3,
};
try {
  const o = { ...partial };
  console.log("FAIL partial", Object.keys(o).length);
} catch (e) {
  console.log("partial:", e.message);
}

// 属性值求值抛异常 → 传播（静态键快路径与含 spread 的通用路径）
try {
  const o = { a: boom() };
  console.log("FAIL static-key value", o.a);
} catch (e) {
  console.log("static-key value:", e.message);
}
try {
  const o = { ...{ x: 1 }, a: boom() };
  console.log("FAIL generic value", o.a);
} catch (e) {
  console.log("generic value:", e.message);
}

// 求值顺序：抛错定义之前的属性恰好求值一次，之后的不再求值
const order = [];
function rec(x) {
  order.push(x);
  return x;
}
try {
  const o = { a: rec(1), ...boom(), b: rec(3) };
  console.log("FAIL order");
} catch (e) {
  order.push("caught");
}
console.log("order:", order.join(","));

// getter 抛错的求值顺序：之后的属性不再求值
try {
  const o = { a: rec(4), ...getterThrows, b: rec(6) };
  console.log("FAIL getter order");
} catch (e) {
  order.push("getter-caught");
}
console.log("order2:", order.join(","));

// 嵌套对象字面量中的 spread 异常同样传播
try {
  const o = { inner: { ...boom() } };
  console.log("FAIL nested");
} catch (e) {
  console.log("nested:", e.message);
}

// 函数体内（非顶层）同样传播
function build(src) {
  return { ...src, tag: "built" };
}
try {
  build(getterThrows);
  console.log("FAIL in function");
} catch (e) {
  console.log("in function:", e.message);
}

// 正常 spread 行为不受影响：null/undefined/原始值源跳过或按 ToObject 复制，
// 覆盖顺序保持从左到右
const base = { x: 1, y: 2 };
const merged = {
  w: 0,
  ...base,
  y: 9,
  ...null,
  ...undefined,
  ...42,
  ..."ab",
  z: 3,
};
console.log("ok:", JSON.stringify(merged), Object.keys({ ...{} }).length);
