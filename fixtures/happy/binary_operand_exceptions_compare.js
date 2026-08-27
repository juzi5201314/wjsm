// Binary 比较的操作数异常传播（ES §13.10 关系比较 / §13.11 相等比较）：
// 操作数求值异常不得被 IsLessThan / IsLooselyEqual 当普通值吞掉返回布尔值；
// 比较自身经 ToPrimitive 调用用户码抛出时同样传播。算术/位运算场景在
// binary_operand_exceptions.js，generator/async 场景在
// binary_operand_exceptions_async.js（拆分控制顶层函数编译耗时）。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

function throwError(message) {
  throw new Error(message);
}
function throwRange(message) {
  throw new RangeError(message);
}

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
