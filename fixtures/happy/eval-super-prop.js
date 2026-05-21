var superProp = null;
var proto = { test262: 262 };
var o = {
  __proto__: proto,
  method() {
    superProp = eval('super.test262;');
  }
};
o.method();
console.log(superProp);
