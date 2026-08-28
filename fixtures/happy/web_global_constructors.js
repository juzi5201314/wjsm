// fetch / Streams / AbortController 构造器是真实全局值：可取值、可 typeof、
// 可 instanceof，name/length 与 Node v22 一致。输出与 Node v22 逐字节对拍。
const names = [
  "fetch",
  "Headers",
  "Request",
  "Response",
  "ReadableStream",
  "WritableStream",
  "TransformStream",
  "AbortController",
];
for (const name of names) {
  const value = globalThis[name];
  console.log(name, typeof value, value.name, value.length);
}

// 裸标识符取值与 globalThis 属性是同一函数身份
console.log(fetch === globalThis.fetch);
console.log(Headers === globalThis.Headers);
console.log(ReadableStream === globalThis.ReadableStream);
console.log(AbortController === globalThis.AbortController);

// 取值后再构造 / 调用
const H = Headers;
const h = new H([["x-a", "1"]]);
console.log(h.get("x-a"));
const RS = ReadableStream;
const rs = new RS();
console.log(typeof rs.getReader);

// instanceof：实例原型链挂接到 Constructor.prototype
console.log(new Headers() instanceof Headers);
console.log(new Request("https://example.com/") instanceof Request);
console.log(new Response("body") instanceof Response);
console.log(new ReadableStream() instanceof ReadableStream);
console.log(new WritableStream() instanceof WritableStream);
console.log(new TransformStream() instanceof TransformStream);
console.log(new AbortController() instanceof AbortController);
console.log(new Headers() instanceof Request);

// prototype 身份
console.log(typeof Headers.prototype);
console.log(Object.getPrototypeOf(new Headers()) === Headers.prototype);
console.log(Headers.prototype.constructor === Headers);
console.log(Object.getPrototypeOf(new AbortController()) === AbortController.prototype);

// fetch 是普通函数：prototype 为 {constructor: fetch}，instanceof 返回 false
console.log(typeof fetch.prototype);
console.log(fetch.prototype.constructor === fetch);
console.log(({}) instanceof fetch);

// TransformStream 的两端也是真实实例
const ts = new TransformStream();
console.log(ts.readable instanceof ReadableStream);
console.log(ts.writable instanceof WritableStream);

// 取值后的 fetch 可调用；响应与 body 流可 instanceof
const localFetch = fetch;
const response = await localFetch("data:text/plain,hello");
console.log(response instanceof Response);
console.log(response.body instanceof ReadableStream);
console.log(await response.text());
