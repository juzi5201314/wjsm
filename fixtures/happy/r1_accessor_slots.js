// accessor 属性占两个相邻值槽（getter@index、setter@index+1）：
// 与数据属性混排、经历 GC、以及数据↔accessor 互转都必须保持正确。
const o = {
  plain: 1,
  get doubled() {
    return this.plain * 2;
  },
  set doubled(v) {
    this.plain = v / 2;
  },
  tail: 9,
};
console.log(o.plain, o.doubled, o.tail);
o.doubled = 10;
console.log(o.plain, o.doubled, o.tail);
console.log(Object.keys(o).join(","));

// GC 之后 getter/setter 函数仍必须存活（它们是普通值槽里的句柄，须被 trace）。
const keep = [];
for (let i = 0; i < 20000; i++) {
  keep.push({ filler: i });
}
console.log(keep.length > 0, o.doubled, o.plain);
o.doubled = 42;
console.log(o.plain, o.doubled);

// 数据属性 → accessor：旧值槽被弃用，新的两个槽生效。
const conv = { v: 1 };
console.log(conv.v);
Object.defineProperty(conv, "v", {
  get() {
    return 77;
  },
  configurable: true,
});
console.log(conv.v);
// accessor → 数据属性。
Object.defineProperty(conv, "v", { value: 5, writable: true, configurable: true });
console.log(conv.v);
conv.v = 6;
console.log(conv.v);

// 描述符可见性。
const d = Object.getOwnPropertyDescriptor(o, "doubled");
console.log(typeof d.get, typeof d.set, d.value === undefined);
const d2 = Object.getOwnPropertyDescriptor(o, "plain");
console.log(d2.value, d2.writable, d2.get === undefined);
