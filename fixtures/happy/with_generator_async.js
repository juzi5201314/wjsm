// 生成器 / async 状态机体内的 with：循环条件读 with 绑定跨 yield/await 恢复、
// 分派链异常（Proxy has trap / getter / @@unscopables getter）经 rejection 传播。
function* g() {
  with ({ n: 3 }) {
    for (let i = 0; i < n; i++) yield i;
  }
}
console.log([...g()].join(","));

async function a() {
  with ({ w: Promise.resolve("await-in-with") }) {
    return await w;
  }
}

async function loopSum() {
  with ({ n: 3 }) {
    let i = 0;
    let sum = 0;
    while (i < n) {
      sum += await Promise.resolve(i);
      i++;
    }
    return sum;
  }
}

async function trapRejects() {
  const p = new Proxy({}, { has() { throw new Error("has-trap"); } });
  with (p) {
    return x;
  }
}

(async () => {
  console.log(await a());
  console.log(await loopSum());
  try {
    await trapRejects();
  } catch (e) {
    console.log(e.message);
  }
  const getterThrow = { get bad() { throw new Error("getter"); } };
  try {
    with (getterThrow) { bad; }
  } catch (e) {
    console.log(e.message);
  }
  try {
    with (null) { }
  } catch (e) {
    console.log(e instanceof TypeError, "null-to-object");
  }
})();
