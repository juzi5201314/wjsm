// async / async generator 状态机体内的表达式级异常分叉：属性写入（strict
// TypeError：冻结属性、getter-only、不可扩展）、抛出的 getter/setter、计算
// 键成员（SetElem）在语义层就地分叉——同步异常进入本地 try/catch，未捕获时
// reject 返回的 promise，不得被吞掉导致 promise 误 resolve。
// 用例串行 await 驱动，输出与 Node 逐行一致。
"use strict";

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

async function main() {
  // —— frozen 属性写入：本地 try/catch 捕获 ——
  await (async () => {
    const o = Object.freeze({ x: 1 });
    try {
      o.x = 2;
      console.log("frozen write resolved");
    } catch (e) {
      caught("frozen-write", e);
    }
    console.log("frozen x: " + o.x);
  })();

  // —— frozen 属性写入未捕获：promise reject，不误 resolve ——
  await (async () => {
    const o = Object.freeze({ x: 1 });
    o.x = 2;
    console.log("unreachable");
  })().then(
    () => console.log("bare frozen resolved"),
    (e) => caught("bare-frozen rejected", e),
  );

  // —— getter-only 属性写入 ——
  await (async () => {
    const o = { get x() { return 1; } };
    try {
      o.x = 2;
      console.log("getter-only write resolved");
    } catch (e) {
      caught("getter-only-write", e);
    }
  })();

  // —— 不可扩展对象新增属性 ——
  await (async () => {
    const o = Object.preventExtensions({});
    try {
      o.y = 1;
      console.log("non-extensible write resolved");
    } catch (e) {
      caught("non-extensible-write", e);
    }
  })();

  // —— 抛出的 setter：本地捕获与未捕获 reject ——
  await (async () => {
    const o = { set x(v) { throw new Error("setter boom " + v); } };
    try {
      o.x = 7;
      console.log("setter write resolved");
    } catch (e) {
      caught("throwing-setter", e);
    }
  })();
  await (async () => {
    const o = { set x(v) { throw new Error("setter bare " + v); } };
    o.x = 8;
    console.log("unreachable");
  })().then(
    () => console.log("bare setter resolved"),
    (e) => caught("bare-setter rejected", e),
  );

  // —— 抛出的 getter：读取位置本地捕获与未捕获 reject ——
  await (async () => {
    const o = { get x() { throw new Error("getter boom"); } };
    try {
      const v = o.x;
      console.log("getter read resolved: " + v);
    } catch (e) {
      caught("throwing-getter", e);
    }
  })();
  await (async () => {
    const o = { get x() { throw new Error("getter bare"); } };
    return o.x;
  })().then(
    (v) => console.log("bare getter resolved: " + v),
    (e) => caught("bare-getter rejected", e),
  );

  // —— 计算键成员（SetElem / GetElem 路径）——
  await (async () => {
    const arr = Object.freeze([1, 2, 3]);
    const i = 1;
    try {
      arr[i] = 9;
      console.log("frozen elem write resolved");
    } catch (e) {
      caught("frozen-elem-write", e);
    }
    console.log("frozen arr[1]: " + arr[i]);
  })();

  // —— 复合赋值经读取+写入：getter 抛出先于写入传播 ——
  await (async () => {
    const o = { get x() { throw new Error("compound getter"); }, set x(v) {} };
    try {
      o.x += 1;
      console.log("compound resolved");
    } catch (e) {
      caught("compound-getter", e);
    }
  })();

  // —— await 操作数是抛出的属性读取：先于挂起传播 ——
  await (async () => {
    const o = { get x() { throw new Error("await operand getter"); } };
    try {
      await o.x;
      console.log("await operand resolved");
    } catch (e) {
      caught("await-operand-getter", e);
    }
  })();

  // —— 跨 await 交错：resume 之后的表达式异常仍然本地可捕获 ——
  await (async () => {
    const o = Object.freeze({ x: 1 });
    let step = "before";
    try {
      await Promise.resolve(0);
      step = "after";
      o.x = 2;
      console.log("post-await write resolved");
    } catch (e) {
      caught("post-await-write(" + step + ")", e);
    }
  })();

  // —— async generator：体内属性写入异常按规范 reject next() 的 promise ——
  await (async () => {
    async function* gen() {
      const o = Object.freeze({ x: 1 });
      yield 1;
      o.x = 2;
      yield 2;
    }
    const it = gen();
    const first = await it.next();
    console.log("agen first: " + first.value + " " + first.done);
    try {
      await it.next();
      console.log("agen second resolved");
    } catch (e) {
      caught("agen-frozen rejected", e);
    }
    const done = await it.next();
    console.log("agen done: " + done.value + " " + done.done);
  })();

  // —— async generator：体内本地 try/catch 捕获后继续产出 ——
  await (async () => {
    async function* gen() {
      const o = Object.freeze({ x: 1 });
      try {
        o.x = 2;
      } catch (e) {
        yield "caught " + e.constructor.name;
      }
      yield "next";
    }
    const it = gen();
    console.log("agen catch: " + (await it.next()).value);
    console.log("agen continue: " + (await it.next()).value);
  })();

  // —— yield 操作数抛出：reject 且随后 done ——
  await (async () => {
    async function* gen() {
      const o = { get x() { throw new Error("yield operand"); } };
      yield o.x;
    }
    const it = gen();
    try {
      await it.next();
      console.log("agen yield resolved");
    } catch (e) {
      caught("agen-yield-operand rejected", e);
    }
  })();

  // —— 正常路径不回归 ——
  await (async () => {
    const o = { x: 1 };
    o.x = 2;
    o.y = 3;
    console.log("plain ok: " + o.x + " " + o.y);
  })();
}

main().then(() => console.log("main done"));
