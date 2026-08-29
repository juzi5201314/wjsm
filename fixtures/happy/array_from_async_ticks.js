// Array.fromAsync 微任务 tick 序（对齐 V8 array-from-async.tq）：
// 首个 next / Get 同步执行；异步迭代器每元素 1 tick；同步可迭代经
// CreateAsyncFromSyncIterator 每元素 2 tick（unwrap + 续点）；array-like
// 每元素 1 tick；空 array-like 在调用当轮即解决。
function ticker(label, n) {
  let p = Promise.resolve();
  for (let i = 1; i <= n; i++) {
    const msg = label + " tick " + i;
    p = p.then(() => console.log(msg));
  }
}

// 场景 A：纯异步迭代器（next 返回已解决 promise）。
function scenarioA() {
  ticker("A", 6);
  const srcA = {
    [Symbol.asyncIterator]() {
      let i = 0;
      return { next() { console.log("A next", i); return Promise.resolve(i < 2 ? { done: false, value: "v" + i++ } : { done: true }); } };
    },
  };
  const done = Array.fromAsync(srcA).then((a) => console.log("A done", JSON.stringify(a)));
  console.log("A sync end");
  return done;
}

// 场景 B：同步可迭代（数组），每元素 2 tick。
function scenarioB() {
  ticker("B", 9);
  const done = Array.fromAsync([10, 20]).then((a) => console.log("B done", JSON.stringify(a)));
  console.log("B sync end");
  return done;
}

// 场景 C：array-like（每元素 1 tick）与空 array-like（当轮解决）。
function scenarioC() {
  ticker("C", 5);
  Array.fromAsync(5).then(() => console.log("C empty done"));
  const done = Array.fromAsync({ length: 2, 0: "a", 1: Promise.resolve("b") })
    .then((a) => console.log("C arrlike done", JSON.stringify(a)));
  console.log("C sync end");
  return done;
}

scenarioA().then(scenarioB).then(scenarioC);
