// 微任务 then 回调为 Proxy：须走 resolve_callback_target（handler.apply trap）
let applied = false;
const handler = {
  apply(_t, _thisArg, args) {
    applied = true;
    return args[0] + 1;
  },
};
const p = new Proxy({}, handler);
Promise.resolve(41).then(p).then((v) => {
  console.log("applied=" + applied + " v=" + v);
});