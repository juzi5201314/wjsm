function t(label, fn) {
  try {
    const v = fn();
    console.log(label, "ok", typeof v === "object" && v !== null ? "[obj]" : String(v));
  } catch (e) {
    console.log(label, e.constructor.name + "|" + e.message);
  }
}
// 品牌检查
t("ab.byteLength.call({})", () => Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength").get.call({}));
t("ab.slice.call({})", () => ArrayBuffer.prototype.slice.call({}, 0));
t("sab.byteLength.call({})", () => Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, "byteLength").get.call({}));
t("sab.growable.call({})", () => Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, "growable").get.call({}));
t("sab.maxByteLength.call({})", () => Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, "maxByteLength").get.call({}));
t("sab.grow.call({})", () => SharedArrayBuffer.prototype.grow.call({}, 1));
t("sab.slice.call({})", () => SharedArrayBuffer.prototype.slice.call({}, 0));
t("dv.buffer.call({})", () => Object.getOwnPropertyDescriptor(DataView.prototype, "buffer").get.call({}));
t("dv.byteLength.call({})", () => Object.getOwnPropertyDescriptor(DataView.prototype, "byteLength").get.call({}));
t("dv.byteOffset.call({})", () => Object.getOwnPropertyDescriptor(DataView.prototype, "byteOffset").get.call({}));
t("dv.getUint8.call({})", () => DataView.prototype.getUint8.call({}, 0));
t("dv.setUint8.call({})", () => DataView.prototype.setUint8.call({}, 0, 1));
// 构造器参数错误
t("new ArrayBuffer(-1)", () => new ArrayBuffer(-1));
t("new SharedArrayBuffer(-1)", () => new SharedArrayBuffer(-1));
t("new SharedArrayBuffer(8,{maxByteLength:-1})", () => new SharedArrayBuffer(8, { maxByteLength: -1 }));
t("new SharedArrayBuffer(8,{maxByteLength:4})", () => new SharedArrayBuffer(8, { maxByteLength: 4 }));
t("new SharedArrayBuffer(8,{maxByteLength:undefined})", () => new SharedArrayBuffer(8, { maxByteLength: undefined }).maxByteLength);
t("new DataView({})", () => new DataView({}));
t("new DataView()", () => new DataView());
t("new DataView(ab,-1)", () => new DataView(new ArrayBuffer(8), -1));
t("new DataView(ab,9)", () => new DataView(new ArrayBuffer(8), 9));
t("new DataView(ab,0,-1)", () => new DataView(new ArrayBuffer(8), 0, -1));
t("new DataView(ab,0,9)", () => new DataView(new ArrayBuffer(8), 0, 9));
t("new DataView(ab,4,5)", () => new DataView(new ArrayBuffer(8), 4, 5));
// DataView 越界读写
t("dv.getUint8(8)", () => new DataView(new ArrayBuffer(8)).getUint8(8));
t("dv.getFloat64(1)", () => new DataView(new ArrayBuffer(8)).getFloat64(1));
t("dv.setUint32(-1)", () => new DataView(new ArrayBuffer(8)).setUint32(-1, 0));
// grow 错误
t("sab.grow(-1)", () => { const s = new SharedArrayBuffer(4, { maxByteLength: 8 }); s.grow(-1); });
t("sab.grow(16)", () => { const s = new SharedArrayBuffer(4, { maxByteLength: 8 }); s.grow(16); });
t("sab.grow(2)", () => { const s = new SharedArrayBuffer(4, { maxByteLength: 8 }); s.grow(2); });
t("nogrow.grow(8)", () => { const s = new SharedArrayBuffer(4); s.grow(8); });
// 无参默认
t("new ArrayBuffer()", () => new ArrayBuffer().byteLength);
t("new SharedArrayBuffer()", () => new SharedArrayBuffer().byteLength);
t("new DataView(ab8)", () => { const d = new DataView(new ArrayBuffer(8)); return d.byteLength + "," + d.byteOffset; });
t("new DataView(ab8,4)", () => { const d = new DataView(new ArrayBuffer(8), 4); return d.byteLength + "," + d.byteOffset; });
