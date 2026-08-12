let {a, ...rest} = {a: 1, b: 2, c: 3};
console.log(rest.b);
console.log(rest.c);

// CopyDataProperties 必须先排除已绑定键，再读取剩余属性。
let calls = [];
let source = {
  get a() {
    calls.push("a");
    return 1;
  },
  get b() {
    calls.push("b");
    return 2;
  },
};
let {a: first, ...getterRest} = source;
console.log(first, getterRest.b, calls.join(","));

// 只复制 enumerable own properties。
let hidden = {};
Object.defineProperty(hidden, "hidden", {value: 1, enumerable: false});
hidden.visible = 2;
let {...visibleRest} = hidden;
console.log(Object.keys(visibleRest).join(","), visibleRest.hidden);

// 排除键与复制键都保留 symbol identity。
let skipped = Symbol("skipped");
let kept = Symbol("kept");
let symbols = {[skipped]: 3, [kept]: 4, x: 5};
let {[skipped]: picked, ...symbolRest} = symbols;
console.log(picked, symbolRest[kept], Object.getOwnPropertySymbols(symbolRest).length);

// Proxy 必须依次观察 ownKeys、descriptor 与未排除键的 Get。
let trapLog = [];
let proxy = new Proxy(
  {a: 1, b: 2},
  {
    ownKeys(target) {
      trapLog.push("keys");
      return Reflect.ownKeys(target);
    },
    getOwnPropertyDescriptor(target, key) {
      trapLog.push("desc:" + key);
      return Object.getOwnPropertyDescriptor(target, key);
    },
    get(target, key, receiver) {
      trapLog.push("get:" + key);
      return Reflect.get(target, key, receiver);
    },
  },
);
let {a: proxyA, ...proxyRest} = proxy;
console.log(proxyA, proxyRest.b, trapLog.join(","));
