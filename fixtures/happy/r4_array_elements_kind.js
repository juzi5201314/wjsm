// 数组 ElementsKind：元素读的快路径只在「无洞、该索引无异质属性」时才直接读槽，
// 其余情形必须退回完整 [[Get]]。三条语义各自都曾是真实缺陷，此处逐一钉死。

// ── 1. PACKED：稠密数组直接读槽 ──
const packed = [1.5, 2.5, 3.5];
console.log("packed:", packed[0], packed[1], packed[2], packed.length);

// ── 2. HOLEY：跨越式写入产生洞，洞必须读作 undefined（不是 0）──
// 曾经的缺陷：新分配的槽字节为 0，而 0u64 解码为 +0.0，于是 a[1] 读出 0。
const holey = [1];
holey[3] = 4;
console.log("holey:", holey[0], holey[1], holey[2], holey[3], holey.length);
console.log("holey-in:", 0 in holey, 1 in holey, 3 in holey);

// 洞不参与遍历，但 length 覆盖整个区间。
console.log("holey-keys:", Object.keys(holey).join(","));

// ── 3. 越界读必须沿原型链查找 ──
// 曾经的缺陷：越界就地归一为 undefined，把原型上的索引属性整个吞掉。
Array.prototype[9] = 77;
console.log("oob-proto:", [1, 2][9]);
console.log("oob-plain:", [1, 2][5]);
// 洞位置同样是「缺失自有属性」，也要落原型链。
Array.prototype[2] = "from-proto";
const holeFallback = [0];
holeFallback[4] = 9;
console.log("hole-proto:", holeFallback[2]);
delete Array.prototype[2];
delete Array.prototype[9];

// ── 4. DICTIONARY：索引位置定义 accessor 后该索引必须调 getter ──
// 曾经的缺陷：元素读绕过侧表，getter 从不触发（读出原元素值）。
const withAccessor = [1, 2, 3];
let getterCalls = 0;
Object.defineProperty(withAccessor, "1", {
  get() {
    getterCalls++;
    return 99;
  },
  configurable: true,
});
// 只有下标 1 走 getter；其余下标仍读元素槽。
console.log("accessor:", withAccessor[0], withAccessor[1], withAccessor[2]);
console.log("accessor-calls:", getterCalls);

// 反复读：每次都必须触发 getter（不得被缓存成普通元素）。
let accSum = 0;
for (let i = 0; i < 10; i++) accSum += withAccessor[1];
console.log("accessor-loop:", accSum, getterCalls);

// ── 5. kind 只单向升级：升级后普通元素仍要正确 ──
const mixed = [10, 20, 30];
mixed[6] = 60; // → HOLEY
Object.defineProperty(mixed, "2", { get: () => 222, configurable: true }); // → DICTIONARY
console.log("mixed:", mixed[0], mixed[1], mixed[2], mixed[4], mixed[6], mixed.length);

// ── 6. 元素写入与读取往返 ──
const rw = [];
for (let i = 0; i < 8; i++) rw[i] = i * i;
console.log("roundtrip:", rw.join(","));
rw[3] = -1;
console.log("overwrite:", rw[3], rw[2], rw[4]);

// ── 7. push/pop 与洞共存 ──
const mix2 = [1];
mix2[3] = 3;
mix2.push(4);
console.log("push:", mix2.length, mix2[4], mix2[1]);
console.log("pop:", mix2.pop(), mix2.length);
