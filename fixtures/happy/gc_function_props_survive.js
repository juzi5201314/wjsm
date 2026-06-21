// GC 后函数属性对象（.length / .name）与函数可调用性必须存活。
// startup snapshot 拆分 bootstrap 后，函数属性 handle 从 __function_props_base 起算
//（primordial 原型占据更低 handle）。GC root 若仍假设 0..num_ir_functions，会漏标
// 函数属性对象 → 被 sweep 回收 → .length/.name 读到 garbage、调用解析错位。
function alpha(a, b, c) {
  return a + b + c;
}
function beta(x) {
  return x * 2;
}

// 触发多轮 GC：分配远超 GC 阈值的临时对象（不保留引用 → 可回收）。
for (let i = 0; i < 200000; i++) {
  const tmp = { x: i, y: i + 1 };
}

// GC 后函数属性必须完好。
console.log(alpha.length, alpha.name);
console.log(beta.length, beta.name);
// GC 后直接调用（函数表索引重定位）必须正确。
console.log(alpha(1, 2, 3), beta(21));
