// map / filter / flatMap 结果写入协议：CreateDataPropertyOrThrow（§7.3.7）。

// species 构造出不可扩展对象 → 首个写入 TypeError（V8 文案）
{
  function Frozen() {
    return Object.freeze({});
  }
  const a = [1, 2, 3];
  a.constructor = { [Symbol.species]: Frozen };
  try {
    a.map(x => x);
    console.log("frozen no-throw");
  } catch (e) {
    console.log("frozen", e.constructor.name, e.message);
  }
}

// species 构造出 Proxy → 写入走 defineProperty trap，键序与值可观察
{
  const writes = [];
  function Trapped() {
    return new Proxy(
      {},
      {
        defineProperty(target, key, desc) {
          writes.push(key + "=" + desc.value + ":" + desc.writable + desc.enumerable + desc.configurable);
          return Reflect.defineProperty(target, key, desc);
        },
      }
    );
  }
  const a = [10, 20];
  a.constructor = { [Symbol.species]: Trapped };
  a.map(x => x + 1);
  a.filter(x => x > 10);
  a.flatMap(x => [x, x]);
  console.log("writes", JSON.stringify(writes));
}

// species 返回普通对象：map 不重设 length，元素为普通自有属性
{
  function Obj() {
    return { length: 99 };
  }
  const a = [1, 2];
  a.constructor = { [Symbol.species]: Obj };
  const r = a.map(x => x * 2);
  console.log("obj-result", r.length, r[0], r[1]);
  const f = a.flatMap(x => [x, x * 10]);
  console.log("obj-flat", f.length, f[0], f[1], f[2], f[3]);
}

// map 洞传播：跳过索引在子类结果上仍为洞
{
  class Holey extends Array {}
  const a = [1, , 3];
  a.constructor = { [Symbol.species]: Holey };
  const m = a.map(x => x);
  console.log("holes", m.length, 0 in m, 1 in m, 2 in m, m instanceof Holey);
}

// defineProperty trap 拒绝写入 → TypeError
{
  function Refuser() {
    return new Proxy(
      {},
      {
        defineProperty() {
          return false;
        },
      }
    );
  }
  const a = [1];
  a.constructor = { [Symbol.species]: Refuser };
  try {
    a.map(x => x);
    console.log("refuse no-throw");
  } catch (e) {
    console.log("refuse", e.constructor.name, e.message);
  }
}

// species 构造器返回带既有内容的数组：map 逐索引重定义覆盖
{
  function Prefilled(n) {
    return [9, 9, 9, 9];
  }
  const a = [1, 2];
  a.constructor = { [Symbol.species]: Prefilled };
  const r = a.map(x => x * 5);
  console.log("prefilled", JSON.stringify(r), r.length);
}
