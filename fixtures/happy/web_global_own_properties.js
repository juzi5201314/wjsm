// Web 全局（fetch / Fetch 类 / Streams / Abort / Events）是全局对象上真实的
// 自有数据属性：own descriptor 可见、可枚举协议可见、赋值 / 删除 / 重定义
// 全按普通属性语义生效，裸标识符读取按全局环境记录语义解析（删除后
// ReferenceError）。特性与浏览器 / WebIDL 一致：{writable, enumerable:
// false, configurable}，fetch 方法额外 enumerable。Node v22 启动时把部分
// 名字装成惰性 accessor、首次读取后替换为同形数据属性，故本 fixture 先
// 读取每个名字再取描述符，stdout 与 Node v22 逐字节一致。
const names = [
  "fetch",
  "Headers",
  "Request",
  "Response",
  "ReadableStream",
  "WritableStream",
  "TransformStream",
  "AbortController",
  "AbortSignal",
  "EventTarget",
  "Event",
];

// own descriptor：全部为可写、可配置的函数值数据属性，仅 fetch 可枚举。
for (const name of names) {
  void globalThis[name];
  const d = Object.getOwnPropertyDescriptor(globalThis, name);
  console.log(name, typeof d.value, d.writable, d.enumerable, d.configurable);
}

// 自有键对枚举协议可见：getOwnPropertyNames / Reflect.ownKeys 含全部名字，
// keys / for-in / spread 只见 enumerable 的 fetch。
const ownNames = Object.getOwnPropertyNames(globalThis);
console.log(names.every((name) => ownNames.includes(name)));
console.log(Reflect.ownKeys(globalThis).includes("Event"));
console.log(Object.keys(globalThis).filter((name) => names.includes(name)).join(","));
const forIn = [];
for (const key in globalThis) if (names.includes(key)) forIn.push(key);
console.log(forIn.join(","));
console.log(Object.keys({ ...globalThis }).filter((name) => names.includes(name)).join(","));
console.log(globalThis.hasOwnProperty("Headers"), Object.hasOwn(globalThis, "fetch"));
console.log(globalThis.propertyIsEnumerable("fetch"), globalThis.propertyIsEnumerable("Headers"));
console.log("Headers" in Object.getOwnPropertyDescriptors(globalThis));

// 赋值：数据属性就地更新，特性保持 {writable, enumerable: false,
// configurable}；裸标识符与属性读取都见新值，new 回退通用构造语义。
const savedHeaders = Headers;
globalThis.Headers = 42;
const afterAssign = Object.getOwnPropertyDescriptor(globalThis, "Headers");
console.log(afterAssign.value, afterAssign.writable, afterAssign.enumerable, afterAssign.configurable);
console.log(Headers, globalThis.Headers, typeof Headers);
try {
  new Headers();
} catch (error) {
  // 非构造器文案两边渲染来源不同（V8 用表达式文本），只断言错误类别。
  console.log(error.constructor.name);
}
globalThis.Headers = savedHeaders;
console.log(Headers === savedHeaders, new Headers([["x-a", "1"]]).get("x-a"));

// Reflect.set：写入生效，new 用改写后的普通函数按通用构造装配 receiver。
console.log(Reflect.set(globalThis, "Request", function Patched() { this.tag = "patched"; }));
console.log(new Request("ignored").tag, Request.name);

// defineProperty 访问器：读取走 getter，裸标识符读取同样生效。
Object.defineProperty(globalThis, "Response", {
  get() { return "from-getter"; },
  configurable: true,
});
console.log(globalThis.Response, Response);

// defineProperty 部分描述符：既有可配置数据属性未指定的特性保持不变。
Object.defineProperty(globalThis, "TransformStream", { value: 7 });
const partial = Object.getOwnPropertyDescriptor(globalThis, "TransformStream");
console.log(partial.value, partial.writable, partial.enumerable, partial.configurable);

// defineProperty 换成自定义构造器：new 用新构造器。
Object.defineProperty(globalThis, "AbortController", {
  value: class Custom { constructor() { this.tag = "custom"; } },
  writable: true,
  configurable: true,
});
console.log(new AbortController().tag);

// 删除：可配置属性删除即消失——descriptor / in / hasOwnProperty 均不可见，
// typeof 容忍返回 undefined，裸标识符读取与 new 都抛 ReferenceError。
console.log(delete globalThis.WritableStream);
console.log(Object.getOwnPropertyDescriptor(globalThis, "WritableStream"));
console.log("WritableStream" in globalThis, globalThis.hasOwnProperty("WritableStream"));
console.log(typeof WritableStream, globalThis.WritableStream);
try {
  WritableStream;
} catch (error) {
  console.log(error.constructor.name, error.message);
}
try {
  new WritableStream();
} catch (error) {
  console.log(error.constructor.name, error.message);
}

// 删除 EventTarget / Event / AbortSignal：FIX-05 全局与八名单同一语义。
console.log(delete globalThis.EventTarget, typeof EventTarget);
try {
  new EventTarget();
} catch (error) {
  console.log(error.constructor.name, error.message);
}
console.log(delete globalThis.Event, typeof Event);
console.log(delete globalThis.AbortSignal, typeof AbortSignal, "AbortSignal" in globalThis);

// 删除 fetch：裸调用在实参求值前抛 ReferenceError；重新赋回后恢复。
const savedFetch = fetch;
console.log(delete globalThis.fetch, typeof fetch);
try {
  fetch("data:text/plain,x");
} catch (error) {
  console.log(error.constructor.name, error.message);
}
globalThis.fetch = savedFetch;
console.log(typeof fetch, fetch === savedFetch);

// 重新赋回构造器后恢复完整构造语义（身份与 instanceof 不变）。
globalThis.ReadableStream = ReadableStream;
console.log(new ReadableStream() instanceof ReadableStream);
