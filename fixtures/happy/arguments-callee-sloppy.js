function f() {
  console.log(typeof arguments.callee);
  console.log(arguments.callee === f);
}
f();