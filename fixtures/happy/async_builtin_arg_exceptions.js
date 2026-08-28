// async 状态机体内实参求值异常按 ES ArgumentListEvaluation 的 `? GetValue`
// 在语义层就地分叉传播：async / async generator 体内的表达式级异常分叉把
// TAG_EXCEPTION 路由到最近的本地 try/catch，未捕获时 reject 返回的 promise
// ——异常对象不得流入消费型宿主 builtin 被 render/转换/比较当普通值吞掉
// （此前 console.log 会打印 "[object Object]" 且 promise 误 resolve）。
// 用例串行 await 驱动，输出与 Node 逐行一致。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

function boom(message) {
  throw new Error(message);
}

function identity(value) {
  return value;
}

async function main() {
  // —— 主形态：console.log 实参抛出，本地 try/catch 捕获，不打印异常对象 ——
  await (async () => {
    try {
      console.log("log ok:", boom("log-arg"));
    } catch (e) {
      caught("log-arg", e);
    }
  })();

  // —— 无 try/catch：沿状态机以 promise rejection 传播 ——
  await (async () => {
    console.log("bare ok:", boom("bare-arg"));
  })().then(
    () => console.log("bare resolved"),
    (e) => caught("bare rejected", e),
  );

  // —— 覆盖各消费型宿主 builtin：数值、序列化、字符串、对象、数组 ——
  await (async () => {
    try {
      console.log("max ok:", Math.max(1, boom("math-max")));
    } catch (e) {
      caught("math-max", e);
    }
    try {
      console.log("json ok:", JSON.stringify(boom("json-stringify")));
    } catch (e) {
      caught("json-stringify", e);
    }
    try {
      console.log("fcc ok:", String.fromCharCode(boom("from-char-code")));
    } catch (e) {
      caught("from-char-code", e);
    }
    try {
      console.log("keys ok:", Object.keys(boom("object-keys")));
    } catch (e) {
      caught("object-keys", e);
    }
    try {
      console.log("includes ok:", [1, 2].includes(boom("array-includes")));
    } catch (e) {
      caught("array-includes", e);
    }
    try {
      console.log("parse ok:", parseInt(boom("parse-int"), 10));
    } catch (e) {
      caught("parse-int", e);
    }
    try {
      console.log("bool ok:", Boolean(boom("boolean")));
    } catch (e) {
      caught("boolean", e);
    }
  })();

  // —— 求值顺序：前序实参的副作用保留，第一个抛出的实参胜出 ——
  await (async () => {
    let ran = false;
    try {
      console.log("order ok:", ((ran = true), "first"), boom("order"), "last");
    } catch (e) {
      caught("order", e);
    }
    console.log("order ran: " + ran);
  })();

  // —— 间接链：哨兵穿过用户函数返回值，最终消费点捕获 ——
  await (async () => {
    try {
      console.log("chain ok:", identity(boom("chain")));
    } catch (e) {
      caught("chain", e);
    }
  })();

  // —— 比较/拼接在本地 try/catch 内可捕获（消费点透传） ——
  await (async () => {
    try {
      console.log("eq ok:", boom("eq") == 1);
    } catch (e) {
      caught("eq", e);
    }
    try {
      console.log("seq ok:", boom("seq") === 1);
    } catch (e) {
      caught("seq", e);
    }
    try {
      console.log("add ok:", "x: " + boom("add"));
    } catch (e) {
      caught("add", e);
    }
  })();

  // —— await 操作数抛出：本地 catch 不回归 ——
  await (async () => {
    try {
      await boom("await-operand");
    } catch (e) {
      caught("await-operand", e);
    }
  })();

  // —— sync generator 用户角色：next 实参求值异常先于恢复传播 ——
  await (async () => {
    function* counter() {
      const received = yield 1;
      yield received;
    }
    const generator = counter();
    console.log("gen first: " + generator.next().value);
    try {
      console.log("gen ok:", generator.next(boom("gen-next-arg")));
    } catch (e) {
      caught("gen-next-arg", e);
    }
  })();

  // —— async generator：yield / return 操作数抛出按规范 reject ——
  await (async () => {
    async function* yields() {
      yield 1;
      yield boom("agen-yield");
    }
    const iterator = yields();
    const first = await iterator.next();
    console.log("agen first: " + first.value + " " + first.done);
    try {
      await iterator.next();
      console.log("agen second resolved");
    } catch (e) {
      caught("agen-yield rejected", e);
    }
    const done = await iterator.next();
    console.log("agen done: " + done.value + " " + done.done);
  })();

  await (async () => {
    async function* returns() {
      return boom("agen-return");
    }
    try {
      await returns().next();
      console.log("agen return resolved");
    } catch (e) {
      caught("agen-return rejected", e);
    }
  })();

  // —— 正常路径不回归 ——
  await (async () => {
    console.log("plain ok: " + Math.max(1, 2) + " " + JSON.stringify([1]));
  })();
}

// —— 同步路径对照：表达式级分叉仍然本地捕获 ——
try {
  console.log("sync ok:", boom("sync-arg"));
} catch (e) {
  caught("sync-arg", e);
}

main().then(() => console.log("main done"));
