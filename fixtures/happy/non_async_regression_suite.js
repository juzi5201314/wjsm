// Regression guard for the 3 non-async fixes (closures logical/shared mutable, for-of abrupt return close).
function run() {
  var x = 0;
  var hits = 0;
  function bump() { hits = hits + 1; return 1; }
  function update() {
    x &&= bump();
    x ||= bump();
    x ??= bump();
    return hits;
  }
  return update();
}
console.log(run()); // 1

function pair() {
  var val = 0;
  function inc() { val = val + 1; }
  function get() { return val; }
  return { inc: inc, get: get };
}
var p = pair();
p.inc(); p.inc(); p.inc();
console.log(p.get()); // 3

// for-of return close (simple array, side effect + value)
let closed = false;
let arr = [1];
for (let v of arr) {
  closed = true;
}
console.log(closed ? "done" : "no");
console.log(closed); // true
