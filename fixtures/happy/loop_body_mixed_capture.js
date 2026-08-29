// TASK-8 核心复现：闭包同时捕获外层稳定绑定与循环体内 const，
// 每轮迭代须捕获独立的 k 实例，而非共享 $shared_env 槽位的末值。
const fns = [];
let label = "L";
for (let i = 0; i < 3; i++) {
  const k = i;
  fns.push(() => console.log(label, k));
}
for (const f of fns) f();
// 外层 let 是活绑定：循环后重赋值，所有闭包见新值；k 仍是各轮快照。
label = "M";
fns[0]();
fns[2]();
