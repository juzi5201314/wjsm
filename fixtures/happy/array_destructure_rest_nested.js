// BindingRestElement 目标为嵌套解构模式时必须真正初始化嵌套绑定
// （IteratorBindingInitialization：rest 收集完成后对 A 执行嵌套模式的
// BindingInitialization），而不是让绑定停留在 undefined/TDZ。
let [...[a1, a2, a3]] = [3, 4, 5];
console.log(a1, a2, a3);

var [...[b1, b2]] = [1, 2];
console.log(b1, b2);

const [...[c1, c2]] = [6, 7];
console.log(c1, c2);

let d1, d2;
[...[d1, d2]] = [8, 9];
console.log(d1, d2);

// rest 目标为对象模式：先收集成数组再做对象解构。
let [...{ length }] = [10, 11, 12];
console.log(length);

const [...{ 0: first, length: len }] = [13, 14];
console.log(first, len);

// rest 目标再嵌套 rest。
let [...[...nested]] = [15, 16];
console.log(nested.length, nested[0], nested[1]);

// 嵌套模式内的默认值与空位。
let [...[e1 = 21, , e3 = 23]] = [undefined, 22];
console.log(e1, e3);

// 函数形参 rest 的嵌套模式（走 CollectRestArgs 路径）。
function f(...[p1, p2]) {
  return p1 + p2;
}
console.log(f(30, 31));

// 前置元素 + 空位 + rest。
let [g1, , ...[g2, g3]] = [40, 41, 42, 43];
console.log(g1, g2, g3);
