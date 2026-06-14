// Safepoint 安全验收：WASM local 持有唯一引用的对象，alloc 触发 GC 后仍可用。
// obj 是唯一引用（不在 shadow stack 外），dummy 的 alloc 触发 proactive GC，
// obj 必须因 safepoint spill 存活，返回 obj.val 正确。
function hold() {
  let obj = { val: 42 };
  let dummy = { a: 1 }; // 触发 alloc，可能触发 proactive GC
  return obj.val; // obj 仍可用（spill 保护）
}
console.log(hold());
