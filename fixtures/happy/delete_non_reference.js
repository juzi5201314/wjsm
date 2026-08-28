// §13.5.1.2 步骤 1–2：delete 的操作数不是 Reference Record 时，求值弃值后
// 恒返回 true——this、字面量、调用结果、void、逗号表达式、构造结果等均是。

// 基元字面量与 this / void。
console.log(delete 1, delete "s", delete null, delete void 0, delete this);
console.log(delete true, delete 1.5, delete 10n);

// 调用结果：操作数按序求值，副作用可见。
function f() {
  console.log("f evaluated");
  return 42;
}
console.log(delete f());

// 括号透传后仍非引用：算术、逗号表达式、数组/对象字面量、模板、typeof。
console.log(delete (1 + 2), delete (0, 1), delete [1, 2], delete { a: 1 });
console.log(delete `tpl`, delete typeof x, delete !0, delete new Object());

// 逗号表达式含属性读取：读取执行（GetValue），属性不受影响。
var o = { x: 1 };
console.log(delete (o.x, "value"), "x" in o);

// 赋值表达式非引用：赋值副作用发生后返回 true。
var target = { p: 0 };
console.log(delete (target.p = 7), target.p);

// 操作数求值抛出必须先传播，不得吞掉后返回 true。
try {
  delete (function () {
    throw new Error("operand boom");
  })();
  console.log("unreachable");
} catch (error) {
  console.log("caught:", error.message);
}

// delete 的结果本身可再被 delete（布尔值非引用）。
var dd = { y: 1 };
console.log(delete delete dd.y, "y" in dd);

// async 函数内 await 结果非引用：状态机续延后仍返回 true。
async function am() {
  console.log("await:", delete (await Promise.resolve(5)));
}
am();
