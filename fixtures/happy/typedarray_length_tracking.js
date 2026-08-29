// TypedArray / DataView length-tracking 视图（§23.2.5.1 / §25.3.2.1 的
// [[ArrayLength]] / [[ByteLength]] = auto）：length 实参缺省且 buffer 可变长
//（resizable ArrayBuffer / growable SharedArrayBuffer）时长度随底层重算；
// 固定长度视图在 shrink 越界后 getter 读 0、元素读 undefined、方法按
// ValidateTypedArray 抛 TypeError，regrow 回界后恢复（内容已补零）。
// stdout 与 Node v22 逐字节一致。

// tracking 视图随 resize 重算，元素读写跟随当前边界。
const rab = new ArrayBuffer(4, { maxByteLength: 16 });
const track = new Uint8Array(rab);
console.log(track.length, track.byteLength);
rab.resize(8);
console.log(track.length, track.byteLength);
track[6] = 5;
console.log(track[6], track[7], String(track[9]), 9 in track);
rab.resize(2);
console.log(track.length, String(track[6]));
rab.resize(8);
console.log(track.length, track[6]);

// 带 offset 的 tracking 视图：可见长度 = (bufLen - byteOffset) / 元素宽；
// shrink 到 offset 之下时 getter 全 0。
const off = new Uint16Array(rab, 4);
console.log(off.length, off.byteOffset, off.byteLength);
rab.resize(12);
console.log(off.length, off.byteLength);
rab.resize(2);
console.log(off.length, off.byteLength, off.byteOffset);
rab.resize(8);

// 固定长度视图 shrink 越界：getter 读 0、元素 undefined、方法 TypeError，
// regrow 恢复且内容补零。
const fixedView = new Uint8Array(rab, 0, 8);
fixedView[7] = 9;
rab.resize(4);
console.log(fixedView.length, fixedView.byteLength, String(fixedView[0]));
try { fixedView.join(","); } catch (e) { console.log(e.constructor.name, e.message); }
try { fixedView.at(0); } catch (e) { console.log(e.constructor.name, e.message); }
try { fixedView.fill(1); } catch (e) { console.log(e.constructor.name, e.message); }
try { fixedView.set([1]); } catch (e) { console.log(e.constructor.name, e.message); }
try { fixedView.toString(); } catch (e) { console.log(e.constructor.name, e.message); }
rab.resize(8);
console.log(fixedView.length, fixedView[7]);

// 迭代协议沿当前长度：toString / keys / values / entries / for-of。
const rab2 = new ArrayBuffer(2, { maxByteLength: 8 });
const it = new Uint8Array(rab2);
it[0] = 1;
it[1] = 2;
rab2.resize(4);
console.log(it.toString(), [...it.keys()].join("|"), Array.from(it.values()).join("|"));
console.log(JSON.stringify(Array.from(it.entries())));
const collected = [];
for (const x of it) collected.push(x);
console.log(collected.join(","));

// subarray：源为 tracking 且 end 缺省 → 结果仍 tracking；带 end → 固定。
const sub = it.subarray(1);
const subFixed = it.subarray(1, 3);
rab2.resize(8);
console.log(sub.length, sub.byteOffset, subFixed.length);

// DataView tracking：byteLength 重算，越界读 RangeError；固定 DataView
// shrink 越界后 byteLength / byteOffset / get 按 V8 文案 TypeError。
const rab3 = new ArrayBuffer(4, { maxByteLength: 16 });
const dv = new DataView(rab3);
console.log(dv.byteLength);
rab3.resize(8);
console.log(dv.byteLength);
dv.setUint8(6, 3);
console.log(dv.getUint8(6));
try { dv.getUint8(9); } catch (e) { console.log(e.constructor.name, e.message); }
const dvFixed = new DataView(rab3, 0, 8);
rab3.resize(4);
try { dvFixed.byteLength; } catch (e) { console.log(e.constructor.name, e.message); }
try { dvFixed.byteOffset; } catch (e) { console.log(e.constructor.name, e.message); }
try { dvFixed.getUint8(0); } catch (e) { console.log(e.constructor.name, e.message); }
console.log(dv.byteLength, dv.byteOffset);
rab3.resize(8);
console.log(dvFixed.byteLength);

// growable SharedArrayBuffer：视图与 DataView 同样 tracking（grow 单调）。
const gsab = new SharedArrayBuffer(4, { maxByteLength: 16 });
const sview = new Uint8Array(gsab);
const sdv = new DataView(gsab);
console.log(sview.length, sdv.byteLength);
gsab.grow(8);
console.log(sview.length, sdv.byteLength);
sview[7] = 9;
console.log(sview[7]);

// structuredClone：tracking 视图克隆保持 tracking（克隆 buffer 亦 resizable）。
const cloneView = structuredClone(track);
cloneView.buffer.resize(12);
console.log(cloneView.length, cloneView.buffer.resizable);
