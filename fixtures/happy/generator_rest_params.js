// generator/async 函数族的 rest 形参：wrapper 在真实调用帧收集后经续体槽传入 body。
function* genRest(...rest) {
  yield rest.length;
  yield rest[0];
}
const it = genRest(10, 20);
console.log(it.next().value);
console.log(it.next().value);

function* genMixed(a, ...rest) {
  yield a + rest.length;
}
console.log(genMixed(100, 1, 2, 3).next().value);

async function asyncRest(...r) {
  return r.join(",");
}

async function* asyncGenRest(first, ...r) {
  yield `${first}:${r.length}`;
}

(async () => {
  console.log(await asyncRest(1, 2, 3));
  console.log((await asyncGenRest("x", 4, 5).next()).value);
})();
