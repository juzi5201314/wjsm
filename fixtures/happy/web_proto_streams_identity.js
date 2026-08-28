// Streams 家族方法安装在共享 prototype 上：实例间方法身份相同、借用按
// 实际 this 分派、品牌不符抛 TypeError（同步方法直接抛，promise 形态以
// rejected promise 交付）、描述符与 name/length 与 Node v22 逐字节对拍。

// --- ReadableStream：prototype 自有方法与 locked 访问器 ---
console.log(typeof ReadableStream.prototype.getReader, typeof ReadableStream.prototype.cancel);
const s1 = new ReadableStream({ start(c) { c.enqueue("one"); c.close(); } });
const s2 = new ReadableStream({ start(c) { c.enqueue("two"); c.close(); } });
console.log(s1.getReader === s2.getReader, s1.getReader === ReadableStream.prototype.getReader);
console.log(Object.hasOwn(s1, "getReader"), Object.hasOwn(ReadableStream.prototype, "getReader"));
for (const name of ["cancel", "getReader", "pipeThrough", "pipeTo"]) {
  const d = Object.getOwnPropertyDescriptor(ReadableStream.prototype, name);
  console.log(name, d.writable, d.enumerable, d.configurable, d.value.name, d.value.length);
}
const lockedDesc = Object.getOwnPropertyDescriptor(ReadableStream.prototype, "locked");
console.log(typeof lockedDesc.get, lockedDesc.set, lockedDesc.enumerable, lockedDesc.configurable);
console.log(lockedDesc.get.name, lockedDesc.get.length);

// --- 借用 getReader：锁定的是被借的 this ---
const reader = s1.getReader.call(s2);
console.log(s1.locked, s2.locked, lockedDesc.get.call(s2));
const first = await reader.read();
console.log(first.value, first.done);
reader.releaseLock();
console.log(s2.locked);

// --- 品牌不符：同步方法直接抛，promise 形态 rejected ---
try { ReadableStream.prototype.getReader.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
try { lockedDesc.get.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
await ReadableStream.prototype.cancel.call({}).then(
  () => console.log("unexpected"),
  (e) => console.log("cancel reject:", e.constructor.name, e.message),
);

// --- WritableStream：方法身份、借用与品牌失败 ---
console.log(typeof WritableStream.prototype.getWriter);
const w1 = new WritableStream({});
const w2 = new WritableStream({});
console.log(w1.getWriter === w2.getWriter, w1.getWriter === WritableStream.prototype.getWriter);
const writer = w1.getWriter.call(w2);
console.log(w1.locked, w2.locked);
writer.releaseLock();
const wLockedDesc = Object.getOwnPropertyDescriptor(WritableStream.prototype, "locked");
console.log(wLockedDesc.get.name, typeof wLockedDesc.set);
try { WritableStream.prototype.getWriter.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
await WritableStream.prototype.abort.call({}).then(
  () => console.log("unexpected"),
  (e) => console.log("abort reject:", e.constructor.name, e.message),
);

// --- TransformStream：readable/writable 访问器在 prototype 上 ---
const t1 = new TransformStream();
const t2 = new TransformStream();
const readableDesc = Object.getOwnPropertyDescriptor(TransformStream.prototype, "readable");
const writableDesc = Object.getOwnPropertyDescriptor(TransformStream.prototype, "writable");
console.log(typeof readableDesc.get, readableDesc.set, readableDesc.enumerable, readableDesc.configurable);
console.log(readableDesc.get.name, writableDesc.get.name);
console.log(readableDesc.get.call(t2) === t2.readable, readableDesc.get.call(t1) === t1.readable);
console.log(t1.readable instanceof ReadableStream, t1.writable instanceof WritableStream);
try { readableDesc.get.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
try { writableDesc.get.call(s1); } catch (e) { console.log(e.constructor.name, e.message); }

console.log("done streams proto identity");
