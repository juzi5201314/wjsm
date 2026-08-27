// FieldDefinition 早错误（ES §15.7.1）：字段初始化器不得包含 arguments
// （穿透箭头函数；静态与实例字段同一条款）。
function outer() {
  class C {
    static x = (() => arguments.length)();
  }
  return C;
}
console.log(outer());
