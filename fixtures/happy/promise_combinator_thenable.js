// #139: Promise.all/race/allSettled/any 应对 thenable 元素走 Promise.resolve(C, x) 语义
Promise.all([{ then: (r) => r(42) }, Promise.resolve(7), 100]).then((a) => {
  console.log("all", a[0], a[1], a[2]);
});
Promise.race([{ then: (r) => r("first") }]).then((v) => console.log("race", v));
Promise.allSettled([{ then: (_, j) => j("bad") }]).then((a) => {
  console.log("allSettled", a[0].status, a[0].reason);
});
Promise.any([{ then: (_, j) => j("e1") }, { then: (r) => r("ok") }]).then((v) => {
  console.log("any", v);
});
