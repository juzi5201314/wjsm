// Function 构造器的可捕获错误路径：语法错误（含注入防护）、严格模式
// 早错误、Symbol/toString 强制转换异常。输出与 Node 一致。

function classify(build) {
  try {
    build();
    return "no error";
  } catch (error) {
    return error.constructor.name;
  }
}

// 形参/函数体语法错误。
console.log(classify(() => new Function("return }{")));
console.log(classify(() => new Function("a b", "return 1")));
console.log(classify(() => new Function("a", "return (")));

// 注入防护：形参与函数体都不能逃出函数边界（规范的三次独立解析）。
console.log(classify(() => new Function("/*", "*/) {")));
console.log(classify(() => new Function("a", "}, function evil() {")));
console.log(classify(() => new Function("a", "} function evil() {")));
console.log(classify(() => new Function("a\n) {} function evil(", "return 1")));

// 函数体带 "use strict" 时的形参早错误。
console.log(classify(() => new Function("a", "a", "'use strict'; return a")));
console.log(classify(() => new Function("eval", "'use strict';")));
console.log(classify(() => new Function("arguments", "'use strict';")));
console.log(classify(() => new Function("a = 1", "'use strict'; return a")));

// 实参 ToString 强制转换：Symbol 抛 TypeError，toString 异常原样传播。
console.log(classify(() => new Function(Symbol("x"), "return 1")));
console.log(classify(() => new Function({ toString() { throw new RangeError("boom"); } })));
