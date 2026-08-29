// ArrayBuffer transfer / transferToFixedLength（ES2024 §25.1.6.6–.7，
// ArrayBufferCopyAndDetach §25.1.3.2）：字节转移到新 buffer（grow 补零 /
// shrink 截断），源 detach；transfer 保留 resizability，
// transferToFixedLength 收敛固定长度。detached 语义：byteLength 读 0、
// 视图读 undefined、再操作按 V8 文案 TypeError。stdout 与 Node v22 逐字节一致。

// 基本转移：缺省长度取当前 byteLength，源立即 detach。
const src = new ArrayBuffer(4);
new Uint8Array(src)[0] = 7;
const moved = src.transfer();
console.log(moved.byteLength, new Uint8Array(moved)[0], src.detached, moved.detached, src.byteLength);

// grow / shrink：新长度补零或截断。
const grow = new ArrayBuffer(2);
new Uint8Array(grow)[1] = 3;
const grown = grow.transfer(6);
console.log(grown.byteLength, new Uint8Array(grown)[1], new Uint8Array(grown)[5]);
console.log(new ArrayBuffer(8).transfer(4).byteLength);

// resizability 保持 / 收敛：transfer 继承 maxByteLength，超出即 RangeError；
// transferToFixedLength 结果不可 resize。
const rab = new ArrayBuffer(4, { maxByteLength: 16 });
const kept = rab.transfer(8);
console.log(kept.resizable, kept.maxByteLength, kept.byteLength, rab.detached);
const flat = kept.transferToFixedLength(6);
console.log(flat.resizable, flat.maxByteLength, flat.byteLength, kept.detached);
try { new ArrayBuffer(4, { maxByteLength: 8 }).transfer(64); } catch (e) { console.log(e.constructor.name, e.message); }
try { new ArrayBuffer(4).transfer(-1); } catch (e) { console.log(e.constructor.name, e.message); }

// detached buffer 的观察面：maxByteLength 读 0 但 resizable 保持，视图长度
// 塌缩为 0、元素读 undefined、写静默；方法与 getter 按 V8 文案。
const det = new ArrayBuffer(6, { maxByteLength: 12 });
const view = new Uint8Array(det);
det.transfer();
console.log(det.maxByteLength, det.resizable, det.detached);
console.log(view.length, view.byteLength, String(view[0]));
view[0] = 1;
console.log(String(view[0]));
try { det.transfer(); } catch (e) { console.log(e.constructor.name, e.message); }
try { det.resize(4); } catch (e) { console.log(e.constructor.name, e.message); }
try { det.slice(0); } catch (e) { console.log(e.constructor.name, e.message); }
try { view.join(","); } catch (e) { console.log(e.constructor.name, e.message); }
try { new Uint8Array(det); } catch (e) { console.log(e.constructor.name, e.message); }
try { new DataView(det); } catch (e) { console.log(e.constructor.name, e.message); }
try { ArrayBuffer.prototype.transfer.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
try { ArrayBuffer.prototype.transfer.call(new SharedArrayBuffer(8)); } catch (e) { console.log(e.constructor.name, e.message); }

// structuredClone：resizable buffer 克隆保持 maxByteLength；transfer 选项
// 转移字节并 detach 源；detached buffer 不可克隆。
const cloneSrc = new ArrayBuffer(4, { maxByteLength: 16 });
const clone = structuredClone(cloneSrc);
console.log(clone.resizable, clone.maxByteLength, clone.byteLength);
const transferSrc = new ArrayBuffer(4);
new Uint8Array(transferSrc)[2] = 9;
const transferred = structuredClone(transferSrc, { transfer: [transferSrc] });
console.log(transferred.byteLength, new Uint8Array(transferred)[2], transferSrc.detached);
try { structuredClone(det); } catch (e) { console.log(e.name); }
