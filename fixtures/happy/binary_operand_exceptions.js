// Binary 运算的操作数异常传播（ES §13.15.4 EvaluateStringOrNumericBinaryExpression）：
// `? GetValue(lref)` 抛出先传播并短路 RHS 求值；`? GetValue(rval)` 抛出先于
// ApplyStringOrNumericBinaryOperator 传播——异常哨兵不得被字符串拼接或数值
// 转换当普通值吞掉。本文件只覆盖算术/位运算/求值顺序；比较与 ToPrimitive
// 场景在 binary_operand_exceptions_compare.js，generator/async 场景在
// binary_operand_exceptions_async.js（顶层函数过大时后端编译超线性变慢，
// 拆分以满足默认 profile 的 30s 挂起门禁）。

function caught(label, e) {
  console.log(label + " " + e.constructor.name + " | " + e.message);
}

function throwError(message) {
  throw new Error(message);
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
