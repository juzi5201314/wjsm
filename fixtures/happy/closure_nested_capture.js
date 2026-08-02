// 回归：嵌套闭包捕获——外层经 env 读内层函数（函数声明）再调用时，
// 内层闭包 env 必须正确传递（issue #341）。
// 修复前：work 被误标 direct_callable，main 直接 functionref 调用（env=undefined），
// work 内 get_prop $env "$0.empty" 读取失败 → "value is not callable"。
let c = 0;
function empty() {
  c++;
}
function work() {
  empty();
}
work();
console.log(c);

// 变体：内层只读模块变量（读 env 返回值）。
let d = 5;
function read() {
  return d;
}
function callRead() {
  return read();
}
console.log(callRead());

// 变体：两层嵌套（work → middle → empty），经 env 读取链调用。
let e = 0;
function inner() {
  e += 2;
}
function middle() {
  inner();
}
function outer() {
  middle();
}
outer();
console.log(e);
