// 跨 suspend 存活的 SSA 临时值溢出：复合赋值、调用参数、模板串等在 await/yield
// 之前求值的中间结果，resume 后必须仍可用，且取 await 前的快照值（ES 求值顺序）。

class Counter {
  q() {
    return 2;
  }
  async sum() {
    let s = 0;
    for (let i = 0; i < 3; i++) {
      s += await Promise.resolve(this.q());
    }
    return s;
  }
}

async function compoundNoLoop() {
  let s = 40;
  s += await Promise.resolve(2);
  return s;
}

function add(a, b) {
  return a + b;
}

async function callArg(x) {
  return add(x * 2, await Promise.resolve(3));
}

async function templateJoin(x) {
  return `${x}-${await Promise.resolve("mid")}-${x + 1}`;
}

// 复合赋值读 await 前的旧值：闭包在 await 期间对 s 的写入不参与本次相加。
async function staleRead() {
  let s = 10;
  const bump = () => {
    s += 100;
  };
  s += await (async () => {
    bump();
    return 1;
  })();
  return s;
}

async function memberCompound() {
  const box = { p: 1 };
  box.p += await Promise.resolve(3);
  return box.p;
}

async function rejectInLoop() {
  let s = 0;
  try {
    for (let i = 0; i < 3; i++) {
      s += await (i === 2 ? Promise.reject(new Error("boom")) : Promise.resolve(2));
    }
  } catch (e) {
    return `${s}:${e.message}`;
  }
  return "unreachable";
}

function* syncGen() {
  let s = 1;
  s += yield 5;
  s += yield 6;
  return s;
}

async function* asyncGen() {
  let s = 1;
  s += yield 5;
  yield s;
}

async function main() {
  console.log(await new Counter().sum());
  console.log(await compoundNoLoop());
  console.log(await callArg(1));
  console.log(await templateJoin(7));
  console.log(await staleRead());
  console.log(await memberCompound());
  console.log(await rejectInLoop());

  const it = syncGen();
  console.log(it.next().value, it.next(10).value, it.next(20).value);

  const ag = asyncGen();
  console.log((await ag.next()).value, (await ag.next(10)).value);
}

main();
