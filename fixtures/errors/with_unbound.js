// with 体内未命中 with 对象且外层无静态绑定的名字：运行时 ReferenceError。
with ({ present: 1 }) {
  console.log(present);
  console.log(notDefinedAnywhere);
}
