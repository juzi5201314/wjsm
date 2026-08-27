// Binary 运算的操作数异常传播（ES §13.15.4 EvaluateStringOrNumericBinaryExpression /
// §13.10 关系比较 / §13.11 相等比较）：`? GetValue(lref)` 抛出先传播并短路 RHS
// 求值；`? GetValue(rval)` 抛出先于 ApplyStringOrNumericBinaryOperator /
// IsLessThan / IsLooselyEqual 传播——异常哨兵不得被字符串拼接、数值转换或
// 相等比较当普通值吞掉。松散/关系比较自身经 ToPrimitive 调用用户码抛出时
// 同样传播。generator 体内可本地捕获；async 体内沿状态机约定以 promise
// rejection 传播。用例写成顶层 try/catch 语句以保持编译耗时可控。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

function throwError(message) {
  throw new Error(message);
}
function throwRange(message) {
  throw new RangeError(message);
}

// —— 算术/字符串拼接：LHS/RHS 求值异常传播 ——
try { console.log("add-str ok: " + ("x: " + throwError("add-str"))); } catch (e) { caught("add-str", e); }
try { console.log("add-str-lhs ok: " + (throwError("add-str-lhs") + " :x")); } catch (e) { caught("add-str-lhs", e); }
try { console.log("add-num ok: " + (1 + throwError("add-num"))); } catch (e) { caught("add-num", e); }
try { console.log("sub ok: " + (throwError("sub") - 1)); } catch (e) { caught("sub", e); }
try { console.log("mul ok: " + (throwError("mul") * 2)); } catch (e) { caught("mul", e); }
try { console.log("div ok: " + (throwError("div") / 2)); } catch (e) { caught("div", e); }
try { console.log("mod ok: " + (throwError("mod") % 2)); } catch (e) { caught("mod", e); }
try { console.log("exp ok: " + (throwError("exp") ** 2)); } catch (e) { caught("exp", e); }

// —— 位运算：LHS/RHS 求值异常传播 ——
try { console.log("bitor ok: " + (throwError("bitor") | 1)); } catch (e) { caught("bitor", e); }
try { console.log("bitand ok: " + (1 & throwError("bitand"))); } catch (e) { caught("bitand", e); }
try { console.log("bitxor ok: " + (throwError("bitxor") ^ 1)); } catch (e) { caught("bitxor", e); }
try { console.log("shl ok: " + (throwError("shl") << 1)); } catch (e) { caught("shl", e); }
try { console.log("shr ok: " + (1 >> throwError("shr"))); } catch (e) { caught("shr", e); }
try { console.log("ushr ok: " + (throwError("ushr") >>> 1)); } catch (e) { caught("ushr", e); }

// —— 求值顺序：LHS 抛出短路 RHS 求值 ——
let addRhsRan = false;
try { console.log("add-order ok: " + (throwError("add-order") + ((addRhsRan = true), 1))); } catch (e) { caught("add-order", e); }
console.log("addRhsRan: " + addRhsRan);
let ltRhsRan = false;
try { console.log("lt-order ok: " + (throwError("lt-order") < ((ltRhsRan = true), 1))); } catch (e) { caught("lt-order", e); }
console.log("ltRhsRan: " + ltRhsRan);

// —— 相等/关系比较：操作数求值异常不得被吞掉返回布尔值 ——
try { console.log("eq ok: " + (throwError("eq") == 1)); } catch (e) { caught("eq", e); }
try { console.log("neq ok: " + (1 != throwError("neq"))); } catch (e) { caught("neq", e); }
try { console.log("seq ok: " + (throwError("seq") === 1)); } catch (e) { caught("seq", e); }
try { console.log("sneq ok: " + (1 !== throwError("sneq"))); } catch (e) { caught("sneq", e); }
try { console.log("lt ok: " + (throwError("lt") < 1)); } catch (e) { caught("lt", e); }
try { console.log("gt ok: " + (throwRange("gt") > "a")); } catch (e) { caught("gt", e); }
try { console.log("lteq ok: " + (throwError("lteq") <= 1)); } catch (e) { caught("lteq", e); }
try { console.log("gteq ok: " + (1 >= throwError("gteq"))); } catch (e) { caught("gteq", e); }

// —— 操作数为抛错 getter 的成员读取 ——
const badGetter = { get p() { throw new RangeError("getter-boom"); } };
try { console.log("getter-add ok: " + (badGetter.p + 1)); } catch (e) { caught("getter-add", e); }
try { console.log("getter-eq ok: " + (badGetter.p == 1)); } catch (e) { caught("getter-eq", e); }

// —— 比较自身的 ToPrimitive 调用用户码抛出（IsLessThan / IsLooselyEqual） ——
const badValueOf = { valueOf() { throw new RangeError("valueOf-boom"); } };
try { console.log("valueof-lt ok: " + (badValueOf < 1)); } catch (e) { caught("valueof-lt", e); }
try { console.log("valueof-eq ok: " + (badValueOf == 1)); } catch (e) { caught("valueof-eq", e); }
try { console.log("valueof-add ok: " + (badValueOf + 1)); } catch (e) { caught("valueof-add", e); }

// —— 正常路径不回归 ——
console.log("add ok: " + (1 + 2) + " " + ("a" + "b"));
console.log("arith ok: " + (7 % 3) + " " + (2 ** 10) + " " + (7 - 2) + " " + (3 * 4) + " " + (8 / 2));
console.log("bits ok: " + (5 | 2) + " " + (5 & 3) + " " + (5 ^ 1) + " " + (1 << 4) + " " + (-16 >> 2) + " " + (-1 >>> 28));
console.log("cmp ok: " + (1 == "1") + " " + (1 != 2) + " " + (1 === 1) + " " + (1 !== 1));
console.log("rel ok: " + (1 < 2) + " " + (2 > 1) + " " + (1 <= 1) + " " + (2 >= 3));
console.log("nan ok: " + (NaN < 1) + " " + (undefined <= 0));
console.log("bigint ok: " + (2n ** 10n) + " " + (5n % 3n) + " " + (1n < 2));

// —— generator 体内本地捕获（同款分叉在 sync generator 内可用） ——
function* gen() {
  try {
    yield "g: " + throwError("gen-boom");
  } catch (e) {
    yield "gen-caught: " + e.message;
  }
}
console.log(String(gen().next().value));

// —— async 体内：沿状态机约定，异常以 promise rejection 传播 ——
async function asyncAdd() {
  await Promise.resolve();
  return "x: " + throwError("async-add");
}
async function asyncEq() {
  await Promise.resolve();
  return throwError("async-eq") == 1;
}
async function asyncNeq() {
  await Promise.resolve();
  return throwError("async-neq") != 1;
}
async function asyncSeq() {
  await Promise.resolve();
  return throwError("async-seq") === 1;
}
async function asyncLt() {
  await Promise.resolve();
  return throwError("async-lt") < 1;
}
asyncAdd()
  .then(
    () => console.log("asyncAdd resolved"),
    (e) => caught("asyncAdd rejected:", e),
  )
  .then(() => asyncEq())
  .then(
    () => console.log("asyncEq resolved"),
    (e) => caught("asyncEq rejected:", e),
  )
  .then(() => asyncNeq())
  .then(
    () => console.log("asyncNeq resolved"),
    (e) => caught("asyncNeq rejected:", e),
  )
  .then(() => asyncSeq())
  .then(
    () => console.log("asyncSeq resolved"),
    (e) => caught("asyncSeq rejected:", e),
  )
  .then(() => asyncLt())
  .then(
    () => console.log("asyncLt resolved"),
    (e) => caught("asyncLt rejected:", e),
  );
