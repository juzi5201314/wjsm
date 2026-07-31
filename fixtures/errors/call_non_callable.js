// 调用非函数值必须抛 TypeError（而非 wasm trap），且可被 try/catch 捕获。
try {
  undefined();
  console.log("no throw");
} catch (e) {
  console.log("caught:", e.name);
}
try {
  const o = {};
  o();
  console.log("no throw 2");
} catch (e) {
  console.log("caught 2:", e.name);
}
