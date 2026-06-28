// 验证 Map/Set/Proxy 跨 GC 存活（#106 map_table/set_table 全量扫描 + #107 TAG_PROXY 追踪）
const m = new Map();
m.set("key", { val: 42 });
const s = new Set();
s.add({ data: "hello" });
const proxy = new Proxy({ base: 100 }, {});

// 触发多轮 GC
for (let i = 0; i < 5000; i++) { const tmp = { x: i }; }

// 验证侧表持有的对象未被回收
console.log(m.get("key").val);
for (const x of s) { console.log(x.data); }
console.log(proxy.base);
