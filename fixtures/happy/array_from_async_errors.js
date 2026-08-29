// Array.fromAsync 错误路径：闭包内异常一律拒绝结果 promise；可迭代路径
// 上的错误先 AsyncIteratorClose 再以原错误拒绝（Node/V8 形态）；array-like
// 路径无 close。各场景以 await 串行化。
const t = async (label, fn) => {
  try {
    console.log(label, "OK", JSON.stringify(await fn()));
  } catch (e) {
    console.log(label, "ERR", e.constructor.name, JSON.stringify(e.message));
  }
};

async function main() {
  // nullish 源：GetMethod(@@asyncIterator) 的属性读取错误。
  await t("null-src", () => Array.fromAsync(null));
  await t("undef-src", () => Array.fromAsync(undefined));
  // mapfn 非可调用：先于源检查（kCalledNonCallable 渲染）。
  await t("badmap-num", () => Array.fromAsync([1], 42));
  await t("badmap-null-src", () => Array.fromAsync(null, 42));
  await t("badmap-str", () => Array.fromAsync([1], "hi"));
  await t("badmap-obj", () => Array.fromAsync([1], {}));
  await t("badmap-arr", () => Array.fromAsync([1], []));
  await t("badmap-null", () => Array.fromAsync([1], null));
  // @@asyncIterator / @@iterator 非可调用。
  await t("nc-asynciter", () => Array.fromAsync({ [Symbol.asyncIterator]: 3 }));
  await t("nc-synciter", () => Array.fromAsync({ [Symbol.iterator]: "hi" }));
  // 迭代器方法返回非对象（V8 统一渲染 Symbol.iterator）。
  await t("nonobj-iter", () => Array.fromAsync({ [Symbol.asyncIterator]() { return 7; } }));
  await t("nonobj-synciter", () => Array.fromAsync({ [Symbol.iterator]() { return "x"; } }));
  // next 非可调用（Call 语义抛出）。
  await t("nc-next", () => Array.fromAsync({ [Symbol.asyncIterator]() { return { next: 9 }; } }));
  await t("nc-next-str", () => Array.fromAsync({ [Symbol.asyncIterator]() { return { next: "foo" }; } }));
  await t("nc-next-missing", () => Array.fromAsync({ [Symbol.asyncIterator]() { return {}; } }));
  // 异步路径 nextResult 非对象：V8 按方法名渲染的实现怪癖。
  await t("nonobj-result", () => Array.fromAsync({ [Symbol.asyncIterator]() { return { next() { return 5; } }; } }));
  await t("nonobj-result-p", () => Array.fromAsync({ [Symbol.asyncIterator]() { return { next() { return Promise.resolve(5); } }; } }));
  // 同步包裹路径 next 结果非对象：渲染实际值。
  await t("sync-nonobj", () => Array.fromAsync({ [Symbol.iterator]() { return { next() { return 5; } }; } }));
  // array-like：length / 元素 getter / mapfn 异常直接拒绝（无 close）。
  await t("len-throw", () => Array.fromAsync({ get length() { throw new Error("len boom"); } }));
  await t("elem-throw", () => Array.fromAsync({ length: 1, get 0() { throw new Error("get boom"); } }));
  await t("arrlike-map-throw", () => Array.fromAsync({ length: 1, 0: "x" }, () => { throw new Error("map boom"); }));
  await t("len-invalid", () => Array.fromAsync({ length: 9007199254740991 }));

  // 可迭代路径的 close 行为矩阵：错误发生时 return 被调用且原错误胜出。
  {
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { return Promise.resolve({ done: false, value: 1 }); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("map-throw-close", () => Array.fromAsync(it, () => { throw new Error("boom"); }));
    console.log("map-throw-close events", JSON.stringify(ev));
  }
  {
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { ev.push("next"); return Promise.reject(new Error("arej")); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("next-reject-close", () => Array.fromAsync(it));
    console.log("next-reject-close events", JSON.stringify(ev));
  }
  {
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { ev.push("next"); throw new Error("nextboom"); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("next-throw-close", () => Array.fromAsync(it));
    console.log("next-throw-close events", JSON.stringify(ev));
  }
  {
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { return Promise.resolve({ get done() { throw new Error("doneboom"); } }); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("done-throw-close", () => Array.fromAsync(it));
    console.log("done-throw-close events", JSON.stringify(ev));
  }
  {
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { return Promise.resolve({ done: false, get value() { throw new Error("valboom"); } }); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("value-throw-close", () => Array.fromAsync(it));
    console.log("value-throw-close events", JSON.stringify(ev));
  }
  {
    // 异步 mapfn 拒绝：close 后以原错误拒绝。
    const ev = [];
    const it = { [Symbol.asyncIterator]() { return { next() { return Promise.resolve({ done: false, value: 1 }); }, return() { ev.push("return"); return Promise.resolve({ done: true }); } }; } };
    await t("map-reject-close", () => Array.fromAsync(it, async () => { throw new Error("mrej"); }));
    console.log("map-reject-close events", JSON.stringify(ev));
  }
  {
    // 同步迭代器产 rejected promise：sync return 恰好调用一次。
    const ev = [];
    const it = { [Symbol.iterator]() { return { next() { ev.push("next"); return { done: false, value: Promise.reject(new Error("rej")) }; }, return() { ev.push("sync-return"); return { done: true }; } }; } };
    await t("sync-reject-close", () => Array.fromAsync(it));
    console.log("sync-reject-close events", JSON.stringify(ev));
  }
  {
    // return 缺失：直接以原错误拒绝。
    const it = { [Symbol.asyncIterator]() { return { next() { return Promise.reject(new Error("noret")); } }; } };
    await t("no-return", () => Array.fromAsync(it));
  }
}

main().then(() => console.log("done"));
