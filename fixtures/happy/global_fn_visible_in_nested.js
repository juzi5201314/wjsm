function Test262Error(msg) {
  this.msg = msg;
}
function f() {
  return new Test262Error("ok");
}
var result = f();
console.log(result.msg);
