// generator `.return()` 的 return completion 按嵌套深度内层优先交错展开
// 迭代器保护区与 finally：嵌套 for-of 内层先关；循环体内 try/finally 先跑
// finally 再关外层迭代器；finally 内再次 yield 时展开跨挂起续行；close
// 抛出时 throw completion 取代 return completion（§7.4.11 步骤 5）；数组
// 解构默认值中的 yield 同样关闭解构迭代器（§8.6.2 保护区）。
function makeIter(tag) {
  return {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { i += 1; return { value: i, done: i > 3 }; },
        return() { console.log(tag + " closed"); return { done: true }; },
      };
    },
  };
}

// 场景1：嵌套 for-of —— 内层迭代器先关闭
function* g1() {
  for (const a of makeIter("outer")) {
    for (const b of makeIter("inner")) {
      yield a * 10 + b;
    }
  }
}
const i1 = g1();
console.log("s1", JSON.stringify(i1.next()));
console.log("s1", JSON.stringify(i1.return(7)));

// 场景2：循环体内 try/finally —— finally 先于外层迭代器 close
function* g2() {
  for (const a of makeIter("loop2")) {
    try {
      yield a;
    } finally {
      console.log("inner finally");
    }
  }
}
const i2 = g2();
console.log("s2", JSON.stringify(i2.next()));
console.log("s2", JSON.stringify(i2.return(8)));

// 场景3：finally 内再 yield —— return completion 穿越含挂起的 finalizer，
// 恢复后继续展开外层迭代器保护区并以原 completion 值完成。
function* g3() {
  for (const x of makeIter("g3-iter")) {
    try {
      yield x;
    } finally {
      console.log("g3 finally");
      yield "from-finally";
    }
  }
}
const i3 = g3();
console.log("s3", JSON.stringify(i3.next()));
console.log("s3", JSON.stringify(i3.return(13)));
console.log("s3", JSON.stringify(i3.next()));

// 场景4：close 抛出 —— throw completion 取代 return completion
function* g4() {
  for (const x of {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { i += 1; return { value: i, done: i > 3 }; },
        return() { console.log("bad return called"); throw new Error("close boom"); },
      };
    },
  }) {
    yield x;
  }
}
const i4 = g4();
console.log("s4", JSON.stringify(i4.next()));
try {
  i4.return(9);
} catch (e) {
  console.log("s4 caught:", e.message);
}
console.log("s4", JSON.stringify(i4.next()));

// 场景5：数组解构默认值中的 yield —— 解构迭代器保护区随 return 关闭
// （首个元素为 undefined 以触发默认值求值中的 yield 挂起）
function* g5() {
  const [a = yield "need-default", b] = {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { i += 1; return { value: i === 1 ? undefined : "v" + i, done: false }; },
        return(v) { console.log("destr closed, arg=" + String(v)); return { done: true }; },
      };
    },
  };
  console.log("unreachable", a, b);
}
const i5 = g5();
console.log("s5", JSON.stringify(i5.next()));
console.log("s5", JSON.stringify(i5.return(11)));
