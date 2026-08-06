// 隐藏类共享：同结构字面量走同一条 transition 链，属性读写与插入序枚举必须全对。
const a = { x: 1, y: 2 };
const b = { x: 3, y: 4 };
console.log(a.x + a.y, b.x + b.y);
console.log(Object.keys(a).join(","), Object.keys(b).join(","));

// 插入序不同 → 不同 shape，但语义（值与枚举序）各自正确。
const xy = {};
xy.x = 10;
xy.y = 20;
const yx = {};
yx.y = 30;
yx.x = 40;
console.log(xy.x, xy.y, yx.x, yx.y);
console.log(Object.keys(xy).join(","), Object.keys(yx).join(","));

// 追加属性 → shape 前进；已有属性的值槽下标必须保持稳定。
a.z = 5;
console.log(a.x, a.y, a.z, Object.keys(a).join(","));
// b 不受 a 的 transition 影响。
console.log(b.z, Object.keys(b).join(","));

// 覆写不改变 shape。
a.x = 100;
console.log(a.x, a.y, a.z, Object.keys(a).length);

// 值槽扩容（超过初始容量 4）后旧属性仍可读。
const grow = { p0: 0 };
for (let i = 1; i < 12; i++) {
  grow["p" + i] = i;
}
console.log(grow.p0, grow.p5, grow.p11, Object.keys(grow).length);
