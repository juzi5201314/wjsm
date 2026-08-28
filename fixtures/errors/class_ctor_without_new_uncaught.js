// 类构造器不带 new 的未捕获调用：TypeError 终止进程（ES §10.2.1 步骤 2，
// 文案与 Node/V8 对齐）。
class C {}
C();
