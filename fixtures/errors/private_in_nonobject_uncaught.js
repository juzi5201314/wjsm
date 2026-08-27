// 未捕获的 `#x in 非对象` TypeError 必须终止执行并以运行时错误退出，
// 文案与 V8/Node 一致（实例私有字段显示 `#x`）。
class C {
  #x = 1;
  static probe(o) {
    return #x in o;
  }
}
C.probe(1);
console.log("unreachable");
