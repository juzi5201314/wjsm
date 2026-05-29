var proto = { test262: 262 };
var o = {
  __proto__: proto,
  method() {
    eval('console.log(super.test262);');
  }
};
o.method();
