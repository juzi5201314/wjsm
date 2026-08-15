// accessor IC：普通 getter、closure getter、原型 accessor、shape 变化失效应走慢路径。
const plain = {
  base: 2,
  get doubled() {
    return this.base * 2;
  },
};
let total = 0;
for (let i = 0; i < 100; i++) total += plain.doubled;
console.log("plain-getter:", total);

// 闭包 getter 定义在原型上：holder 不是 receiver，缓存必须记录 holder 与世代。
const proto = {
  factor: 3,
  get tripled() {
    return this.base * this.factor;
  },
};
const child = Object.create(proto);
child.base = 4;
let protoTotal = 0;
for (let i = 0; i < 100; i++) protoTotal += child.tripled;
console.log("proto-getter:", protoTotal);

// 原型 getter 改为新实现：形状变化 → IC 失效 → 必须读到新值。
Object.defineProperty(proto, "tripled", {
  get() {
    return this.base + this.factor;
  },
  configurable: true,
});
let changedTotal = 0;
for (let i = 0; i < 100; i++) changedTotal += child.tripled;
console.log("changed-getter:", changedTotal);

// 自有 accessor 遮蔽原型 accessor：形状变化后走慢路径，语义必须正确。
Object.defineProperty(child, "tripled", {
  get() {
    return this.base * 10;
  },
  configurable: true,
});
console.log("shadow-getter:", child.tripled);

// delete 后回落到原型 getter：缓存失效并重新回填。
delete child.tripled;
console.log("unshadow-getter:", child.tripled);

// 非 callable getter 退化为 MEGAMORPHIC，仍按 [[Get]] 返回 undefined。
const bad = {};
Object.defineProperty(bad, "x", { get: undefined, configurable: true });
console.log("non-callable-getter:", bad.x);

// 字典 shape 的 accessor 仍落宿主，语义正确。
const dict = {};
Object.defineProperty(dict, "x", {
  get() {
    return 77;
  },
  configurable: true,
});
dict.extra = 1;
console.log("dict-accessor:", dict.x, dict.extra);
