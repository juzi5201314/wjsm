// throw completion（yield 悬挂点 `.throw()` 与 throw 语句）按嵌套深度内层
// 优先交错展开迭代器保护区与 finally：嵌套 for-of 内外层依次先关再跑外层
// finally；循环体内 try/finally 先跑 finally 再关外层迭代器；close 抛出时
// 按 §7.4.11 步骤 5 吞咽、原始异常胜出；finally 自身抛出时新异常取代原异常
// 且迭代器不重复 close；路由到本地 catch 时同样交错且不越过 catch 外层的
// try-finally（栈索引边界）；finally 内 yield 时展开跨挂起续行。
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

// 场景1：嵌套 for-of + 外层 finally —— 内层先关、再关外层、再 finally
function* g1() {
  try {
    for (const a of makeIter("s1-outer")) {
      for (const b of makeIter("s1-inner")) {
        yield a * 10 + b;
      }
    }
  } finally {
    console.log("s1 finally");
  }
}
const i1 = g1();
console.log("s1", JSON.stringify(i1.next()));
try { i1.throw(new Error("s1 boom")); } catch (e) { console.log("s1 caught:", e.message); }

// 场景2：循环体内 try/finally —— finally 先于外层迭代器 close
function* g2() {
  for (const a of makeIter("s2-loop")) {
    try {
      yield a;
    } finally {
      console.log("s2 inner finally");
    }
  }
}
const i2 = g2();
console.log("s2", JSON.stringify(i2.next()));
try { i2.throw(new Error("s2 boom")); } catch (e) { console.log("s2 caught:", e.message); }

// 场景3：throw 语句（非 yield 悬挂点）从普通函数向外传播 —— close 先于 finally
function f3() {
  try {
    for (const x of makeIter("s3-iter")) {
      throw new Error("s3 boom");
    }
  } finally {
    console.log("s3 finally");
  }
}
try { f3(); } catch (e) { console.log("s3 caught:", e.message); }

// 场景4：close 抛出 —— throw completion 下吞咽，原始异常胜出，外层 finally 照跑
function* g4() {
  try {
    for (const x of {
      [Symbol.iterator]() {
        let i = 0;
        return {
          next() { i += 1; return { value: i, done: i > 3 }; },
          return() { console.log("s4 bad return called"); throw new Error("close boom"); },
        };
      },
    }) {
      yield x;
    }
  } finally {
    console.log("s4 finally");
  }
}
const i4 = g4();
console.log("s4", JSON.stringify(i4.next()));
try { i4.throw(new Error("s4 original")); } catch (e) { console.log("s4 caught:", e.message); }

// 场景5：finally 自身抛出 —— 新异常取代原异常，迭代器不重复 close
function* g5() {
  try {
    for (const x of makeIter("s5-iter")) {
      yield x;
    }
  } finally {
    console.log("s5 finally");
    throw new Error("s5 replaced");
  }
}
const i5 = g5();
console.log("s5", JSON.stringify(i5.next()));
try { i5.throw(new Error("s5 original")); } catch (e) { console.log("s5 caught:", e.message); }

// 场景6：throw 路由到本地 catch —— 内层 finally 先跑，再关迭代器，再进 catch
function f6() {
  try {
    for (const x of makeIter("s6-iter")) {
      try {
        throw new Error("s6 boom");
      } finally {
        console.log("s6 inner finally");
      }
    }
  } catch (e) {
    console.log("s6 caught:", e.message);
  }
}
f6();

// 场景7：catch 外层的 try-finally 不参与 throw 展开（finalizer 栈索引边界）
function f7() {
  try {
    try {
      for (const x of makeIter("s7-iter")) {
        throw new Error("s7 boom");
      }
    } catch (e) {
      console.log("s7 caught:", e.message);
    }
  } finally {
    console.log("s7 outer finally");
  }
}
f7();

// 场景8：catch 内 try { for-of } finally —— close 先于 finally 再进 catch
function f8() {
  try {
    try {
      for (const x of makeIter("s8-iter")) {
        throw new Error("s8 boom");
      }
    } finally {
      console.log("s8 finally");
    }
  } catch (e) {
    console.log("s8 caught:", e.message);
  }
}
f8();

// 场景9：finally 内 yield —— throw completion 穿越含挂起的 finalizer，
// 恢复后继续关闭外层迭代器并以原异常完成
function* g9() {
  for (const x of makeIter("s9-iter")) {
    try {
      yield x;
    } finally {
      console.log("s9 finally");
      yield "from-finally";
    }
  }
}
const i9 = g9();
console.log("s9", JSON.stringify(i9.next()));
try { console.log("s9", JSON.stringify(i9.throw(new Error("s9 boom")))); } catch (e) { console.log("s9 caught early:", e.message); }
try { console.log("s9", JSON.stringify(i9.next())); } catch (e) { console.log("s9 caught:", e.message); }
