// ToPropertyKey（§7.1.19）用户代码再入：成员读写、对象字面量计算键、
// delete、可选链、解构等路径的对象键必须经 ToPrimitive(string) 调用用户
// `toString` / `valueOf` / `Symbol.toPrimitive`，转换异常必须传播。
// 期望输出与 Node 一致。

const log = [];

// ── 1. 成员写 + 成员读：各调用一次 toString ──
{
  log.length = 0;
  const o = {};
  const k = { toString() { log.push("ts"); return "K"; } };
  o[k] = 42;
  log.push(String(o.K));
  log.push(String(o[k]));
  console.log(log.join(","));
}

// ── 2. 对象字面量计算键：键转换先于属性值求值 ──
{
  log.length = 0;
  const k = { toString() { log.push("key"); return "K"; } };
  const o = { [k]: (log.push("value"), 7) };
  log.push(String(o.K));
  console.log(log.join(","));
}

// ── 3. 数组键经 Array.prototype.toString → join ──
{
  const o = {};
  o[[1, 2]] = "arr";
  console.log(Object.keys(o).join("|"), o["1,2"]);
}

// ── 4. Symbol.toPrimitive：hint 为 string，返回 symbol 时保留 symbol 键 ──
{
  log.length = 0;
  const s = Symbol("s");
  const k = { [Symbol.toPrimitive](hint) { log.push("prim:" + hint); return s; } };
  const o = {};
  o[k] = "sym";
  log.push(String(o[s]));
  log.push(String(Object.getOwnPropertySymbols(o).length));
  console.log(log.join(","));
}

// ── 5. toString 不可调用时回退 valueOf；数字结果按 ToString 成键 ──
{
  const o = {};
  o[{ valueOf() { return 9; }, toString: null }] = "v9";
  console.log(Object.keys(o).join("|"), o["9"]);
}

// ── 6. toString 返回对象时回退 valueOf；两者皆对象则 TypeError ──
{
  log.length = 0;
  const o = {};
  o[{ toString() { log.push("obj-ts"); return {}; }, valueOf() { log.push("vo"); return "F"; } }] = 1;
  log.push(String(o.F));
  try {
    o[{ toString() { return {}; }, valueOf() { return {}; } }] = 2;
    log.push("unreachable");
  } catch (e) {
    log.push("TypeError:" + (e instanceof TypeError));
  }
  console.log(log.join(","));
}

// ── 7. 转换抛异常：读与写均传播，不产生 "[object Object]" 键 ──
{
  log.length = 0;
  const o = {};
  const boom = { toString() { throw new Error("boom"); } };
  try { o[boom]; } catch (e) { log.push("read:" + e.message); }
  try { o[boom] = 1; } catch (e) { log.push("write:" + e.message); }
  try { const p = { [boom]: (log.push("value must not run"), 1) }; } catch (e) { log.push("literal:" + e.message); }
  log.push(String(Object.keys(o).length));
  console.log(log.join(","));
}

// ── 8. 复合赋值与自增：GetValue / PutValue 各转换一次 ──
{
  log.length = 0;
  const o = { C: 1 };
  const k = { toString() { log.push("c"); return "C"; } };
  o[k] += 1;
  log.push(String(o.C));
  o[k]++;
  log.push(String(o.C));
  console.log(log.join(","));
}

// ── 9. delete 转换一次；可选链短路时不转换 ──
{
  log.length = 0;
  const o = { D: 1 };
  const k = { toString() { log.push("d"); return "D"; } };
  delete o[k];
  log.push(String(o.D));
  const u = undefined;
  log.push(String(u?.[k]));
  log.push(String(o?.[k]));
  console.log(log.join(","));
}

// ── 10. 解构计算键与数字键索引快路径 ──
{
  log.length = 0;
  const k = { toString() { log.push("x"); return "X"; } };
  const { [k]: x } = { X: "destructured" };
  log.push(x);
  const arr = ["a", "b", "c"];
  log.push(arr[{ toString() { log.push("idx"); return 1; } }]);
  console.log(log.join(","));
}

// ── 11. 正则与包装对象键 ──
{
  const o = {};
  o[/re/] = "regexp";
  o[new String("boxed")] = "wrapper";
  console.log(o["/re/"], o["boxed"]);
}

// ── 12. proxy trap 接收已转换的属性键 ──
{
  log.length = 0;
  const target = {};
  const p = new Proxy(target, {
    get(t, prop, r) { log.push("get:" + String(prop)); return Reflect.get(t, prop, r); },
    set(t, prop, v, r) { log.push("set:" + String(prop)); return Reflect.set(t, prop, v, r); },
    deleteProperty(t, prop) { log.push("del:" + String(prop)); return Reflect.deleteProperty(t, prop); },
  });
  const k = { toString() { return "P"; } };
  p[k] = 1;
  log.push(String(p[k]));
  delete p[k];
  log.push(String(target.P));
  console.log(log.join(","));
}
