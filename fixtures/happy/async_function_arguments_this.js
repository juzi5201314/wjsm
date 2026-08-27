// async 函数族的 arguments 与 this：wrapper 侧保存进续体槽，body 恢复。
async function argLen() {
  return arguments.length;
}

const argExpr = async function () {
  return arguments[0] + arguments[1];
};

async function thisX() {
  return this.x;
}

async function thisAfterAwait() {
  await Promise.resolve();
  return this.x * 2;
}

async function* asyncGenArgs() {
  yield arguments.length;
  yield arguments[1];
}

(async () => {
  console.log(await argLen(1, 2, 3));
  console.log(await argExpr(40, 2));
  console.log(await thisX.call({ x: 5 }));
  console.log(await thisAfterAwait.call({ x: 6 }));
  const it = asyncGenArgs("a", "b");
  console.log((await it.next()).value);
  console.log((await it.next()).value);
})();
