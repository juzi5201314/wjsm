// Issue #389: dynamic binary arithmetic gets a guarded native fast path.
// number+number must produce the same observable result, while string concat,
// object ToPrimitive, BigInt and implicit conversion still go through the host.
function add(a, b) { return a + b; }
function sub(a, b) { return a - b; }
function mul(a, b) { return a * b; }
function div(a, b) { return a / b; }

console.log(add(1, 2));
console.log(sub(5, 2));
console.log(mul(6, 7));
console.log(div(8, 2));
console.log(add(0, 0));
console.log(div(0, 0));

console.log(add("a", 2));
console.log(add(2, "a"));
console.log(add("1", 2));
console.log(sub("5", 2));
console.log(mul("6", 2));
console.log(div("8", 2));
console.log(add([1], 2));
console.log(add({ valueOf: function () { return 3; } }, 2));
console.log(add(2, { valueOf: function () { return 4; } }));

function check(label, fn) {
  try {
    fn();
    console.log(label, "no throw");
  } catch (e) {
    console.log(label, e.name);
  }
}
check("1n+2", function () { return add(1n, 2); });
check("1n-2", function () { return sub(1n, 2); });
check("10n*3", function () { return mul(10n, 3); });
check("5n/2", function () { return div(5n, 2); });
