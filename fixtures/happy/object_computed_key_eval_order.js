// 对象字面量计算键按 PropertyDefinitionEvaluation 求值：
// 先求属性键，再求属性值；计算键抛异常必须传播且不得求属性值。
// 期望输出与 Node 一致。

const log = [];
function trace(tag, result) {
  log.push(tag);
  return result;
}

// ── 1. 单属性：键先于值求值 ──
{
  log.length = 0;
  const o = { [trace("key", "k")]: trace("value", 1) };
  console.log(log.join(","), o.k);
}

// ── 2. 多属性从左到右，逐属性键先值后 ──
{
  log.length = 0;
  const o = {
    [trace("k1", "a")]: trace("v1", 1),
    b: trace("v2", 2),
    [trace("k3", "c")]: trace("v3", 3),
  };
  console.log(log.join(","), o.a, o.b, o.c);
}

// ── 3. 计算键与 spread 交错保持从左到右 ──
{
  log.length = 0;
  const o = {
    [trace("k1", "a")]: trace("v1", 1),
    ...trace("spread", { s: 9 }),
    [trace("k2", "b")]: trace("v2", 2),
  };
  console.log(log.join(","), o.a, o.s, o.b);
}

// ── 4. 计算键抛异常：属性值不得求值 ──
{
  log.length = 0;
  function boomKey() {
    log.push("boomKey");
    throw new Error("key boom");
  }
  try {
    const o = { [boomKey()]: trace("value should not run", 1) };
    console.log("unreachable", o);
  } catch (e) {
    console.log(log.join(","), "caught:", e.message);
  }
}

// ── 5. 前序属性值抛异常：后续键不得求值 ──
{
  log.length = 0;
  function boomValue() {
    log.push("boomValue");
    throw new Error("value boom");
  }
  try {
    const o = { [trace("k1", "a")]: boomValue(), [trace("k2", "b")]: 2 };
    console.log("unreachable", o);
  } catch (e) {
    console.log(log.join(","), "caught:", e.message);
  }
}

// ── 6. 方法计算键抛异常：方法不定义，异常传播 ──
{
  log.length = 0;
  function methodKeyBoom() {
    log.push("methodKey");
    throw new Error("method key boom");
  }
  try {
    const o = {
      [methodKeyBoom()]() {
        return 1;
      },
    };
    console.log("unreachable", o);
  } catch (e) {
    console.log(log.join(","), "caught:", e.message);
  }
}

// ── 7. getter / setter 计算键抛异常 ──
{
  log.length = 0;
  function getterKeyBoom() {
    log.push("getterKey");
    throw new Error("getter key boom");
  }
  try {
    const o = {
      get [getterKeyBoom()]() {
        return 1;
      },
    };
    console.log("unreachable", o);
  } catch (e) {
    console.log(log.join(","), "caught:", e.message);
  }
  log.length = 0;
  function setterKeyBoom() {
    log.push("setterKey");
    throw new Error("setter key boom");
  }
  try {
    const o = {
      set [setterKeyBoom()](v) {},
    };
    console.log("unreachable", o);
  } catch (e) {
    console.log(log.join(","), "caught:", e.message);
  }
}

// ── 8. 方法 / 访问器计算键先于后续属性求值 ──
{
  log.length = 0;
  const o = {
    [trace("mkey", "m")]() {
      return "method";
    },
    get [trace("gkey", "g")]() {
      return "getter";
    },
    [trace("k", "p")]: trace("v", "plain"),
  };
  console.log(log.join(","), o.m(), o.g, o.p);
}

// ── 9. 计算键 ["__proto__"] 定义自有属性而非设置原型 ──
{
  const proto = { tag: "proto" };
  const viaComputed = { ["__proto__"]: proto };
  const viaStatic = { __proto__: proto };
  console.log(
    Object.getPrototypeOf(viaComputed) === proto,
    Object.getPrototypeOf(viaStatic) === proto,
  );
}

// ── 10. 数字与表达式计算键正常取值 ──
{
  log.length = 0;
  const n = 1;
  const o = { [n + 1]: trace("v", "two"), [`x${n}`]: "tpl" };
  console.log(log.join(","), o[2], o.x1);
}
