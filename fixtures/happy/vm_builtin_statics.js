// context 上的 Object / Promise 静态方法可获取且可调用
const vm = require("vm");
const ctx = vm.createContext({});

console.log("typeof_keys", typeof ctx.Object.keys);
console.log("typeof_resolve", typeof ctx.Promise.resolve);
console.log("keys", ctx.Object.keys({ a: 1, b: 2 }).join(","));
console.log(
  "inside",
  vm.runInContext(
    "typeof Object.keys + '|' + Object.keys({ x: 1 }).join(',') + '|' + typeof Promise.resolve",
    ctx
  )
);
