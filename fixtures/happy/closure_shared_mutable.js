function pair() {
  var val = 0;
  function inc() {
    val = val + 1;
  }
  function get() {
    return val;
  }
  return { inc: inc, get: get };
}
var p = pair();
p.inc();
p.inc();
p.inc();
console.log(p.get());
