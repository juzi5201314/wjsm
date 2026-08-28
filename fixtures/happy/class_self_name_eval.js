// direct eval 在方法体内动态解析类自身名字：classEnv 绑定须对 eval 词法
// 解析可见（保守扫描把 eval 出现视为可能引用，保留 classEnv 帧）。
class H {
  m() { return eval("H"); }
}
console.log(new H().m() === H);
