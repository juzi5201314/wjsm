// 1. 双层 function 嵌套读取调用方绑定（W2-TASK-4 核心复现）
function t1() {
  let x = 1;
  return eval("function o(){ function i(){ return x; } return i(); } o()");
}
console.log("t1", t1());

// 2. 三层函数表达式嵌套 + 闭包逃逸 eval 后延迟调用（记录存活）
function t2() {
  let x = 2;
  const f = eval("(function(){ return function(){ return function(){ return x; }; }; })()()");
  return f();
}
console.log("t2", t2());

// 3. 嵌套闭包写调用方绑定（eval 生命周期内，回写可见）
function t3() {
  let x = 0;
  eval("function o(){ function i(){ x = 3; } i(); } o()");
  return x;
}
console.log("t3", t3());

// 4. 嵌套闭包 delete 调用方声明式绑定（sloppy，不可删返回 false）
function t4() {
  var x = 4;
  const deleted = eval("function o(){ function i(){ return delete x; } return i(); } o()");
  return [deleted, x];
}
console.log("t4", ...t4());

// 5. 箭头函数嵌套
function t5() {
  let x = 5;
  return eval("(() => (() => x)())()");
}
console.log("t5", t5());

// 6. 生成器声明嵌套（协程体经 $closure_env 接链）
function t6() {
  let x = 6;
  return eval("function o(){ function* g(){ yield x; } return g().next().value; } o()");
}
console.log("t6", t6());

// 7. 类方法内嵌套函数
function t7() {
  let x = 7;
  return eval("class C { m(){ return (function(){ return x; })(); } } new C().m()");
}
console.log("t7", t7());

// 8. 对象字面量方法内嵌套函数
function t8() {
  let x = 8;
  return eval("({ m(){ return (function(){ return x; })(); } }).m()");
}
console.log("t8", t8());

// 9. eval 内声明遮蔽调用方绑定（静态解析优先，不走桥）
function t9() {
  let x = 90;
  return eval("function o(){ let x = 9; function i(){ return x; } return i(); } o()");
}
console.log("t9", t9());

// 10. typeof 不可解析名经嵌套闭包（不抛 ReferenceError）
function t10() {
  return eval("function o(){ function i(){ return typeof nonexistent_xyz; } return i(); } o()");
}
console.log("t10", t10());

// 11. 逃逸闭包对（读者）：eval 返回后经写者闭包变更仍可见（记录存活 + 同链）
function t11() {
  let x = 11;
  const pair = eval("function o(){ return [function(){ return x; }, function(v){ x = v; }]; } o()");
  pair[1](111);
  return pair[0]();
}
console.log("t11", t11());

// 12. 匿名生成器表达式（内部临时名不外泄 + IR 延续块回归）
function t12() {
  let x = 12;
  const g = eval("(function*(){ yield (function(){ return x; })(); })");
  return g().next().value;
}
console.log("t12", t12());

// 13. async 函数体嵌套读取（微任务阶段读记录）
function t13() {
  let x = 13;
  return eval("function o(){ async function a(){ return x; } return a(); } o()");
}

// 14. 嵌套 direct eval：内层 eval 穿过嵌套函数与记录链到调用方
function t14() {
  let x = 14;
  return eval("function o(){ function i(){ return eval('x'); } return i(); } o()");
}
console.log("t14", t14());

// 15. 内层 eval 之后外层 eval 体继续读自由名（$eval_env 槽恢复）
function t15() {
  let x = 15;
  return eval("eval('1'); x");
}
console.log("t15", t15());

// 16. TDZ：嵌套闭包前向读 eval 内未初始化 let
function t16() {
  try {
    return eval("function o(){ function i(){ return z16; } const r = i(); let z16 = 1; return r; } o()");
  } catch (e) {
    return e.constructor.name;
  }
}
console.log("t16", t16());

// 17. 具名函数表达式内嵌套函数
function t17() {
  let x = 17;
  return eval("(function named(){ return (function(){ return x; })(); })()");
}
console.log("t17", t17());

// 18. 形参绑定穿透双层嵌套
function t18(p) {
  return eval("function o(){ function i(){ return p; } return i(); } o()");
}
console.log("t18", t18(18));

// 19. 对象方法（eval 内定义）延迟调用：方法闭包逃逸后仍接调用方链
function t19() {
  let x = 19;
  const obj = eval("({ m(){ return (function(){ return x; })(); } })");
  return obj.m();
}
console.log("t19", t19());

// 20. 双层嵌套写 const 调用方绑定：TypeError 文案裁决在记录
function t20() {
  const x = 20;
  try {
    eval("function o(){ function i(){ x = 1; } i(); } o()");
    return "no-throw";
  } catch (e) {
    return e.constructor.name;
  }
}
console.log("t20", t20());

t13().then(v => console.log("t13", v));
