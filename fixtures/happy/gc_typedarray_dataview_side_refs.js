// 验证 TypedArray/DataView 通过侧表保留 [[ViewedArrayBuffer]] 的 JS wrapper（#331）。
let buffer = new ArrayBuffer(8);
let typed = new Uint8Array(buffer);
let view = new DataView(buffer);
typed[0] = 77;
view.setUint8(1, 88);
buffer = null;
gc();
for (let i = 0; i < 5000; i++) { const tmp = { i }; }
console.log(typed[0]);
console.log(view.getUint8(1));
