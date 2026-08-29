// IsConstructor（§7.2.4）对 native callable 家族必须穷举分类：Web API
// 原型方法与访问器（TextDecoder.prototype.decode / ReadableStream 系 /
// EventTarget 系）无 [[Construct]]，作 Reflect.construct 的 newTarget、
// new、bound、proxy 目标一律 TypeError；构造器本体（TextDecoder /
// EventTarget / %TypedArray% / Iterator / Buffer / Timeout 等）与 Node
// 里以普通函数声明形态存在的 atob / btoa（保有 [[Construct]]）继续通过。
// 分类结果与 Node v22 逐行对拍。
function classify(label, value) {
  try {
    Reflect.construct(function () {}, [], value);
    console.log(label, "constructor");
  } catch (e) {
    console.log(label, e instanceof TypeError ? "TypeError" : "unexpected");
  }
}
// 普通方法 / 访问器 getter：无 [[Construct]]，newTarget 校验拒绝。
classify("TextDecoder.prototype.decode", TextDecoder.prototype.decode);
classify(
  "TextDecoder encoding getter",
  Object.getOwnPropertyDescriptor(TextDecoder.prototype, "encoding").get,
);
classify("TextEncoder#encode", new TextEncoder().encode);
classify("TextEncoder#encodeInto", new TextEncoder().encodeInto);
classify("ReadableStream.prototype.getReader", ReadableStream.prototype.getReader);
classify("WritableStream#getWriter", new WritableStream().getWriter);
classify("reader#read", new ReadableStream().getReader().read);
classify("EventTarget.prototype.addEventListener", EventTarget.prototype.addEventListener);
classify("EventTarget.prototype.removeEventListener", EventTarget.prototype.removeEventListener);
classify("EventTarget.prototype.dispatchEvent", EventTarget.prototype.dispatchEvent);
classify("Event#stopPropagation", new Event("x").stopPropagation);
classify("AbortSignal#throwIfAborted", new AbortController().signal.throwIfAborted);
// 真实构造器继续通过（含构造期自抛的抽象构造器）。
classify("TextDecoder", TextDecoder);
classify("TextEncoder", TextEncoder);
classify("atob", atob);
classify("btoa", btoa);
classify("EventTarget", EventTarget);
classify("Event", Event);
classify("ReadableStream", ReadableStream);
classify("Iterator", Iterator);
classify("%TypedArray%", Object.getPrototypeOf(Uint8Array));
classify("Buffer", Buffer);
classify("AggregateError", AggregateError);
const timeout = setTimeout(() => {}, 0);
classify("Timeout", timeout.constructor);
clearTimeout(timeout);
const immediate = setImmediate(() => {});
classify("Immediate", immediate.constructor);
clearImmediate(immediate);

// new / bound / proxy 路径的拒绝文案（callsite 渲染，与 Node 一致）。
const decode = TextDecoder.prototype.decode;
const getReader = ReadableStream.prototype.getReader;
const addEventListener = EventTarget.prototype.addEventListener;
function probe(label, fn) {
  try {
    fn();
    console.log(label, "ok");
  } catch (e) {
    console.log(label, e.constructor.name + ": " + e.message);
  }
}
probe("new decode", () => new decode());
probe("new getReader", () => new getReader());
probe("new addEventListener", () => new addEventListener());
probe("new bound decode", () => new (decode.bind(null))());
probe("new proxy decode", () => new (new Proxy(decode, {}))());
// extends 非构造器：实例化在 super 构造处拒绝（只对拍错误类型名；
// Node 在类定义期即抛且文案嵌入其内部实现源码，本实现当前在构造期
// 抛，均为 TypeError）。
try {
  class X extends decode {}
  new X();
  console.log("new (class extends decode)", "ok");
} catch (e) {
  console.log("new (class extends decode)", e.constructor.name);
}
// bound / proxy 包装真实构造器继续通过。
probe("new bound TextDecoder", () => new (TextDecoder.bind(null))().encoding);
probe("new proxy EventTarget", () => new (new Proxy(EventTarget, {}))());
probe("newTarget proxy TextDecoder", () =>
  Reflect.construct(function () {}, [], new Proxy(TextDecoder, {})),
);
// decode 的 name / length / NativeFunction 形态（callable 元数据）。
console.log(JSON.stringify(decode.name), decode.length);
