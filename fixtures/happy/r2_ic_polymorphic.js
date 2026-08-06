// inline cache 只缓存「shape_id → 值槽下标」，属性语义仍由宿主承担。
// 因此无论缓存多陈旧，读到的值都必须与无缓存时完全一致。

// 同一访问点先后喂不同 shape 的对象：每次 shape 变化都让缓存失效并重新回填。
function readX(o) { return o.x; }
const shapes = [
  { x: 1 },
  { x: 2, y: 9 },
  { a: 0, x: 3 },
  { x: 4, y: 1, z: 2 },
];
console.log(shapes.map(readX).join(","));

// 反复交替读同一访问点：缓存在两个 shape 间来回失效，结果必须始终正确。
let alt = 0;
for (let i = 0; i < 200; i++) {
  alt += readX(shapes[i % shapes.length]);
}
console.log("alt:", alt);

// 缓存命中后给对象加新属性（shape 前进），旧属性仍必须读对。
const grow = { x: 10 };
let before = 0;
for (let i = 0; i < 50; i++) before += grow.x;
grow.y = 20;
let after = 0;
for (let i = 0; i < 50; i++) after += grow.x;
console.log("grow:", before, after, grow.x, grow.y);

// 命中后 delete 使对象退化字典：读被删属性得 undefined，其余仍正确。
const del = { p: 1, q: 2 };
let sum = 0;
for (let i = 0; i < 50; i++) sum += del.p;
delete del.p;
console.log("delete:", sum, del.p, del.q);

// 覆写不改 shape：缓存持续命中，读到的必须是新值。
const over = { v: 1 };
let seen = "";
for (let i = 0; i < 5; i++) {
  over.v = i * 10;
  seen += over.v + ";";
}
console.log("overwrite:", seen);

// 非对象接收者走宿主完整语义（IC 不缓存）：字符串/数组/函数属性读。
console.log("str:", "abc".length, "arr:", [1, 2, 3].length);
function fn() {}
fn.tag = 7;
console.log("fn:", fn.tag, typeof fn.call);

// accessor 属性永不进缓存（需要调用 getter），值必须正确。
let calls = 0;
const acc = {
  get computed() {
    calls++;
    return 42;
  },
};
let accSum = 0;
for (let i = 0; i < 10; i++) accSum += acc.computed;
console.log("accessor:", accSum, calls);
