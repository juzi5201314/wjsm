// EventTarget/Event/AbortSignal 参数校验与品牌检查的错误信息保真：
// 与 Node v22 的错误 name/message 逐字节对拍（异常全部捕获，happy 路径）。
const et = new EventTarget();
const probe = (fn) => {
  try {
    fn();
  } catch (e) {
    console.log(e.name + " | " + e.message);
  }
};
probe(() => et.addEventListener());
probe(() => et.addEventListener("x"));
probe(() => et.addEventListener("x", 42));
probe(() => et.addEventListener("x", "str"));
probe(() => et.addEventListener("x", () => {}, 42));
probe(() => et.addEventListener("x", () => {}, "str"));
probe(() => et.addEventListener(Symbol("s"), () => {}));
probe(() => et.removeEventListener("x"));
probe(() => et.removeEventListener("x", 42));
probe(() => et.dispatchEvent());
probe(() => et.dispatchEvent(undefined));
probe(() => et.dispatchEvent(null));
probe(() => et.dispatchEvent(42));
probe(() => et.dispatchEvent("x"));
probe(() => et.dispatchEvent({}));
probe(() => et.dispatchEvent(42n));
probe(() => et.dispatchEvent(true));
probe(() => et.dispatchEvent("x".repeat(29)));
probe(() => et.dispatchEvent(function foo() {}));
probe(() => new Event());
probe(() => new Event(Symbol("e")));
probe(() => new Event("x", 42));
probe(() => new Event("x", "str"));

// 品牌检查：this 不是对应接口实例时抛 TypeError
probe(() => et.addEventListener.call({}, "x", () => {}));
probe(() => et.dispatchEvent.call({}, new Event("x")));
probe(() => Object.getOwnPropertyDescriptor(Event.prototype, "type").get.call({}));
probe(() => Event.prototype.stopPropagation.call({}));
probe(() => Object.getOwnPropertyDescriptor(AbortSignal.prototype, "aborted").get.call({}));
probe(() => Object.getOwnPropertyDescriptor(AbortSignal.prototype, "reason").get.call({}));
probe(() => Object.getOwnPropertyDescriptor(AbortSignal.prototype, "onabort").get.call({}));
probe(() => Object.getOwnPropertyDescriptor(AbortSignal.prototype, "onabort").set.call({}, null));
probe(() => AbortSignal.prototype.throwIfAborted.call({}));

// 递归派发同一事件对象：ERR_EVENT_RECURSION
const recursive = new Event("r");
et.addEventListener("r", () => {
  try {
    et.dispatchEvent(recursive);
  } catch (e) {
    console.log("recursion: " + e.name + " | " + (e.code ?? "-") + " | " + e.message);
  }
});
et.dispatchEvent(recursive);

// throwIfAborted：未 abort 返回 undefined，abort 后抛出 reason
const controller = new AbortController();
console.log("tia ok:", controller.signal.throwIfAborted());
controller.abort("boom");
probe(() => controller.signal.throwIfAborted());
