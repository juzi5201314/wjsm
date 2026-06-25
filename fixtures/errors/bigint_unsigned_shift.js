function check(label, fn) {
  try {
    fn();
    console.log(label, "no throw");
  } catch (e) {
    console.log(label, e.name);
  }
}
check("5n>>>1n", function () { 5n >>> 1n; });
check("5n>>>1", function () { 5n >>> 1; });
check("5>>>1n", function () { 5 >>> 1n; });
check("5n&3", function () { 5n & 3; });