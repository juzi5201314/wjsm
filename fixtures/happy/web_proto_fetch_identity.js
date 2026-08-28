// fetch 家族方法安装在共享 prototype 上：实例间方法身份相同、借用按
// 实际 this 分派、品牌不符抛 TypeError（Web IDL brand check）、描述符与
// name/length 与 Node v22 逐字节对拍。AbortController 品牌失败只对拍
// 错误类型（Node 走 V8 私有字段报错，文案实现相关）。

// --- Headers：prototype 自有方法、身份、描述符 ---
console.log(typeof Headers.prototype.get, typeof Headers.prototype.set);
const ha = new Headers([["x-a", "1"]]);
const hb = new Headers([["x-a", "2"]]);
console.log(ha.get === hb.get, ha.get === Headers.prototype.get);
console.log(Object.hasOwn(ha, "get"), Object.hasOwn(Headers.prototype, "get"));
console.log(Headers.prototype.constructor === Headers);
for (const name of ["append", "delete", "get", "has", "set"]) {
  const d = Object.getOwnPropertyDescriptor(Headers.prototype, name);
  console.log(name, d.writable, d.enumerable, d.configurable, d.value.name, d.value.length);
}

// --- Headers：借用按 this 工作，提取后可继续用 ---
console.log(ha.get.call(hb, "x-a"));
const extractedSet = ha.set;
extractedSet.call(hb, "x-a", "3");
console.log(ha.get("x-a"), hb.get("x-a"));

// --- Headers：品牌不符（普通对象 / 跨品牌 / 裸调用）同步抛 ---
try { Headers.prototype.get.call({}, "x-a"); } catch (e) { console.log(e.constructor.name, e.message); }
try { Headers.prototype.get.call(new Request("https://x.example/"), "x-a"); } catch (e) { console.log(e.constructor.name, e.message); }
try { (0, ha.get)("x-a"); } catch (e) { console.log(e.constructor.name, e.message); }

// --- Request：只读访问器与方法在 prototype 上 ---
const ra = new Request("https://a.example/");
const rb = new Request("https://b.example/", { method: "POST" });
console.log(ra.clone === rb.clone, Object.hasOwn(Request.prototype, "clone"));
const urlDesc = Object.getOwnPropertyDescriptor(Request.prototype, "url");
console.log(typeof urlDesc.get, urlDesc.set, urlDesc.enumerable, urlDesc.configurable);
console.log(urlDesc.get.name, urlDesc.get.length);
console.log(urlDesc.get.call(rb));
const methodDesc = Object.getOwnPropertyDescriptor(Request.prototype, "method");
console.log(methodDesc.get.call(rb), methodDesc.get.name);
try { urlDesc.get.call({}); } catch (e) { console.log(e.constructor.name, e.message); }

// --- Response：方法借用与 promise 形态品牌失败（rejected，不同步抛） ---
const respA = new Response("aaa");
const respB = new Response("bbb");
console.log(respA.text === respB.text, respA.text === Response.prototype.text);
console.log(await respA.text.call(respB));
const statusDesc = Object.getOwnPropertyDescriptor(Response.prototype, "status");
console.log(statusDesc.get.name, statusDesc.get.call(respA));
await Response.prototype.text.call({}).then(
  () => console.log("unexpected"),
  (e) => console.log("text reject:", e.constructor.name, e.message),
);

// --- AbortController：signal 访问器 + abort 方法在 prototype 上 ---
console.log(typeof AbortController.prototype.abort);
const c1 = new AbortController();
const c2 = new AbortController();
console.log(c1.abort === c2.abort, c1.abort === AbortController.prototype.abort);
const abortDesc = Object.getOwnPropertyDescriptor(AbortController.prototype, "abort");
console.log(abortDesc.writable, abortDesc.enumerable, abortDesc.configurable, abortDesc.value.name);
const signalDesc = Object.getOwnPropertyDescriptor(AbortController.prototype, "signal");
console.log(typeof signalDesc.get, signalDesc.set, signalDesc.enumerable, signalDesc.configurable, signalDesc.get.name);
console.log(signalDesc.get.call(c2) === c2.signal);

// 借用 abort：只作用于被借的 this
c1.abort.call(c2, "why");
console.log(c1.signal.aborted, c2.signal.aborted, c2.signal.reason);

// 品牌不符只对拍错误类型（Node 文案为 V8 私有字段实现细节）
try { AbortController.prototype.abort.call({}); } catch (e) { console.log(e.constructor.name); }
try { signalDesc.get.call(new Headers()); } catch (e) { console.log(e.constructor.name); }

console.log("done fetch proto identity");
