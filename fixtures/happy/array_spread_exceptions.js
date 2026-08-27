// 数组/调用/new spread 的异常传播语义（ECMAScript ArrayAccumulation /
// ArgumentListEvaluation）：spread 源求值抛错、GetIterator 抛错、迭代器
// next()/done/value 抛错都必须传播，不得静默产生空数组或空实参。
function boom() {
  throw new Error("boom");
}

// spread 源调用抛异常 → 传播
try {
  const a = [...boom()];
  console.log("FAIL array literal", a.length);
} catch (e) {
  console.log("array literal:", e.message);
}

// 不可迭代值 → GetIterator 抛 TypeError
for (const value of [5, null, undefined, true]) {
  try {
    const a = [...value];
    console.log("FAIL non-iterable", a.length);
  } catch (e) {
    console.log("non-iterable:", e.constructor.name);
  }
}
try {
  const a = [...{ a: 1 }];
  console.log("FAIL plain object", a.length);
} catch (e) {
  console.log("plain object:", e.constructor.name);
}

// 求值顺序：抛错元素之前的元素恰好求值一次，之后的元素不再求值
const order = [];
function rec(x) {
  order.push(x);
  return x;
}
try {
  const a = [rec(1), ...boom(), rec(3)];
  console.log("FAIL order");
} catch (e) {
  order.push("caught");
}
console.log("order:", order.join(","));

// 非 spread 元素抛异常同样传播，异常值不得存入数组
try {
  const a = [rec(4), boom(), rec(6)];
  console.log("FAIL element throw");
} catch (e) {
  order.push("element-caught");
}
console.log("order2:", order.join(","));

// 自定义迭代器 next() 抛错：传播且不调用 return()（IteratorStepValue 无 close）
const nextThrows = {
  [Symbol.iterator]() {
    let n = 0;
    return {
      next() {
        n += 1;
        if (n === 2) throw new Error("next-throw");
        return { value: n, done: false };
      },
      return() {
        console.log("FAIL return() called");
        return { done: true };
      },
    };
  },
};
try {
  const a = [...nextThrows];
  console.log("FAIL next throw", a.length);
} catch (e) {
  console.log("next throw:", e.message);
}

// Symbol.iterator 属性 getter 抛错 → 传播
const iteratorGetterThrows = {
  get [Symbol.iterator]() {
    throw new Error("getter-throw");
  },
};
try {
  const a = [...iteratorGetterThrows];
  console.log("FAIL iterator getter", a.length);
} catch (e) {
  console.log("iterator getter:", e.message);
}

// Symbol.iterator 不可调用 → TypeError
const iteratorNotCallable = { [Symbol.iterator]: 42 };
try {
  const a = [...iteratorNotCallable];
  console.log("FAIL not callable", a.length);
} catch (e) {
  console.log("not callable:", e.constructor.name);
}

// Symbol.iterator 返回非对象 → TypeError
const iteratorNonObject = {
  [Symbol.iterator]() {
    return 7;
  },
};
try {
  const a = [...iteratorNonObject];
  console.log("FAIL non-object iterator", a.length);
} catch (e) {
  console.log("non-object iterator:", e.constructor.name);
}

// 调用参数 spread：源抛错传播，之后的实参不再求值
function collect(a, b) {
  return [a, b].join(",");
}
const callOrder = [];
function crec(x) {
  callOrder.push(x);
  return x;
}
try {
  collect(crec(1), ...boom(), crec(3));
  console.log("FAIL call spread");
} catch (e) {
  callOrder.push("caught");
}
console.log("call order:", callOrder.join(","));

// 宿主静态 API spread 与 new spread 同样传播
try {
  Math.max(...boom());
  console.log("FAIL Math.max spread");
} catch (e) {
  console.log("Math.max spread:", e.message);
}
class Pair {
  constructor(a, b) {
    this.value = [a, b].join(",");
  }
}
try {
  new Pair(...boom());
  console.log("FAIL new spread");
} catch (e) {
  console.log("new spread:", e.message);
}

// 正常 spread 行为不受影响（含空洞混合）
const holes = [, ...[1, 2], , 3];
console.log(
  "ok:",
  [...[1, 2], 3, ..."ab"].join(","),
  collect(...[7, 8]),
  new Pair(...[9, 10]).value,
  Math.max(...[1, 5, 3]),
  holes.length,
  holes[1]
);
