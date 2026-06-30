// #166: Promise.race 应对 thenable 与非原生-promise 对象走 Promise.resolve(C, x) 语义
// thenable 元素需 adopt 其状态（而非当作立即值）；普通对象作为值 fulfill race。

// thenable fulfill — adopt 其状态
Promise.race([{ then: (r) => r("thenable-wins") }]).then((v) =>
  console.log("thenable-fulfill:", v)
);
// thenable reject — reject race
Promise.race([{ then: (_, j) => j("thenable-rej") }]).then(
  () => console.log("never"),
  (e) => console.log("thenable-reject:", e)
);
// 普通对象（非 thenable）作为值 fulfill race（首个元素即 settle）
Promise.race([{ a: 1 }, { then: (r) => r(2) }, 3]).then((v) =>
  console.log("plain-object:", JSON.stringify(v))
);
// thenable 与立即值竞争：立即值 42 同步 resolve，先于 thenable 的微任务
Promise.race([{ then: (r) => r(99) }, 42]).then((v) =>
  console.log("value-beats-thenable:", v)
);
